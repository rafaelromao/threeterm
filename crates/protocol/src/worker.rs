//! Versioned worker protocol envelope and host trait.
//!
//! The host speaks the versioned protocol to a disposable worker over the
//! newline-framed JSON envelope. The `Envelope` enum is the canonical wire
//! shape: every variant carries the protocol's `schema_version` and uses
//! `kind` as the externally-tagged discriminator so a single JSON line
//! self-describes its shape.
//!
//! The `WorkerHost` trait is the host-side abstraction the supervisor and
//! the fake test transport share. The production wiring (OCCT, libslvs)
//! implements the trait over a real subprocess; the integration tests
//! implement it over an `mpsc` channel. Both run the same supervisor.
//!
//! Staged binary artifact bytes remain in a host-chosen private staging
//! directory. The `Artifact` envelope carries only the validated header;
//! the host independently checks the staged file before promotion.

use std::collections::VecDeque;
use std::fmt;
use std::io::{Read, Write};
use std::process::Child;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::ArtifactHeader;
use crate::frame::{FrameParser, MAX_FRAME_BUFFER};

/// Maximum size of a staged artifact payload, in bytes. Artifacts exceeding
/// this limit emit `ArtifactError::PayloadTooLarge` so a malicious or buggy
/// worker cannot exhaust the host's memory.
pub const MAX_ARTIFACT_BYTES: usize = 1 << 20;

/// Maximum cumulative stdout bytes the host accepts from one worker
/// before failing closed. A flooding worker that exceeds this bound is
/// terminated and reported as a structured `StreamOverflow` error.
pub const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;

/// Maximum cumulative stderr bytes the host retains for diagnostics
/// from one worker. The bounded tail is preserved so structured
/// diagnostic context survives a flooding worker.
pub const MAX_STDERR_BYTES: usize = 64 * 1024;

/// Poll interval used while waiting for a worker envelope. The
/// subprocess transport re-checks its stream overflow flags every slice
/// so a flood on a quiet stream is observed well before the deadline.
const OVERFLOW_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Byte bounds applied to a worker's standard streams. The host fails
/// closed when a stream exceeds its bound, terminating the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLimits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            stdout_bytes: MAX_STDOUT_BYTES,
            stderr_bytes: MAX_STDERR_BYTES,
        }
    }
}

/// The versioned envelope exchanged between host and worker.
///
/// Every variant carries `schema_version` so the host can reject frames
/// from a mismatched protocol version before parsing the payload. The
/// `kind` field is the externally-tagged discriminator emitted by serde.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Envelope {
    /// Emitted by the worker as soon as it has accepted the schema
    /// version and is ready to receive a `Request`.
    #[serde(rename = "worker_ready")]
    WorkerReady {
        schema_version: String,
        worker_id: String,
    },

    /// The host's request to a worker. `command_id` is a registered
    /// domain command; `args` is the schema-validated argument object;
    /// `revision_id` is the authoritative Revision Snapshot the worker
    /// must operate on.
    #[serde(rename = "request")]
    Request {
        schema_version: String,
        request_id: String,
        command_id: String,
        args: Value,
        revision_id: String,
    },

    /// The host's cooperative cancellation request.
    #[serde(rename = "cancel")]
    Cancel {
        schema_version: String,
        request_id: String,
        reason: String,
    },

    /// The worker's progress update. The host uses the most recent
    /// `Progress` to populate `TerminationRecord.last_progress` when a
    /// force-terminate fires.
    #[serde(rename = "progress")]
    Progress {
        schema_version: String,
        request_id: String,
        stage: String,
        percent: u8,
    },

    /// The worker's staged binary artifact. The host validates the
    /// advertised `sha256` against the decoded bytes before promotion;
    /// see `artifact::Stage`.
    #[serde(rename = "artifact")]
    Artifact {
        schema_version: String,
        header: Box<ArtifactHeader>,
    },

    /// The worker's terminal success envelope. `result` is the
    /// command-typed response value.
    #[serde(rename = "completed")]
    Completed {
        schema_version: String,
        request_id: String,
        result: Value,
    },

    /// The worker's terminal cooperative cancellation acknowledgement.
    /// Emitted after a `Cancel` envelope; arrives inside the supervisor's
    /// grace period on the cooperative path.
    #[serde(rename = "cancelled")]
    Cancelled {
        schema_version: String,
        request_id: String,
        reason: String,
    },

    /// The worker's terminal failure envelope. `code` is a stable
    /// diagnostic identifier; `detail` carries the offending argument.
    #[serde(rename = "failed")]
    Failed {
        schema_version: String,
        request_id: String,
        code: String,
        detail: String,
    },
}

impl Envelope {
    /// Returns the `schema_version` carried by this envelope. Used by the
    /// frame parser to reject envelopes from a mismatched protocol version
    /// before parsing the payload.
    pub fn schema_version(&self) -> &str {
        match self {
            Self::WorkerReady { schema_version, .. }
            | Self::Request { schema_version, .. }
            | Self::Cancel { schema_version, .. }
            | Self::Progress { schema_version, .. }
            | Self::Artifact { schema_version, .. }
            | Self::Completed { schema_version, .. }
            | Self::Cancelled { schema_version, .. }
            | Self::Failed { schema_version, .. } => schema_version,
        }
    }

    /// Returns the request_id for envelope variants that carry one, or
    /// `None` for `WorkerReady`. The supervisor uses this to thread the
    /// per-request identity into `TerminationRecord` and the cooperative
    /// ack.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::WorkerReady { .. } => None,
            Self::Request { request_id, .. }
            | Self::Cancel { request_id, .. }
            | Self::Progress { request_id, .. }
            | Self::Completed { request_id, .. }
            | Self::Cancelled { request_id, .. }
            | Self::Failed { request_id, .. } => Some(request_id),
            Self::Artifact { header, .. } => Some(&header.request_id),
        }
    }
}

/// The host-side abstraction of a single disposable worker.
///
/// `WorkerHost` is the boundary the supervisor and the fake test
/// transport share. The production wiring (OCCT, libslvs) implements it
/// over a real subprocess; the integration tests implement it over an
/// in-process channel. Both run the same `Supervisor`.
pub trait WorkerHost {
    /// Send an envelope to the worker. Returns `Err(WorkerError::Closed)`
    /// if the worker has already exited.
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError>;

    /// Receive the next envelope from the worker. Returns
    /// `Err(WorkerError::Closed)` when the worker has exited and no more
    /// envelopes are pending.
    fn recv(&mut self, deadline: std::time::Instant) -> Result<Envelope, WorkerError>;

    /// Send a cooperative cancellation for `request_id`. The worker is
    /// expected to acknowledge with a `Cancelled` envelope inside the
    /// supervisor's grace period; if it doesn't, the supervisor force-
    /// terminates the worker.
    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError>;

    /// Force-terminate and reap the disposable worker after grace expires.
    fn terminate(&mut self) -> Result<(), WorkerError> {
        Ok(())
    }
}

/// Newline-frame transport for a disposable worker byte stream.
///
/// A worker process adapter feeds stdout chunks into `inbound` and consumes
/// encoded host frames from `outbound`. Keeping the timed receive here means
/// both a process-backed adapter and an in-process channel use the same
/// deadline contract.
#[derive(Debug)]
pub struct FramedWorkerHost {
    inbound: Receiver<Vec<u8>>,
    outbound: Sender<Vec<u8>>,
    parser: FrameParser,
    pending: VecDeque<Envelope>,
}

impl FramedWorkerHost {
    pub fn new(inbound: Receiver<Vec<u8>>, outbound: Sender<Vec<u8>>) -> Self {
        Self {
            inbound,
            outbound,
            parser: FrameParser::new(),
            pending: VecDeque::new(),
        }
    }
}

impl WorkerHost for FramedWorkerHost {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        let frame = encode_frame(envelope)
            .map_err(|error| WorkerError::Protocol(format!("encode_frame failed: {error}")))?;
        if frame.len() > MAX_FRAME_BUFFER {
            return Err(WorkerError::Protocol(format!(
                "host frame of {} bytes exceeds the {MAX_FRAME_BUFFER} byte bound",
                frame.len()
            )));
        }
        self.outbound.send(frame).map_err(|_| WorkerError::Closed)
    }

    fn recv(&mut self, deadline: Instant) -> Result<Envelope, WorkerError> {
        loop {
            if Instant::now() >= deadline {
                return Err(WorkerError::TimedOut);
            }

            if let Some(envelope) = self.pending.pop_front() {
                return Ok(envelope);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let bytes = if remaining.is_zero() {
                match self.inbound.try_recv() {
                    Ok(bytes) => bytes,
                    Err(TryRecvError::Empty) => return Err(WorkerError::TimedOut),
                    Err(TryRecvError::Disconnected) => return Err(WorkerError::Closed),
                }
            } else {
                match self.inbound.recv_timeout(remaining) {
                    Ok(bytes) => bytes,
                    Err(RecvTimeoutError::Timeout) => return Err(WorkerError::TimedOut),
                    Err(RecvTimeoutError::Disconnected) => return Err(WorkerError::Closed),
                }
            };
            let envelopes = self
                .parser
                .push(&bytes)
                .map_err(|error| WorkerError::Protocol(error.to_string()))?;
            self.pending.extend(envelopes);
        }
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.send(&Envelope::Cancel {
            schema_version: crate::schema_version().to_string(),
            request_id: request_id.to_string(),
            reason: reason.to_string(),
        })
    }
}

/// Production transport binding a subprocess's standard streams to the
/// deadline-aware newline-frame transport.
///
/// The stdout and stderr streams are read by bounded threads: each
/// stream has a byte cap (see [`StreamLimits`]) and a worker that
/// exceeds its cap fails the host closed with a structured
/// [`WorkerError::StreamOverflow`] instead of exhausting host memory.
/// The captured stderr tail is preserved for diagnostics.
#[derive(Debug)]
pub struct SubprocessWorkerHost {
    child: Child,
    transport: FramedWorkerHost,
    limits: StreamLimits,
    stdout_overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stderr_overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl SubprocessWorkerHost {
    /// Wrap a spawned worker process with default stream bounds.
    pub fn new(child: Child) -> Result<Self, WorkerError> {
        Self::with_limits(child, StreamLimits::default())
    }

    /// Wrap a spawned worker process with explicit stream bounds.
    pub fn with_limits(mut child: Child, limits: StreamLimits) -> Result<Self, WorkerError> {
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => return Err(missing_pipe_error(&mut child, "stdin")),
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => return Err(missing_pipe_error(&mut child, "stdout")),
        };
        let stderr = child.stderr.take();
        let (inbound_tx, inbound_rx) = std::sync::mpsc::channel();
        let (outbound_tx, outbound_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        let stdout_overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stderr_overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stderr_tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let stdout_cap = limits.stdout_bytes;
        let stdout_overflow_flag = std::sync::Arc::clone(&stdout_overflow);
        std::thread::spawn(move || {
            let mut stdout = stdout;
            let mut buffer = [0; 4096];
            let mut total: usize = 0;
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 || inbound_tx.send(buffer[..read].to_vec()).is_err() {
                    break;
                }
                total += read;
                if total > stdout_cap {
                    stdout_overflow_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
            }
        });

        if let Some(stderr) = stderr {
            let stderr_cap = limits.stderr_bytes;
            let stderr_overflow_flag = std::sync::Arc::clone(&stderr_overflow);
            let stderr_tail_shared = std::sync::Arc::clone(&stderr_tail);
            std::thread::spawn(move || {
                let mut stderr = stderr;
                let mut buffer = [0; 4096];
                while let Ok(read) = stderr.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let chunk = &buffer[..read];
                    let mut tail = stderr_tail_shared.lock().expect("stderr tail mutex");
                    if tail.len() + chunk.len() <= stderr_cap {
                        tail.extend_from_slice(chunk);
                    } else {
                        stderr_overflow_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        // Keep the newest bytes so the diagnostic tail
                        // survives the overflow.
                        let keep = stderr_cap.saturating_sub(chunk.len());
                        if keep == 0 {
                            tail.clear();
                            tail.extend_from_slice(&chunk[chunk.len() - stderr_cap..]);
                        } else {
                            let drop = tail.len().saturating_sub(keep);
                            tail.drain(..drop);
                            tail.extend_from_slice(chunk);
                        }
                    }
                }
            });
        }

        std::thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(frame) = outbound_rx.recv() {
                if stdin.write_all(&frame).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            transport: FramedWorkerHost::new(inbound_rx, outbound_tx),
            limits,
            stdout_overflow,
            stderr_overflow,
            stderr_tail,
        })
    }

    /// Returns the bounded stderr tail captured so far. The tail never
    /// exceeds the configured `stderr_bytes` cap.
    pub fn stderr_tail(&self) -> Vec<u8> {
        self.stderr_tail.lock().expect("stderr tail mutex").clone()
    }

    /// Fail closed with a structured `StreamOverflow` error when either
    /// standard stream exceeded its configured bound.
    fn fail_closed_on_overflow(&self) -> Result<(), WorkerError> {
        if self
            .stdout_overflow
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(WorkerError::StreamOverflow {
                stream: "stdout",
                limit: self.limits.stdout_bytes,
            });
        }
        if self
            .stderr_overflow
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(WorkerError::StreamOverflow {
                stream: "stderr",
                limit: self.limits.stderr_bytes,
            });
        }
        Ok(())
    }
}

fn missing_pipe_error(child: &mut Child, pipe: &str) -> WorkerError {
    // Constructor failure must not leave a disposable worker running.
    let _ = child.kill();
    let _ = child.wait();
    WorkerError::Io(std::io::Error::other(format!(
        "worker {pipe} was not piped"
    )))
}

impl WorkerHost for SubprocessWorkerHost {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        self.transport.send(envelope)
    }

    fn recv(&mut self, deadline: Instant) -> Result<Envelope, WorkerError> {
        // Poll in short slices so an overflow on a silent stream (e.g. a
        // stderr flood while stdout goes quiet) is observed before the
        // deadline instead of blocking until `deadline` and being
        // misreported as a timeout.
        loop {
            self.fail_closed_on_overflow()?;
            if Instant::now() >= deadline {
                return Err(WorkerError::TimedOut);
            }
            let slice = deadline.min(Instant::now() + OVERFLOW_POLL_INTERVAL);
            match self.transport.recv(slice) {
                Ok(envelope) => return Ok(envelope),
                Err(WorkerError::TimedOut) => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.transport.cancel(request_id, reason)
    }

    fn terminate(&mut self) -> Result<(), WorkerError> {
        match self.child.try_wait()? {
            Some(_) => Ok(()),
            None => {
                if let Err(error) = self.child.kill()
                    && self.child.try_wait()?.is_none()
                {
                    return Err(error.into());
                }
                self.child.wait()?;
                Ok(())
            }
        }
    }
}

impl Drop for SubprocessWorkerHost {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

/// Production wiring of `WorkerHost` over a real subprocess. The
/// foundation slice does not implement this trait; it exists so the
/// future OCCT/`libslvs` adapters have a seam.
pub trait WorkerProcess {
    /// Spawn a fresh disposable worker and return a boxed `WorkerHost`
    /// bound to it. One spawn per `Request` (closed issue #49).
    fn spawn(config: WorkerConfig) -> Result<Box<dyn WorkerHost>, WorkerError>;
}

/// Configuration handed to `WorkerProcess::spawn`.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub worker_id: &'static str,
    pub schema_version: &'static str,
    pub command_line: Vec<String>,
}

/// Errors emitted by `WorkerHost` implementations. The supervisor maps
/// every variant to a structured `WorkerError` diagnostic so callers
/// never have to inspect free-form text.
#[derive(Debug)]
pub enum WorkerError {
    /// No complete envelope arrived before the receive deadline.
    TimedOut,
    /// The worker process exited or its channel closed before delivering
    /// the requested envelope.
    Closed,
    /// The worker emitted a frame that the host could not parse.
    Protocol(String),
    /// The worker emitted more bytes on a standard stream than the
    /// configured bound. The host fails closed: the worker is
    /// terminated and no partial output is accepted.
    StreamOverflow { stream: &'static str, limit: usize },
    /// The worker emitted an envelope with a schema_version the host
    /// does not recognize.
    SchemaMismatch { received: String, expected: String },
    /// I/O error talking to the worker's stdin/stdout.
    Io(std::io::Error),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => formatter.write_str("worker receive deadline exceeded"),
            Self::Closed => formatter.write_str("worker closed before delivering envelope"),
            Self::Protocol(detail) => write!(formatter, "worker protocol violation: {detail}"),
            Self::StreamOverflow { stream, limit } => {
                write!(formatter, "worker {stream} exceeded the {limit} byte bound")
            }
            Self::SchemaMismatch { received, expected } => write!(
                formatter,
                "worker schema mismatch: received {received:?}, expected {expected:?}"
            ),
            Self::Io(error) => write!(formatter, "worker io error: {error}"),
        }
    }
}

impl std::error::Error for WorkerError {}

impl From<std::io::Error> for WorkerError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Returns the canonical JSON encoding of `envelope` followed by `\n`.
/// The host and worker speak newline-framed JSON; every line carries one
/// envelope (closed issue #49).
pub fn encode_frame(envelope: &Envelope) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec(envelope)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{Layer1CacheKey, WorkerFingerprint};
    use serde_json::json;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    fn schema() -> String {
        crate::schema_version().to_string()
    }

    #[test]
    fn cancelled_envelope_round_trips_with_kind_discriminator() {
        let envelope = Envelope::Cancelled {
            schema_version: schema(),
            request_id: "req-7".to_string(),
            reason: "user requested stop".to_string(),
        };

        let encoded = serde_json::to_value(&envelope).expect("envelope serializes");
        assert_eq!(encoded["kind"], Value::from("cancelled"));
        assert_eq!(encoded["schema_version"], Value::from(schema()));
        assert_eq!(encoded["request_id"], Value::from("req-7"));
        assert_eq!(encoded["reason"], Value::from("user requested stop"));

        let decoded: Envelope = serde_json::from_value(encoded).expect("envelope deserializes");
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn artifact_envelope_carries_staged_artifact_header() {
        let fingerprint = WorkerFingerprint {
            worker_kind: "occt".to_string(),
            worker_schema_version: "threeterm.workers.occt/1".to_string(),
            protocol_schema_version: schema(),
        };
        let envelope = Envelope::Artifact {
            schema_version: schema(),
            header: Box::new(ArtifactHeader {
                request_id: "req-7".to_string(),
                source_revision_id: "rev-1".to_string(),
                cache_key: Layer1CacheKey {
                    source_revision_id: "rev-1".to_string(),
                    worker_fingerprint: fingerprint.clone(),
                    artifact_kind: "brep".to_string(),
                    semantic_input_sha256: "11".repeat(32),
                    deterministic_settings_sha256: "22".repeat(32),
                },
                worker_fingerprint: fingerprint,
                artifact_kind: "brep".to_string(),
                staging_name: "sketch-1.brep".to_string(),
                byte_count: 5,
                sha256: "deadbeef".to_string(),
            }),
        };

        let encoded = serde_json::to_value(&envelope).expect("envelope serializes");
        assert_eq!(encoded["kind"], Value::from("artifact"));
        assert!(encoded.get("bytes_b64").is_none());
        assert_eq!(encoded["header"]["sha256"], Value::from("deadbeef"));

        let decoded: Envelope = serde_json::from_value(encoded).expect("envelope deserializes");
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn unknown_kind_discriminator_is_rejected_on_deserialize() {
        let value = json!({
            "kind": "not_a_real_envelope",
            "schema_version": schema(),
        });

        let error = serde_json::from_value::<Envelope>(value)
            .expect_err("unknown kind discriminator must be rejected");
        let message = error.to_string();
        assert!(
            message.contains("not_a_real_envelope") || message.contains("unknown variant"),
            "deserialization error should mention the offending kind, got {message:?}"
        );
    }

    #[test]
    fn envelope_request_id_is_none_for_worker_ready() {
        let envelope = Envelope::WorkerReady {
            schema_version: schema(),
            worker_id: "occt-worker".to_string(),
        };
        assert_eq!(envelope.request_id(), None);
        assert_eq!(envelope.schema_version(), schema());
    }

    #[test]
    fn envelope_request_id_is_some_for_progress_and_terminal_variants() {
        let progress = Envelope::Progress {
            schema_version: schema(),
            request_id: "req-1".to_string(),
            stage: "tessellating".to_string(),
            percent: 42,
        };
        assert_eq!(progress.request_id(), Some("req-1"));

        let completed = Envelope::Completed {
            schema_version: schema(),
            request_id: "req-1".to_string(),
            result: json!({ "ok": true }),
        };
        assert_eq!(completed.request_id(), Some("req-1"));
    }

    #[test]
    fn encode_frame_emits_one_json_object_terminated_with_newline() {
        let envelope = Envelope::WorkerReady {
            schema_version: schema(),
            worker_id: "occt-worker".to_string(),
        };
        let frame = encode_frame(&envelope).expect("frame encodes");

        assert!(
            frame.ends_with(b"\n"),
            "frame must end with a newline; got {:?}",
            std::str::from_utf8(&frame)
        );
        let body = &frame[..frame.len() - 1];
        let parsed: Envelope =
            serde_json::from_slice(body).expect("frame body is a valid envelope");
        assert_eq!(parsed, envelope);
    }

    #[test]
    fn framed_transport_times_out_at_an_expired_deadline_without_consuming_a_later_frame() {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (_outbound_tx, outbound_rx) = mpsc::channel();
        let mut transport = FramedWorkerHost::new(inbound_rx, _outbound_tx);
        let ready = Envelope::WorkerReady {
            schema_version: schema(),
            worker_id: "occt-worker".to_string(),
        };
        assert!(matches!(
            transport.recv(Instant::now() - Duration::from_nanos(1)),
            Err(WorkerError::TimedOut)
        ));
        inbound_tx
            .send(encode_frame(&ready).expect("ready frame encodes"))
            .expect("worker frame queues");
        assert_eq!(
            transport
                .recv(Instant::now() + Duration::from_secs(1))
                .expect("queued frame remains available"),
            ready
        );
        assert!(outbound_rx.try_recv().is_err());
    }

    #[test]
    fn framed_transport_preserves_multiple_envelopes_from_one_chunk() {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, _outbound_rx) = mpsc::channel();
        let mut transport = FramedWorkerHost::new(inbound_rx, outbound_tx);
        let ready = Envelope::WorkerReady {
            schema_version: schema(),
            worker_id: "occt-worker".to_string(),
        };
        let progress = Envelope::Progress {
            schema_version: schema(),
            request_id: "req-1".to_string(),
            stage: "tessellating".to_string(),
            percent: 50,
        };
        let mut chunk = encode_frame(&ready).expect("ready frame encodes");
        chunk.extend(encode_frame(&progress).expect("progress frame encodes"));
        inbound_tx.send(chunk).expect("worker chunk queues");

        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            transport.recv(deadline).expect("ready frame decodes"),
            ready
        );
        assert!(matches!(
            transport.recv(Instant::now() - Duration::from_nanos(1)),
            Err(WorkerError::TimedOut)
        ));
        assert_eq!(transport.pending.pop_front(), Some(progress));
    }

    #[test]
    fn framed_transport_times_out_when_only_a_partial_frame_arrives() {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, _outbound_rx) = mpsc::channel();
        let mut transport = FramedWorkerHost::new(inbound_rx, outbound_tx);
        let ready = Envelope::WorkerReady {
            schema_version: schema(),
            worker_id: "occt-worker".to_string(),
        };
        let frame = encode_frame(&ready).expect("ready frame encodes");
        inbound_tx
            .send(frame[..frame.len() - 1].to_vec())
            .expect("partial frame queues");

        assert!(matches!(
            transport.recv(Instant::now()),
            Err(WorkerError::TimedOut)
        ));
    }

    #[test]
    fn subprocess_transport_honors_an_expired_receive_deadline() {
        let child = Command::new("sh")
            .args(["-c", "exec cat >/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("blocked worker starts");
        let mut transport = SubprocessWorkerHost::new(child).expect("subprocess transport starts");

        assert!(matches!(
            transport.recv(Instant::now()),
            Err(WorkerError::TimedOut)
        ));
        transport.terminate().expect("blocked worker terminates");
    }
}

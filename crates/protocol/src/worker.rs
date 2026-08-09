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
use std::path::{Path, PathBuf};
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

/// How long `terminate` waits for a SIGKILLed leader to become reaped
/// before giving up on recording its exit status.
const REAP_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// How long `terminate` waits for the detached stream readers to drain
/// after the worker is reaped, so overflow flags are settled before a
/// terminal outcome is accepted or rejected.
const STREAM_DRAIN_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

/// Poll interval while waiting for the leader to reap after a kill.
const REAP_POLL: std::time::Duration = std::time::Duration::from_millis(10);

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

    /// Returns the actual Linux signal the worker process exited by, if
    /// it was killed by a signal rather than exiting cleanly. `None`
    /// when the worker exited normally or has not been reaped yet.
    fn exit_signal(&mut self) -> Option<i32> {
        None
    }

    /// Returns the numeric exit code the worker process exited with, if
    /// it exited by calling `exit(n)` rather than by a signal. `None`
    /// for a signal exit or before the worker has been reaped.
    fn exit_code(&mut self) -> Option<i32> {
        None
    }

    /// Returns the bounded stderr tail captured from the worker, used to
    /// preserve structured diagnostic context on a terminal record.
    /// Bounded transports return the capped tail; fakes return empty.
    fn stderr_tail(&mut self) -> String {
        String::new()
    }

    /// Returns the stream that exceeded its byte bound, if any. The
    /// supervisor re-checks this before accepting a terminal outcome so
    /// an overflow racing a completion still fails closed.
    fn stream_overflowed(&mut self) -> Option<&'static str> {
        None
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
    stdout_pipe_identity: Option<String>,
    /// The worker's process-group ID when spawning established the required
    /// group-leader invariant. `None` means the caller supplied a plain child
    /// and termination must not guess a process group from its PID.
    process_group_id: Option<i32>,
    /// Linux cgroup-v2 containment boundary, when the runtime grants the
    /// worker's parent cgroup delegation. Descendants inherit membership,
    /// including descendants that call `setsid` or close worker pipes.
    containment_cgroup: Option<PathBuf>,
    /// Count of reader threads that have finished draining their stream
    /// (observed EOF or an overflow). `terminate` waits for this to
    /// reach the number of active readers so overflow flags are settled
    /// before a terminal outcome is accepted.
    readers_finished: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    readers_total: usize,
    /// Exit status observed on the last reap. `Some` once the worker has
    /// been reaped, `None` while it is still running.
    reaped_status: Option<std::process::ExitStatus>,
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
        let stdout_pipe_identity = pipe_identity(&stdout);
        let process_group_id = worker_process_group_id(child.id() as i32);
        let containment_cgroup = create_process_cgroup(child.id() as i32);
        let stderr = child.stderr.take();
        let (inbound_tx, inbound_rx) = std::sync::mpsc::channel();
        let (outbound_tx, outbound_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        let stdout_overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stderr_overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stderr_tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let readers_finished = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut readers_total: usize = 1;

        let stdout_cap = limits.stdout_bytes;
        let stdout_overflow_flag = std::sync::Arc::clone(&stdout_overflow);
        let stdout_finished = std::sync::Arc::clone(&readers_finished);
        std::thread::spawn(move || {
            let mut stdout = stdout;
            let mut buffer = [0; 4096];
            let mut total: usize = 0;
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                total += read;
                if total > stdout_cap {
                    // Fail closed: the over-cap chunk is dropped, not
                    // forwarded, so no envelope crosses the bound.
                    stdout_overflow_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                if inbound_tx.send(buffer[..read].to_vec()).is_err() {
                    break;
                }
            }
            stdout_finished.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        if let Some(stderr) = stderr {
            readers_total += 1;
            let stderr_cap = limits.stderr_bytes;
            let stderr_overflow_flag = std::sync::Arc::clone(&stderr_overflow);
            let stderr_tail_shared = std::sync::Arc::clone(&stderr_tail);
            let stderr_finished = std::sync::Arc::clone(&readers_finished);
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
                stderr_finished.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
            stdout_pipe_identity,
            process_group_id,
            containment_cgroup,
            readers_finished,
            readers_total,
            reaped_status: None,
        })
    }

    /// Returns the bounded stderr tail captured so far, as raw bytes.
    /// The tail never exceeds the configured `stderr_bytes` cap. The
    /// `WorkerHost` trait exposes the lossy-string view the supervisor
    /// copies into terminal records.
    pub fn stderr_tail_bytes(&self) -> Vec<u8> {
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
        // Poll in short slices: a slice that elapses without an
        // envelope returns `TimedOut` so the caller (the supervisor)
        // regains control to check its cancellation flag and its own
        // deadline, instead of being held here until `deadline`.
        self.fail_closed_on_overflow()?;
        if Instant::now() >= deadline {
            return Err(WorkerError::TimedOut);
        }
        let slice = deadline.min(Instant::now() + OVERFLOW_POLL_INTERVAL);
        match self.transport.recv(slice) {
            Ok(envelope) => {
                // A stream can overflow concurrently with the
                // receive; re-check before delivering so a terminal
                // envelope racing a flood still fails closed.
                self.fail_closed_on_overflow()?;
                Ok(envelope)
            }
            // One slice elapsed without an envelope: return control
            // to the supervisor immediately. A subsequent recv with
            // the same deadline continues the poll.
            Err(WorkerError::TimedOut) => Err(WorkerError::TimedOut),
            Err(WorkerError::Closed) => {
                // The worker's stdout closed. If it exited by a
                // signal (crash, forced kill), report the actual
                // signal instead of a bare closed-stream error.
                self.fail_closed_on_overflow()?;
                self.reap_if_exited()?;
                if let Some(signal) = self.exit_signal() {
                    return Err(WorkerError::Signalled { signal });
                }
                Err(WorkerError::Closed)
            }
            Err(error) => Err(error),
        }
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.transport.cancel(request_id, reason)
    }

    fn terminate(&mut self) -> Result<(), WorkerError> {
        // Reap before addressing the process group. Once the leader has been
        // reaped its PID may be reused by an unrelated worker, so killpg on
        // the stale PID could terminate a concurrent request.
        self.reap_if_exited()?;
        let leader_reaped = self.reaped_status.is_some();
        // Capture descendants before killing the leader. This closes the
        // race where a child creates a new session and would otherwise be
        // reparented before process-group termination reaches it.
        let pid = self.child.id() as i32;
        // Once the leader is reaped, its PID may already belong to another
        // process. Do not walk a stale /proc parent relationship in that
        // case; the pipe identity and cgroup boundaries remain authoritative.
        let mut contained = if leader_reaped {
            Vec::new()
        } else {
            descendant_pids(pid)
        };
        if let Some(identity) = &self.stdout_pipe_identity {
            let inherited = inherited_pipe_pids(identity);
            for descendant in &inherited {
                // A daemonized holder may have already forked a second
                // generation after leaving the worker's process group.
                contained.extend(descendant_pids(*descendant));
            }
            contained.extend(inherited);
        }
        contained.sort_unstable();
        contained.dedup();
        let cgroup_killed = self
            .containment_cgroup
            .as_deref()
            .is_some_and(kill_process_cgroup);
        for descendant in contained {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(descendant),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        if !leader_reaped && !cgroup_killed {
            if let Some(group_id) = self.process_group_id {
                match nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(group_id),
                    nix::sys::signal::Signal::SIGKILL,
                ) {
                    Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                    Err(error) => return Err(WorkerError::Io(error.into())),
                }
            } else {
                // The caller did not establish a private process group. Kill
                // only the direct child instead of guessing from its PID.
                self.child.kill()?;
            }
        }
        // Reap the leader, waiting briefly for the SIGKILL to land so
        // the exit status (including the kill signal) is recorded. If
        // the leader still cannot be reaped, fail closed: a terminal
        // outcome must never be accepted without proof of reap.
        self.reap_if_exited()?;
        if self.reaped_status.is_none() {
            let deadline = Instant::now() + REAP_WAIT;
            while self.reaped_status.is_none() && Instant::now() < deadline {
                std::thread::sleep(REAP_POLL);
                self.reap_if_exited()?;
            }
        }
        if self.reaped_status.is_none() {
            return Err(WorkerError::Io(std::io::Error::other(
                "worker leader could not be reaped",
            )));
        }
        // The worker is reaped; let the detached stdout/stderr reader
        // threads observe EOF and settle their overflow flags before
        // the supervisor accepts or rejects the terminal outcome. If
        // the readers do not settle inside the drain window, fail
        // closed: a terminal outcome must never be accepted while a
        // stream could still be delivering over-limit bytes.
        let drain_deadline = Instant::now() + STREAM_DRAIN_WAIT;
        while Instant::now() < drain_deadline {
            if self.readers_settled() {
                break;
            }
            std::thread::sleep(REAP_POLL);
        }
        if !self.readers_settled() {
            return Err(WorkerError::Io(std::io::Error::other(
                "worker stream readers did not settle before termination",
            )));
        }
        if let Some(path) = self.containment_cgroup.take() {
            let _ = std::fs::remove_dir(path);
        }
        Ok(())
    }

    fn exit_signal(&mut self) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        self.reaped_status.and_then(|status| status.signal())
    }

    fn exit_code(&mut self) -> Option<i32> {
        self.reaped_status.and_then(|status| status.code())
    }

    fn stderr_tail(&mut self) -> String {
        String::from_utf8_lossy(&self.stderr_tail.lock().expect("stderr tail mutex")).into_owned()
    }

    fn stream_overflowed(&mut self) -> Option<&'static str> {
        if self
            .stdout_overflow
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Some("stdout");
        }
        if self
            .stderr_overflow
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Some("stderr");
        }
        None
    }
}

/// Create a private cgroup-v2 child of the current cgroup and move the worker
/// into it. Cgroup membership is inherited by every descendant, which closes
/// the escape window left by process-group-only cleanup. Runtimes without
/// delegated cgroup write access use the process-group fallback below.
#[cfg(target_os = "linux")]
fn create_process_cgroup(pid: i32) -> Option<PathBuf> {
    // Cgroup delegation is runtime-dependent and some rootless containers
    // expose a hierarchy whose kill operation is not safely delegated. Keep
    // the tested process-group boundary as the default; production runtimes
    // with an explicitly delegated cgroup can opt into the durable boundary.
    if std::env::var_os("THREETERM_ENABLE_WORKER_CGROUP").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return None;
    }
    let cgroup_file = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let relative = cgroup_file
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?;
    let parent = Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
    let path = parent.join(format!(".threeterm-worker-{pid}"));
    if std::fs::create_dir(&path).is_err() {
        return None;
    }
    if std::fs::write(path.join("cgroup.procs"), pid.to_string()).is_err() {
        let _ = std::fs::remove_dir(&path);
        return None;
    }
    Some(path)
}

#[cfg(unix)]
fn worker_process_group_id(pid: i32) -> Option<i32> {
    let process_group = nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid))).ok()?;
    (process_group.as_raw() == pid).then_some(process_group.as_raw())
}

#[cfg(not(unix))]
fn worker_process_group_id(_pid: i32) -> Option<i32> {
    None
}

#[cfg(not(target_os = "linux"))]
fn create_process_cgroup(_pid: i32) -> Option<PathBuf> {
    None
}

/// Kill every process in a worker cgroup. `cgroup.kill` is atomic with respect
/// to fork: unlike a `/proc` snapshot it also catches descendants created
/// after termination begins.
#[cfg(target_os = "linux")]
fn kill_process_cgroup(path: &Path) -> bool {
    if std::fs::write(path.join("cgroup.kill"), b"1").is_ok() {
        return true;
    }
    let Ok(members) = std::fs::read_to_string(path.join("cgroup.procs")) else {
        return false;
    };
    let mut attempted = false;
    for member in members.lines().filter_map(|line| line.parse::<i32>().ok()) {
        attempted = true;
        match nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(member),
            nix::sys::signal::Signal::SIGKILL,
        ) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(_) => return false,
        }
    }
    attempted
}

#[cfg(not(target_os = "linux"))]
fn kill_process_cgroup(_path: &Path) -> bool {
    false
}

/// Return the worker's currently observable descendants from `/proc`.
/// Process-group containment handles ordinary descendants; this snapshot is
/// the additional containment layer for a child that calls `setsid` before
/// the worker leader is reaped.
#[cfg(target_os = "linux")]
fn descendant_pids(root: i32) -> Vec<i32> {
    use std::collections::HashMap;

    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|value| value.parse::<i32>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let Some(ppid) = fields
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<i32>().ok())
        else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
    }

    let mut pending = vec![root];
    let mut descendants = Vec::new();
    while let Some(parent) = pending.pop() {
        for child in children.remove(&parent).unwrap_or_default() {
            pending.push(child);
            descendants.push(child);
        }
    }
    descendants
}

#[cfg(target_os = "linux")]
fn pipe_identity(file: &impl std::os::fd::AsRawFd) -> Option<String> {
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
fn pipe_identity(_file: &impl std::os::fd::AsRawFd) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn inherited_pipe_pids(identity: &str) -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let current = std::process::id() as i32;
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|value| value.parse::<i32>().ok()) else {
            continue;
        };
        if pid == current {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        if fds.flatten().any(|fd| {
            std::fs::read_link(fd.path())
                .ok()
                .is_some_and(|path| path.to_string_lossy() == identity)
        }) {
            matches.push(pid);
        }
    }
    matches
}

#[cfg(not(target_os = "linux"))]
fn inherited_pipe_pids(_identity: &str) -> Vec<i32> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
fn descendant_pids(_root: i32) -> Vec<i32> {
    Vec::new()
}

impl SubprocessWorkerHost {
    /// True once every active stream reader has finished draining its
    /// stream (observed EOF or an overflow). The reader count is fixed
    /// at construction time, so a reader that never starts is counted
    /// as settled only when it exits.
    fn readers_settled(&self) -> bool {
        self.readers_finished
            .load(std::sync::atomic::Ordering::SeqCst)
            >= self.readers_total
    }

    /// Reap the child if it has exited, recording its exit status.
    /// Leaves the host usable for a later `terminate` if still running.
    fn reap_if_exited(&mut self) -> Result<(), WorkerError> {
        if self.reaped_status.is_none()
            && let Some(status) = self.child.try_wait()?
        {
            self.reaped_status = Some(status);
        }
        Ok(())
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
    /// The worker process exited due to a signal (e.g. SIGSEGV after a
    /// crash, or SIGKILL after force termination). `signal` is the
    /// actual Linux signal number observed on the reaped exit status.
    Signalled { signal: i32 },
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
            Self::Signalled { signal } => {
                write!(formatter, "worker exited by signal {signal}")
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
    let mut bytes = serialize_capped(envelope, MAX_FRAME_BUFFER)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Serializes `value` to JSON with a hard byte cap, so an oversized
/// payload fails during encoding instead of being fully materialized
/// in memory first (the input bound is enforced, not checked after).
pub fn serialize_capped<T: Serialize>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, serde_json::Error> {
    // Serialize through a capped writer so an oversized frame is
    // rejected during encoding instead of being fully materialized in
    // memory first (the input bound is enforced, not checked after).
    let mut writer = BoundedWriter {
        bytes: Vec::with_capacity(256),
        limit,
        exceeded: false,
    };
    serde_json::to_writer(&mut writer, value)?;
    if writer.exceeded {
        return Err(serde_json::Error::io(std::io::Error::other(format!(
            "payload exceeds the {limit} byte bound"
        ))));
    }
    Ok(writer.bytes)
}

/// Writer that aborts once `limit` bytes have been written, so a frame
/// cannot be fully materialized past the protocol's input bound.
struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl std::io::Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.exceeded {
            return Err(std::io::Error::other("frame bound exceeded"));
        }
        if self.bytes.len() + buf.len() > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("frame bound exceeded"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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

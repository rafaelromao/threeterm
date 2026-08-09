//! End-to-end integration test for the worker protocol round-trip and
//! cooperative cancellation lifecycle.
//!
//! Wires the production `Supervisor` against a fake in-process
//! `WorkerHost` (over the same trait the production `WorkerProcess`
//! adapter will use) and asserts both the cooperative-cancel-acks path
//! and the force-terminate-after-grace path produce structured records.
//! The fake worker is fully synchronous so CI is deterministic. The
//! wire layer is exercised by piping the fake's `send`/`recv` traffic
//! through `encode_frame` and `FrameParser` so the demoable behavior
//! covers the same code path the production subprocess wiring will use.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use threeterm_protocol::artifact::{
    ArtifactHeader, Layer1CacheKey, Stage, WorkerFingerprint, sha256_hex,
};
use threeterm_protocol::frame::FrameParser;
use threeterm_protocol::schema_version;
use threeterm_protocol::supervisor::{
    ExitKind, Request, Supervisor, SupervisorOutcome, TerminationRecord,
};
use threeterm_protocol::worker::{Envelope, WorkerError, WorkerHost, encode_frame};

/// A fake worker that serves envelopes from a `mpsc`-style queue. `recv`
/// returns the next envelope and `send` records what the host emitted.
/// The fake never sleeps; the supervisor's grace period is exercised
/// with a sub-millisecond `Duration`.
struct FakeWorker {
    received: Vec<Envelope>,
    pending: VecDeque<Envelope>,
    cancel_calls: Vec<(String, String)>,
}

impl FakeWorker {
    fn new(script: Vec<Envelope>) -> Self {
        Self {
            received: Vec::new(),
            pending: script.into(),
            cancel_calls: Vec::new(),
        }
    }
}

impl WorkerHost for FakeWorker {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        self.received.push(envelope.clone());
        Ok(())
    }

    fn recv(&mut self, _deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        self.pending.pop_front().ok_or(WorkerError::Closed)
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.cancel_calls
            .push((request_id.to_string(), reason.to_string()));
        Ok(())
    }
}

fn sample_request() -> Request {
    Request {
        request_id: "req-1".to_string(),
        command_id: "list".to_string(),
        args: serde_json::json!({}),
        revision_id: "rev-0".to_string(),
    }
}

/// Decodes a single envelope through `FrameParser` so the wire layer is
/// exercised end-to-end. Returns the parsed envelope.
fn round_trip(envelope: &Envelope) -> Envelope {
    let frame = encode_frame(envelope).expect("envelope encodes");
    let mut parser = FrameParser::new();
    let decoded = parser.push(&frame).expect("frame decodes");
    decoded
        .into_iter()
        .next()
        .expect("frame produced exactly one envelope")
}

/// Canonical `WorkerReady` handshake envelope the production wire
/// protocol expects as the worker's first envelope.
fn ready_envelope() -> Envelope {
    Envelope::WorkerReady {
        schema_version: schema_version().to_string(),
        worker_id: "fake".to_string(),
    }
}

fn artifact_header(
    staging_root: &std::path::Path,
    bytes: &[u8],
    sha256: String,
) -> Box<ArtifactHeader> {
    let staged = Stage::open(staging_root)
        .expect("stage opens")
        .stage_bytes("sketch-1.brep", bytes)
        .expect("worker bytes stage");
    let worker_fingerprint = WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: "threeterm.workers.occt/1".to_string(),
        protocol_schema_version: schema_version().to_string(),
    };
    Box::new(ArtifactHeader {
        request_id: "req-1".to_string(),
        source_revision_id: "rev-0".to_string(),
        cache_key: Layer1CacheKey {
            source_revision_id: "rev-0".to_string(),
            worker_fingerprint: worker_fingerprint.clone(),
            artifact_kind: "brep".to_string(),
            semantic_input_sha256: "11".repeat(32),
            deterministic_settings_sha256: "22".repeat(32),
        },
        worker_fingerprint,
        artifact_kind: "brep".to_string(),
        staging_name: staged.staging_name,
        byte_count: staged.byte_count,
        sha256,
    })
}

/// `PipeHost` is the production-style wiring: it pipes the host's
/// envelopes through `encode_frame` and the worker's envelopes through
/// `FrameParser::push` so the wire format is exercised on every send and
/// receive. This is the same code path a subprocess-backed `WorkerHost`
/// implementation will follow.
struct PipeHost {
    parser: FrameParser,
    /// Pending encoded frames from the worker; each `recv` pops the
    /// next frame, pushes it through the parser, and returns the
    /// envelope. The parser drains the frame on each push, so this
    /// round-trips the wire format end-to-end.
    outbound: VecDeque<Vec<u8>>,
}

impl PipeHost {
    fn new(script: Vec<Envelope>) -> Self {
        let outbound = script
            .into_iter()
            .map(|envelope| encode_frame(&envelope).expect("script encodes"))
            .collect::<VecDeque<_>>();
        Self {
            parser: FrameParser::new(),
            outbound,
        }
    }
}

impl WorkerHost for PipeHost {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        encode_frame(envelope)
            .map(|_| ())
            .map_err(|error| WorkerError::Protocol(format!("encode_frame failed: {error}")))
    }

    fn recv(&mut self, _deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        let frame = self.outbound.pop_front().ok_or(WorkerError::Closed)?;
        let envelopes = self
            .parser
            .push(&frame)
            .map_err(|error| WorkerError::Protocol(format!("{error}")))?;
        envelope_or_closed(envelopes)
    }

    fn cancel(&mut self, _request_id: &str, _reason: &str) -> Result<(), WorkerError> {
        Ok(())
    }
}

fn envelope_or_closed(envelopes: Vec<Envelope>) -> Result<Envelope, WorkerError> {
    envelopes.into_iter().next().ok_or(WorkerError::Closed)
}

#[test]
fn envelope_round_trip_through_frame_parser() {
    let envelope = Envelope::Cancelled {
        schema_version: schema_version().to_string(),
        request_id: "req-1".to_string(),
        reason: "user pressed stop".to_string(),
    };
    assert_eq!(round_trip(&envelope), envelope);
}

#[test]
fn cooperative_cancellation_returns_structured_acknowledgement() {
    // The fake serves exactly one Cancelled envelope; the supervisor
    // sends a cooperative Cancel and observes the Cancelled ack inside
    // the grace period.
    let cancelled = Envelope::Cancelled {
        schema_version: schema_version().to_string(),
        request_id: "req-1".to_string(),
        reason: "user pressed stop".to_string(),
    };
    let worker = PipeHost::new(vec![cancelled]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let outcome = supervisor.cancel("req-1", "user pressed stop");

    let SupervisorOutcome::Acknowledged {
        request_id,
        reason,
        elapsed: _,
    } = outcome
    else {
        panic!("expected Acknowledged; got {outcome:?}");
    };
    assert_eq!(request_id, "req-1");
    assert_eq!(reason, "user pressed stop");
}

#[test]
fn cancellation_preserves_the_last_progress_context_on_force_termination() {
    let worker = FakeWorker::new(vec![Envelope::Progress {
        schema_version: schema_version().to_string(),
        request_id: "req-1".to_string(),
        stage: "writing staged output".to_string(),
        percent: 72,
    }]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.cancel("req-1", "stop") else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    let progress = record
        .last_progress
        .expect("cancellation termination must retain the last progress");
    assert_eq!(progress.stage, "writing staged output");
    assert_eq!(progress.percent, 72);
}

#[test]
fn request_consumes_worker_ready_handshake_then_returns_staged_facts() {
    // Worker handshake + a valid staged artifact + Completed. The
    // supervisor must consume WorkerReady and return the staged artifact
    // fact on Completed without publishing it.
    let staging_root = std::env::temp_dir().join(format!(
        "threeterm-pipe-stage-promote-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging_root);
    let stage = Stage::open(&staging_root).expect("stage opens");

    let bytes = b"hello, worker";
    let sha = sha256_hex(bytes);

    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Artifact {
            schema_version: schema_version().to_string(),
            header: artifact_header(&staging_root, bytes, sha.clone()),
        },
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), Some(stage));

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::Completed {
        request_id,
        artifact_headers,
        ..
    } = outcome
    else {
        panic!("expected Completed; got {outcome:?}");
    };
    assert_eq!(request_id, "req-1");
    assert_eq!(artifact_headers.len(), 1);

    // Publication is exclusively a Host acceptance concern.
    let final_path = staging_root.join("sketch-1.brep");
    assert!(
        !final_path.exists(),
        "supervisor must not publish artifacts"
    );
    assert!(
        staging_root.join("sketch-1.brep.partial").exists(),
        "host must receive the staged artifact for acceptance"
    );

    let _ = std::fs::remove_dir_all(&staging_root);
}

#[test]
fn request_force_terminate_after_grace_emits_structured_termination_record() {
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Progress {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            stage: "tessellating".to_string(),
            percent: 50,
        },
        Envelope::Progress {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            stage: "almost done".to_string(),
            percent: 95,
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert!(
        record.stage.starts_with("grace_exceeded") || record.stage.starts_with("worker_closed"),
        "force-terminate stage should be grace_exceeded or worker_closed; got {:?}",
        record.stage
    );
    let progress = record
        .last_progress
        .expect("supervisor tracks the most recent progress before force-terminate");
    assert_eq!(progress.stage, "almost done");
    assert_eq!(progress.percent, 95);
}

#[test]
fn request_force_terminates_with_no_progress_when_worker_closes_immediately() {
    // Worker sends only the WorkerReady handshake and then closes
    // without emitting a terminal envelope.
    let worker = PipeHost::new(vec![ready_envelope()]);
    let mut supervisor = Supervisor::new(Duration::from_millis(1), Box::new(worker), None);

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert!(
        record.stage.starts_with("worker_closed") || record.stage.starts_with("grace_exceeded"),
        "force-terminate stage should be worker_closed or grace_exceeded; got {:?}",
        record.stage
    );
    assert!(
        record.last_progress.is_none(),
        "no progress envelopes were sent, so last_progress must be None; got {:?}",
        record.last_progress
    );
}

#[test]
fn request_force_terminates_when_worker_never_sends_worker_ready() {
    // Worker closes before sending a WorkerReady handshake. The
    // supervisor must fail closed without sending a Request.
    let worker = PipeHost::new(Vec::new());
    let mut supervisor = Supervisor::new(Duration::from_millis(1), Box::new(worker), None);

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.request_id, "<handshake>");
    assert!(
        record.stage.starts_with("handshake_worker_closed")
            || record.stage.starts_with("handshake_grace_exceeded"),
        "expected handshake_worker_closed or handshake_grace_exceeded; got {:?}",
        record.stage
    );
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
}

#[test]
fn request_force_terminates_when_worker_ready_uses_mismatched_schema_version() {
    let worker = PipeHost::new(vec![Envelope::WorkerReady {
        schema_version: "threeterm.protocol/0".to_string(),
        worker_id: "fake".to_string(),
    }]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert!(
        record.stage.starts_with("handshake_schema_mismatch"),
        "expected handshake_schema_mismatch; got {:?}",
        record.stage
    );
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
}

#[test]
fn request_fails_closed_on_unsolicited_cancelled_envelope() {
    // Worker sends WorkerReady + Cancelled without the host ever
    // sending a Cancel. The supervisor must NOT classify this as a
    // cooperative ack; it must fail the request closed immediately.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Cancelled {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            reason: "spurious".to_string(),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert!(
        record
            .stage
            .starts_with("protocol_violation:unsolicited_cancelled:"),
        "expected protocol_violation:unsolicited_cancelled:; got {:?}",
        record.stage
    );
}

#[test]
fn request_returns_unvalidated_staged_artifact_facts_to_host() {
    // Worker sends WorkerReady + an Artifact whose advertised hash is
    // wrong. The supervisor must not validate or promote it; the Host
    // receives the fact and decides whether to reject and clean it up.
    let staging_root = std::env::temp_dir().join(format!(
        "threeterm-pipe-stage-mismatch-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging_root);
    let stage = Stage::open(&staging_root).expect("stage opens");

    let bytes = b"hello, worker";

    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Artifact {
            schema_version: schema_version().to_string(),
            header: artifact_header(&staging_root, bytes, "deadbeef".to_string()),
        },
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), Some(stage));

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::Completed {
        artifact_headers, ..
    } = outcome
    else {
        panic!("expected Completed; got {outcome:?}");
    };
    assert_eq!(artifact_headers.len(), 1);

    // No final file exists before Host acceptance, and the staged file is
    // available for the Host's independent digest validation.
    assert!(
        staging_root.join("sketch-1.brep.partial").exists(),
        "host acceptance must receive the staged artifact"
    );
    assert!(
        !staging_root.join("sketch-1.brep").exists(),
        "rejected artifacts must never be promoted to the final path"
    );

    let _ = std::fs::remove_dir_all(&staging_root);
}

#[test]
fn termination_record_carries_elapsed_duration_and_request_id() {
    let worker = PipeHost::new(Vec::new());
    let mut supervisor = Supervisor::new(Duration::from_millis(1), Box::new(worker), None);

    let outcome = supervisor.request(sample_request());
    let TerminationRecord {
        request_id,
        stage,
        elapsed,
        last_progress,
        last_artifact_error,
        exit_signal,
        exit_code,
        stderr_tail,
        failed_code,
        failed_detail,
        exit_kind,
    } = match outcome {
        SupervisorOutcome::ForceTerminated { record } => record,
        other => panic!("expected ForceTerminated; got {other:?}"),
    };

    assert_eq!(request_id, "<handshake>");
    assert_eq!(exit_signal, None);
    assert_eq!(exit_code, None);
    assert_eq!(stderr_tail, "");
    assert_eq!(failed_code, None);
    assert_eq!(failed_detail, None);
    assert_eq!(exit_kind, ExitKind::ForceAfterGrace);
    assert!(last_progress.is_none());
    assert!(last_artifact_error.is_none());
    assert!(
        !stage.is_empty(),
        "termination stage label must not be empty"
    );
    assert!(
        elapsed <= Duration::from_secs(5),
        "elapsed must not exceed a sane upper bound; got {elapsed:?}"
    );
}

#[test]
fn request_sends_a_well_formed_request_envelope() {
    // Captures the bytes the supervisor emitted via `send` so the test
    // asserts the wire format matches the canonical envelope. The
    // WorkerReady handshake is supplied first; after the supervisor
    // consumes it the Request envelope is emitted.
    let received = Arc::new(Mutex::new(Vec::<Envelope>::new()));
    let captured = Arc::clone(&received);
    let inner = FakeWorker::new(vec![ready_envelope()]);
    let capturing_worker = CapturingFake { inner, captured };
    let mut supervisor =
        Supervisor::new(Duration::from_millis(50), Box::new(capturing_worker), None);

    let outcome = supervisor.request(sample_request());
    assert!(
        matches!(outcome, SupervisorOutcome::ForceTerminated { .. }),
        "expected ForceTerminated; got {outcome:?}"
    );

    let captured = received.lock().expect("capture mutex");
    assert_eq!(captured.len(), 1, "host emitted exactly one envelope");
    match &captured[0] {
        Envelope::Request {
            request_id,
            command_id,
            revision_id,
            schema_version: envelope_schema_version,
            ..
        } => {
            assert_eq!(request_id, "req-1");
            assert_eq!(command_id, "list");
            assert_eq!(revision_id, "rev-0");
            assert_eq!(envelope_schema_version, schema_version());
        }
        other => panic!("expected Request envelope; got {other:?}"),
    }
}

struct CapturingFake {
    inner: FakeWorker,
    captured: Arc<Mutex<Vec<Envelope>>>,
}

impl WorkerHost for CapturingFake {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        self.captured
            .lock()
            .expect("capture mutex")
            .push(envelope.clone());
        self.inner.send(envelope)
    }

    fn recv(&mut self, deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        self.inner.recv(deadline)
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.inner.cancel(request_id, reason)
    }
}

#[test]
fn cancel_invokes_worker_cancel_exactly_once_on_the_cooperative_ack_path() {
    // On the cooperative ack path the supervisor sends the cooperative
    // `Cancel` envelope once and observes the `Cancelled` ack inside the
    // grace period. The fake's `cancel` log therefore records exactly
    // one entry — the cooperative cancel — with no follow-up force
    // terminate cancel.
    let cancel_log: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let cancelled = Envelope::Cancelled {
        schema_version: schema_version().to_string(),
        request_id: "req-1".to_string(),
        reason: "ok".to_string(),
    };
    let inner = FakeWorker::new(vec![cancelled]);
    let worker = CancelLoggingWorker {
        inner,
        log: Arc::clone(&cancel_log),
    };
    let mut supervisor = Supervisor::new(Duration::from_millis(50), Box::new(worker), None);

    let outcome = supervisor.cancel("req-1", "user requested stop");
    assert!(
        matches!(outcome, SupervisorOutcome::Acknowledged { .. }),
        "expected Acknowledged; got {outcome:?}"
    );
    let log = cancel_log.lock().expect("cancel log mutex");
    assert_eq!(
        log.len(),
        1,
        "cooperative ack path must call worker.cancel exactly once; got {log:?}"
    );
    assert_eq!(log[0].0, "req-1");
    assert_eq!(log[0].1, "user requested stop");
}

/// Wraps a `FakeWorker` and forwards `cancel` calls into a shared log so
/// the test can inspect them after the supervisor returns.
struct CancelLoggingWorker {
    inner: FakeWorker,
    log: Arc<Mutex<Vec<(String, String)>>>,
}

impl WorkerHost for CancelLoggingWorker {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        self.inner.send(envelope)
    }

    fn recv(&mut self, deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        self.inner.recv(deadline)
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.log
            .lock()
            .expect("cancel log mutex")
            .push((request_id.to_string(), reason.to_string()));
        self.inner.cancel(request_id, reason)
    }
}

#[test]
fn request_rejects_an_artifact_bound_to_another_revision() {
    let staging_root = std::env::temp_dir().join(format!(
        "threeterm-pipe-stage-stale-revision-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging_root);
    let stage = Stage::open(&staging_root).expect("stage opens");
    let bytes = b"stale worker bytes";
    let mut header = artifact_header(&staging_root, bytes, sha256_hex(bytes));
    header.source_revision_id = "rev-stale".to_string();
    header.cache_key.source_revision_id = "rev-stale".to_string();
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Artifact {
            schema_version: schema_version().to_string(),
            header,
        },
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), Some(stage));

    let SupervisorOutcome::Completed {
        artifact_headers, ..
    } = supervisor.request(sample_request())
    else {
        panic!("expected completed facts");
    };

    assert_eq!(artifact_headers.len(), 1);
    assert!(staging_root.join("sketch-1.brep.partial").exists());
    assert!(!staging_root.join("sketch-1.brep").exists());
    let _ = std::fs::remove_dir_all(staging_root);
}

/// Wraps a `PipeHost` and reports a scripted exit signal and stderr
/// tail, simulating a worker that was killed by a signal while emitting
/// diagnostics.
struct SignalReportingWorker {
    inner: PipeHost,
    signal: Option<i32>,
    stderr: String,
}

impl SignalReportingWorker {
    fn new(script: Vec<Envelope>, signal: Option<i32>, stderr: &str) -> Self {
        Self {
            inner: PipeHost::new(script),
            signal,
            stderr: stderr.to_string(),
        }
    }
}

impl WorkerHost for SignalReportingWorker {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        self.inner.send(envelope)
    }

    fn recv(&mut self, deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        self.inner.recv(deadline)
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.inner.cancel(request_id, reason)
    }

    fn exit_signal(&mut self) -> Option<i32> {
        self.signal
    }

    fn stderr_tail(&mut self) -> String {
        self.stderr.clone()
    }
}

#[test]
fn completed_carries_the_worker_result_value() {
    // The Completed envelope's `result` value must survive the
    // supervisor so the typed OCCT boundary can parse it.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "status": "ok", "brep_path": "/tmp/out.brep" }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::Completed { result, .. } = supervisor.request(sample_request()) else {
        panic!("expected Completed; got a terminal failure");
    };
    assert_eq!(result["status"], "ok");
    assert_eq!(result["brep_path"], "/tmp/out.brep");
}

#[test]
fn termination_record_carries_exit_signal_and_stderr_tail() {
    // A worker killed by a signal (e.g. SIGKILL after force termination)
    // must surface the actual signal and its bounded stderr tail in the
    // structured record.
    let worker = SignalReportingWorker::new(
        vec![
            ready_envelope(),
            Envelope::Progress {
                schema_version: schema_version().to_string(),
                request_id: "req-1".to_string(),
                stage: "computing".to_string(),
                percent: 50,
            },
        ],
        Some(9),
        "worker trace: overflow\n",
    );
    let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert_eq!(record.exit_signal, Some(9));
    assert_eq!(record.stderr_tail, "worker trace: overflow\n");
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
}

#[test]
fn failed_envelope_populates_structured_failure_fields() {
    // A cooperative Failed envelope must surface code and detail
    // structurally instead of relying on the free-form stage string.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Failed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            code: "brep_invalid".to_string(),
            detail: "BRepCheck_Analyzer failed".to_string(),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert_eq!(record.exit_kind, ExitKind::Cooperative);
    assert_eq!(record.failed_code.as_deref(), Some("brep_invalid"));
    assert_eq!(
        record.failed_detail.as_deref(),
        Some("BRepCheck_Analyzer failed")
    );
    assert!(
        record.stage.starts_with("failed:"),
        "stage: {:?}",
        record.stage
    );
}

#[test]
fn progress_bound_to_another_request_is_rejected() {
    // A Progress envelope for a foreign request must not be accepted as
    // progress for the active request; it is a protocol violation.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Progress {
            schema_version: schema_version().to_string(),
            request_id: "other-request".to_string(),
            stage: "sneaky".to_string(),
            percent: 99,
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    let progress = record
        .last_progress
        .expect("mismatched progress must surface as a protocol violation");
    assert!(
        progress
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        progress.stage
    );
}

#[test]
fn artifact_bound_to_another_request_is_rejected() {
    // An Artifact envelope whose header names a foreign request must not
    // be recorded as a staged artifact for the active request.
    let staging_root = std::env::temp_dir().join(format!(
        "threeterm-pipe-stage-foreign-request-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging_root);
    let stage = Stage::open(&staging_root).expect("stage opens");

    let bytes = b"foreign worker bytes";
    let mut header = artifact_header(&staging_root, bytes, sha256_hex(bytes));
    header.request_id = "other-request".to_string();
    // The worker sends only the mismatched artifact and never completes;
    // the violation must surface and the grace period must terminate it.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Artifact {
            schema_version: schema_version().to_string(),
            header,
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), Some(stage));

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    let progress = record
        .last_progress
        .expect("mismatched artifact must surface as a protocol violation");
    assert!(
        progress
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        progress.stage
    );
    assert!(
        !staging_root.join("sketch-1.brep.partial").exists(),
        "the foreign artifact must never be staged"
    );
    let _ = std::fs::remove_dir_all(staging_root);
}

#[test]
fn failed_envelope_bound_to_another_request_is_rejected() {
    // A Failed envelope for a foreign request must not be accepted as
    // the terminal failure of the active request.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Failed {
            schema_version: schema_version().to_string(),
            request_id: "other-request".to_string(),
            code: "brep_invalid".to_string(),
            detail: "not this request".to_string(),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert_eq!(
        record.failed_code, None,
        "foreign failure must not be adopted"
    );
    let progress = record
        .last_progress
        .expect("mismatched failure must surface as a protocol violation");
    assert!(
        progress
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        progress.stage
    );
}

#[test]
fn request_fails_closed_on_post_handshake_schema_mismatch() {
    // WorkerReady with the canonical schema, then a Progress envelope
    // carrying a foreign protocol version: the request must fail closed
    // instead of accepting the mis-versioned message.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Progress {
            schema_version: "threeterm.protocol/0".to_string(),
            request_id: "req-1".to_string(),
            stage: "sneaky".to_string(),
            percent: 1,
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert!(
        record.stage.starts_with("envelope_schema_mismatch"),
        "stage: {:?}",
        record.stage
    );
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
}

#[test]
fn cancel_fails_closed_on_post_handshake_schema_mismatch() {
    let worker = PipeHost::new(vec![Envelope::Progress {
        schema_version: "threeterm.protocol/0".to_string(),
        request_id: "req-1".to_string(),
        stage: "sneaky".to_string(),
        percent: 1,
    }]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.cancel("req-1", "stop") else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert!(
        record.stage.starts_with("envelope_schema_mismatch"),
        "stage: {:?}",
        record.stage
    );
}

#[test]
fn mismatched_progress_fails_the_request_closed_immediately() {
    // A foreign-request Progress must terminate the request immediately
    // (not merely be marked and continue until grace).
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Progress {
            schema_version: schema_version().to_string(),
            request_id: "other-request".to_string(),
            stage: "sneaky".to_string(),
            percent: 99,
        },
        // Even a valid Completed afterwards must not rescue the request.
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert!(
        record
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        record.stage
    );
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
}

#[test]
fn request_with_cancel_acknowledges_a_cooperative_worker() {
    // The request loop hands over to the cancellation lifecycle when
    // the cancel flag is set; a worker that acks yields Acknowledged.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Cancelled {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            reason: "cancelled by host".to_string(),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);
    let cancel = std::sync::atomic::AtomicBool::new(true);

    let outcome = supervisor.request_with_cancel(sample_request(), &cancel);
    let SupervisorOutcome::Acknowledged {
        request_id, reason, ..
    } = outcome
    else {
        panic!("expected Acknowledged; got {outcome:?}");
    };
    assert_eq!(request_id, "req-1");
    assert_eq!(reason, "cancelled by host");
}

#[test]
fn request_with_cancel_force_terminates_a_worker_that_never_acks() {
    // The worker never acks the cooperative Cancel; the cancellation
    // lifecycle force-terminates after the grace period.
    let worker = PipeHost::new(vec![ready_envelope()]);
    let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);
    let cancel = std::sync::atomic::AtomicBool::new(true);

    let outcome = supervisor.request_with_cancel(sample_request(), &cancel);
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert!(
        record.stage.starts_with("cancel_") || record.stage.starts_with("worker_closed"),
        "stage: {:?}",
        record.stage
    );
}

#[test]
fn cancel_fails_closed_on_a_foreign_acknowledgement() {
    let worker = PipeHost::new(vec![Envelope::Cancelled {
        schema_version: schema_version().to_string(),
        request_id: "other-request".to_string(),
        reason: "wrong request".to_string(),
    }]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.cancel("req-1", "stop") else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert!(
        record
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        record.stage
    );
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
}

#[test]
fn cancel_fails_closed_on_a_terminal_envelope_instead_of_an_acknowledgement() {
    // A worker that completes instead of acknowledging the cancellation
    // is a protocol violation: the cancellation must fail closed rather
    // than treating the Completed envelope as an acknowledgement.
    let worker = PipeHost::new(vec![Envelope::Completed {
        schema_version: schema_version().to_string(),
        request_id: "req-1".to_string(),
        result: serde_json::json!({ "ok": true }),
    }]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.cancel("req-1", "stop") else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert_eq!(record.stage, "protocol_violation:expected_cancelled_ack");
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
}

/// Wraps a `PipeHost` and reports a scripted exit code and signal,
/// simulating a worker whose reaped status is not clean.
struct ExitStatusWorker {
    inner: PipeHost,
    signal: Option<i32>,
    code: Option<i32>,
}

impl ExitStatusWorker {
    fn new(script: Vec<Envelope>, signal: Option<i32>, code: Option<i32>) -> Self {
        Self {
            inner: PipeHost::new(script),
            signal,
            code,
        }
    }
}

impl WorkerHost for ExitStatusWorker {
    fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
        self.inner.send(envelope)
    }

    fn recv(&mut self, deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        self.inner.recv(deadline)
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.inner.cancel(request_id, reason)
    }

    fn exit_signal(&mut self) -> Option<i32> {
        self.signal
    }

    fn exit_code(&mut self) -> Option<i32> {
        self.code
    }
}

#[test]
fn completed_after_nonzero_worker_exit_is_accepted() {
    // A terminal envelope is authoritative; the worker's exit code remains
    // diagnostic input rather than becoming a completion gate.
    let worker = ExitStatusWorker::new(
        vec![
            ready_envelope(),
            Envelope::Completed {
                schema_version: schema_version().to_string(),
                request_id: "req-1".to_string(),
                result: serde_json::json!({ "ok": true }),
            },
        ],
        None,
        Some(7),
    );
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    assert!(matches!(
        supervisor.request(sample_request()),
        SupervisorOutcome::Completed { .. }
    ));
}

#[test]
fn acknowledged_after_signal_exit_is_accepted() {
    // A cancellation acknowledgement is authoritative; the worker's signal
    // remains diagnostic input rather than becoming a completion gate.
    let worker = ExitStatusWorker::new(
        vec![Envelope::Cancelled {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            reason: "ok".to_string(),
        }],
        Some(11),
        None,
    );
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    assert!(matches!(
        supervisor.cancel("req-1", "stop"),
        SupervisorOutcome::Acknowledged { .. }
    ));
}

#[test]
fn request_fails_closed_on_a_host_only_envelope_followed_by_completion() {
    // A worker that sends a host-only Request envelope and then a valid
    // Completed must NOT succeed: the protocol violation fails the
    // request closed.
    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Request {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            command_id: "extrude".to_string(),
            args: serde_json::json!({}),
            revision_id: "".to_string(),
        },
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request()) else {
        panic!("expected ForceTerminated; got non-terminal outcome");
    };
    assert_eq!(
        record.stage, "protocol_violation:worker_sent_host_only_envelope",
        "the host-only envelope must fail the request closed, not be ignored"
    );
}

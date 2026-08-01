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

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use threeterm_protocol::artifact::{Stage, sha256_hex};
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

    fn recv(&mut self) -> Result<Envelope, WorkerError> {
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

    fn recv(&mut self) -> Result<Envelope, WorkerError> {
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
fn request_consumes_worker_ready_handshake_then_promotes_completed() {
    // Worker handshake + a valid staged artifact + Completed. The
    // supervisor must consume WorkerReady, stage the artifact,
    // promote it on Completed, and report a Cooperative terminal
    // record.
    let staging_root = std::env::temp_dir().join(format!(
        "threeterm-pipe-stage-promote-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging_root);
    let stage = Stage::open(&staging_root).expect("stage opens");

    let bytes = b"hello, worker";
    let sha = sha256_hex(bytes);
    let encoded = BASE64.encode(bytes);

    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Artifact {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            artifact_kind: "brep".to_string(),
            staging_name: "sketch-1.brep".to_string(),
            sha256: sha.clone(),
            bytes_b64: encoded.clone(),
        },
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), Some(stage));

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated(Completed); got {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert_eq!(record.exit_kind, ExitKind::Cooperative);
    assert_eq!(record.stage, "completed");
    assert!(record.last_artifact_error.is_none());

    // The artifact's .partial file must have been promoted to its final
    // filename with the validated bytes on disk.
    let final_path = staging_root.join("sketch-1.brep");
    let promoted = std::fs::read(&final_path).expect("promoted file reads");
    assert_eq!(
        promoted, bytes,
        "promoted bytes must match the worker payload"
    );
    assert!(
        !staging_root.join("sketch-1.brep.partial").exists(),
        "partial must be gone after promotion"
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
fn request_records_protocol_violation_on_unsolicited_cancelled_envelope() {
    // Worker sends WorkerReady + Cancelled without the host ever
    // sending a Cancel. The supervisor must NOT classify this as a
    // cooperative ack; it must surface the violation via
    // `last_progress` and continue until grace expires.
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
    let progress = record
        .last_progress
        .expect("unsolicited Cancelled must surface in last_progress");
    assert!(
        progress
            .stage
            .starts_with("protocol_violation:unsolicited_cancelled:"),
        "expected protocol_violation:unsolicited_cancelled:; got {:?}",
        progress.stage
    );
}

#[test]
fn request_surfaces_staging_failures_in_last_artifact_error() {
    // Worker sends WorkerReady + an Artifact whose advertised hash is
    // wrong. The supervisor must NOT promote the artifact, must
    // surface the `HashMismatch` error in the terminal record, and
    // must preserve the canonical host state by discarding the
    // rejected .partial file.
    let staging_root = std::env::temp_dir().join(format!(
        "threeterm-pipe-stage-mismatch-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging_root);
    let stage = Stage::open(&staging_root).expect("stage opens");

    let bytes = b"hello, worker";
    let encoded = BASE64.encode(bytes);

    let worker = PipeHost::new(vec![
        ready_envelope(),
        Envelope::Artifact {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            artifact_kind: "brep".to_string(),
            staging_name: "sketch-1.brep".to_string(),
            sha256: "deadbeef".to_string(),
            bytes_b64: encoded,
        },
        Envelope::Completed {
            schema_version: schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        },
    ]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), Some(stage));

    let outcome = supervisor.request(sample_request());

    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated(Completed); got {outcome:?}");
    };
    assert_eq!(record.exit_kind, ExitKind::Cooperative);
    let error = record
        .last_artifact_error
        .expect("staging failure must surface in last_artifact_error");
    assert!(
        error.contains("hash mismatch") || error.contains("HashMismatch"),
        "expected a hash-mismatch diagnostic; got {error:?}"
    );

    // Canonical host state preserved: no .partial, no final file.
    assert!(
        !staging_root.join("sketch-1.brep.partial").exists(),
        "rejected artifacts must not leave a partial behind"
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
        exit_kind,
    } = match outcome {
        SupervisorOutcome::ForceTerminated { record } => record,
        other => panic!("expected ForceTerminated; got {other:?}"),
    };

    assert_eq!(request_id, "<handshake>");
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

    fn recv(&mut self) -> Result<Envelope, WorkerError> {
        self.inner.recv()
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

    fn recv(&mut self) -> Result<Envelope, WorkerError> {
        self.inner.recv()
    }

    fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
        self.log
            .lock()
            .expect("cancel log mutex")
            .push((request_id.to_string(), reason.to_string()));
        self.inner.cancel(request_id, reason)
    }
}

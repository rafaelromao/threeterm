//! End-to-end integration test for the worker protocol round-trip and
//! cooperative cancellation lifecycle.
//!
//! Wires the production `Supervisor` against a fake in-process
//! `WorkerHost` (over the same trait the production `WorkerProcess`
//! adapter will use) and asserts both the cooperative-cancel-acks path
//! and the force-terminate-after-grace path produce structured records.
//! The fake worker is fully synchronous so CI is deterministic.

use std::collections::VecDeque;
use std::time::Duration;

use threeterm_protocol::schema_version;
use threeterm_protocol::supervisor::{
    ExitKind, Request, Supervisor, SupervisorOutcome, TerminationRecord,
};
use threeterm_protocol::worker::{Envelope, WorkerError, WorkerHost};

/// In-process `WorkerHost` used by the integration tests. The fake serves
/// envelopes from a scripted queue, records every envelope received from
/// the host via `send`, and counts `cancel` calls.
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

#[test]
fn cooperative_cancellation_returns_structured_acknowledgement() {
    let envelope = Envelope::Cancelled {
        schema_version: schema_version().to_string(),
        request_id: "req-1".to_string(),
        reason: "user pressed stop".to_string(),
    };
    let worker = FakeWorker::new(vec![envelope.clone()]);
    let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

    let outcome = supervisor.run(sample_request());

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
fn force_terminate_after_grace_emits_structured_termination_record() {
    // The fake serves two progress envelopes, then Closed. The grace
    // period is long enough to consume both Progress envelopes; once
    // the script is exhausted the supervisor records `worker_closed`
    // (or `grace_exceeded`, depending on the order of the deadline
    // check) and emits a structured `TerminationRecord` carrying the
    // most recent progress.
    let worker = FakeWorker::new(vec![
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

    let outcome = supervisor.run(sample_request());

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
fn force_terminate_emits_record_with_no_progress_when_worker_closes_immediately() {
    let worker = FakeWorker::new(Vec::new());
    let mut supervisor = Supervisor::new(Duration::from_millis(1), Box::new(worker), None);

    let outcome = supervisor.run(sample_request());

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
fn termination_record_carries_elapsed_duration_and_request_id() {
    let worker = FakeWorker::new(Vec::new());
    let mut supervisor = Supervisor::new(Duration::from_millis(1), Box::new(worker), None);

    let outcome = supervisor.run(sample_request());
    let TerminationRecord {
        request_id,
        stage,
        elapsed,
        last_progress,
        exit_kind,
    } = match outcome {
        SupervisorOutcome::ForceTerminated { record } => record,
        other => panic!("expected ForceTerminated; got {other:?}"),
    };

    assert_eq!(request_id, "req-1");
    assert_eq!(exit_kind, ExitKind::ForceAfterGrace);
    assert!(last_progress.is_none());
    assert!(
        !stage.is_empty(),
        "termination stage label must not be empty"
    );
    // Duration is monotonic; allow zero for very fast test runs.
    assert!(
        elapsed <= Duration::from_secs(5),
        "elapsed must not exceed a sane upper bound; got {elapsed:?}"
    );
}

#[test]
fn supervisor_records_every_envelope_the_host_emitted() {
    // After the supervisor finishes, the fake's `received` log should
    // contain the Request envelope the supervisor forwarded to the
    // worker. We capture it by having the fake capture every envelope
    // it sees on its `send` side.
    struct CapturingFake {
        captured: std::sync::Arc<std::sync::Mutex<Vec<Envelope>>>,
        cancel_calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
        script: VecDeque<Envelope>,
    }
    impl WorkerHost for CapturingFake {
        fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
            self.captured
                .lock()
                .expect("capture mutex")
                .push(envelope.clone());
            Ok(())
        }
        fn recv(&mut self) -> Result<Envelope, WorkerError> {
            self.script.pop_front().ok_or(WorkerError::Closed)
        }
        fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
            self.cancel_calls
                .lock()
                .expect("cancel mutex")
                .push((request_id.to_string(), reason.to_string()));
            Ok(())
        }
    }

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancels = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let script = vec![Envelope::Cancelled {
        schema_version: schema_version().to_string(),
        request_id: "req-1".to_string(),
        reason: "ok".to_string(),
    }];
    let worker = CapturingFake {
        captured: std::sync::Arc::clone(&captured),
        cancel_calls: std::sync::Arc::clone(&cancels),
        script: script.into(),
    };
    let mut supervisor = Supervisor::new(Duration::from_millis(50), Box::new(worker), None);

    let outcome = supervisor.run(sample_request());
    assert!(
        matches!(outcome, SupervisorOutcome::Acknowledged { .. }),
        "expected Acknowledged; got {outcome:?}"
    );

    let sent = captured.lock().expect("capture mutex");
    assert_eq!(sent.len(), 1, "host emitted exactly one envelope");
    match &sent[0] {
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
    drop(sent);

    let cancel_log = cancels.lock().expect("cancel mutex");
    assert!(
        cancel_log.is_empty(),
        "cooperative ack path must not call cancel; got {cancel_log:?}"
    );
}

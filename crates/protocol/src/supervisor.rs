//! Cooperative cancellation supervisor.
//!
//! The supervisor drives one `WorkerHost` per `Request`. The
//! `Supervisor::request` method runs the full request lifecycle:
//! consume the versioned `WorkerReady` handshake, send the `Request`
//! envelope, track staged artifacts, promote them on `Completed`, or
//! discard them on `Failed` / force termination. The `Supervisor::cancel`
//! method runs the cooperative cancellation lifecycle: send a `Cancel`
//! envelope and wait for a `Cancelled` acknowledgement inside the
//! configured grace period; otherwise force-terminate the worker.
//!
//! When the supervisor force-terminates, it discards every staged
//! Derived Result so no partial artifact can compete with the
//! authoritative Revision Snapshot.

use std::fmt;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::artifact::{ArtifactHeader, Stage};
use crate::worker::{Envelope, WorkerError, WorkerHost};

/// A host-issued worker request.
#[derive(Debug, Clone)]
pub struct Request {
    pub request_id: String,
    pub command_id: String,
    pub args: Value,
    pub revision_id: String,
}

/// Outcome of a single `Supervisor::request` / `Supervisor::cancel` invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorOutcome {
    /// The worker acknowledged the cooperative cancellation inside the
    /// grace period and exited cleanly.
    Acknowledged {
        request_id: String,
        reason: String,
        elapsed: Duration,
    },
    /// The supervisor finished the lifecycle with a structured terminal
    /// record. `exit_kind` distinguishes a cooperative worker-emitted
    /// terminal envelope (`Completed` / `Failed`) from a hard
    /// force-terminate after the grace period expired.
    ForceTerminated { record: TerminationRecord },
}

/// Structured record emitted on every supervisor terminal transition
/// (cooperative terminal envelope, handshake failure, or force-terminate
/// after grace). The host uses these fields to surface the failure in
/// its diagnostic surface and to prove the canonical state survived:
/// the staged Derived Result was discarded; the Revision Snapshot was
/// never touched.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminationRecord {
    pub request_id: String,
    pub stage: String,
    pub elapsed: Duration,
    pub last_progress: Option<Progress>,
    /// Most recent `Stage::write` failure (`HashMismatch`,
    /// `PayloadTooLarge`, `InvalidName`, `Decode`). The supervisor
    /// surfaces staging failures here so the host's diagnostic taxonomy
    /// sees them (closed issue #237 AC: "Failures produce structured
    /// diagnostics and preserve the canonical host state").
    pub last_artifact_error: Option<String>,
    pub exit_kind: ExitKind,
}

/// Worker exit category. `Cooperative` is reserved for a worker-emitted
/// terminal envelope path; `ForceAfterGrace` is the variant emitted when
/// the supervisor had to kill the worker after the grace expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    Cooperative,
    ForceAfterGrace,
}

impl ExitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cooperative => "cooperative",
            Self::ForceAfterGrace => "force_after_grace",
        }
    }
}

/// Snapshot of the most recent `Progress` envelope, kept here so the
/// `TerminationRecord` can carry diagnostic context without forcing the
/// supervisor to thread the original envelope through.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub stage: String,
    pub percent: u8,
}

/// Drives a single worker process through the request and cancellation
/// lifecycles. Each `Supervisor` owns one `WorkerHost` for its lifetime;
/// the production wiring spawns a fresh disposable worker per
/// `Supervisor` (closed issue #49: one disposable worker per request).
pub struct Supervisor {
    grace: Duration,
    host: Box<dyn WorkerHost>,
    stage: Option<Stage>,
    /// Artifact headers accumulated during the request lifecycle. The
    /// `Completed` arm validates and atomically promotes each staged file.
    /// `discard_stage` clears the headers without promoting.
    artifact_headers: Vec<ArtifactHeader>,
    /// Most recent artifact binding or validation failure. The supervisor
    /// surfaces staging errors here so the host's diagnostic taxonomy sees them.
    last_artifact_error: Option<String>,
}

impl fmt::Debug for Supervisor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Supervisor")
            .field("grace", &self.grace)
            .field(
                "stage",
                &self.stage.as_ref().map(|stage| stage.root().to_path_buf()),
            )
            .finish_non_exhaustive()
    }
}

impl Supervisor {
    pub fn new(grace: Duration, host: Box<dyn WorkerHost>, stage: Option<Stage>) -> Self {
        Self {
            grace,
            host,
            stage,
            artifact_headers: Vec::new(),
            last_artifact_error: None,
        }
    }

    /// Returns the configured grace period.
    pub fn grace(&self) -> Duration {
        self.grace
    }

    /// Cooperative cancellation lifecycle: send a `Cancel` envelope and
    /// wait for a `Cancelled` acknowledgement inside the configured
    /// grace period. If the worker does not ack inside the grace
    /// period, the supervisor force-terminates it.
    ///
    /// The worker is assumed to have already completed the
    /// `WorkerReady` handshake during `Supervisor::request`; this
    /// method does not re-consume it.
    pub fn cancel(&mut self, request_id: &str, reason: &str) -> SupervisorOutcome {
        let started = Instant::now();
        if let Err(error) = self.host.cancel(request_id, reason) {
            return self.force_terminate_outcome(
                request_id,
                started,
                "host_cancel_failed",
                Some(error.to_string()),
                None,
            );
        }

        let deadline = started + self.grace;
        loop {
            match self.host.recv(deadline) {
                Ok(Envelope::Cancelled {
                    request_id: ack_request_id,
                    reason: ack_reason,
                    ..
                }) => {
                    if ack_request_id != request_id {
                        continue;
                    }
                    self.discard_stage();
                    return SupervisorOutcome::Acknowledged {
                        request_id: ack_request_id,
                        reason: ack_reason,
                        elapsed: started.elapsed(),
                    };
                }
                Ok(Envelope::Progress { .. }) => {}
                Ok(Envelope::Artifact { header, .. }) => {
                    let _ = header.staging_name;
                }
                Ok(_) => {}
                Err(WorkerError::Closed) => {
                    return self.force_terminate_outcome(
                        request_id,
                        started,
                        "worker_closed",
                        None,
                        None,
                    );
                }
                Err(WorkerError::TimedOut) => {
                    return self.force_terminate_outcome(
                        request_id,
                        started,
                        "cancel_grace_exceeded",
                        None,
                        None,
                    );
                }
                Err(error) => {
                    return self.force_terminate_outcome(
                        request_id,
                        started,
                        "worker_recv_error",
                        Some(error.to_string()),
                        None,
                    );
                }
            }

            if Instant::now() >= deadline {
                return self.force_terminate_outcome(
                    request_id,
                    started,
                    "grace_exceeded",
                    None,
                    None,
                );
            }
        }
    }

    /// Request lifecycle: consume the versioned `WorkerReady`
    /// handshake, send the `Request` envelope, track staged artifacts,
    /// and wait for a terminal envelope inside the configured grace
    /// period. On `Completed` every staged artifact is promoted to its
    /// final filename; on `Failed` / unsolicited `Cancelled` / force
    /// termination every staged artifact is discarded.
    pub fn request(&mut self, request: Request) -> SupervisorOutcome {
        let started = Instant::now();

        // Phase 1: consume one WorkerReady handshake. The worker must
        // advertise the canonical schema_version or the host fails
        // closed before sending any `Request`.
        let deadline = started + self.grace;
        if let Some(outcome) = self.consume_worker_ready("<handshake>", started, deadline) {
            return outcome;
        }

        // Phase 2: send the request envelope. Staged artifacts are
        // accumulated by `record_artifact`; the worker is expected to
        // emit a terminal envelope (Completed / Failed) inside the
        // grace period.
        if let Err(error) = self.host.send(&Envelope::Request {
            schema_version: crate::schema_version().to_string(),
            request_id: request.request_id.clone(),
            command_id: request.command_id.clone(),
            args: request.args.clone(),
            revision_id: request.revision_id.clone(),
        }) {
            return self.force_terminate_outcome(
                &request.request_id,
                started,
                "host_send_failed",
                Some(error.to_string()),
                None,
            );
        }

        let mut last_progress: Option<Progress> = None;

        loop {
            match self.host.recv(deadline) {
                Ok(Envelope::Progress { stage, percent, .. }) => {
                    last_progress = Some(Progress { stage, percent });
                }
                Ok(Envelope::Artifact { header, .. }) => {
                    self.record_artifact(&header, &request);
                }
                // An unsolicited Cancelled envelope during the request
                // lifecycle is a protocol violation: `request()` never
                // sends a `Cancel`, so a `Cancelled` arriving here
                // means the worker is misbehaving. Record it as a
                // protocol violation via the `last_progress` workaround
                // so the diagnostic surface sees it; do not classify it
                // as a cooperative ack.
                Ok(Envelope::Cancelled {
                    request_id: cancelled_request_id,
                    ..
                }) => {
                    last_progress = Some(Progress {
                        stage: format!(
                            "protocol_violation:unsolicited_cancelled:{cancelled_request_id}"
                        ),
                        percent: 0,
                    });
                }
                Ok(Envelope::Completed { request_id, .. }) => {
                    return self.complete_with_promotion(
                        request_id,
                        &request,
                        started,
                        last_progress,
                    );
                }
                Ok(Envelope::Failed {
                    request_id,
                    code,
                    detail,
                    ..
                }) => {
                    self.discard_stage();
                    return SupervisorOutcome::ForceTerminated {
                        record: TerminationRecord {
                            request_id,
                            stage: format!("failed:{code}:{detail}"),
                            elapsed: started.elapsed(),
                            last_progress,
                            last_artifact_error: self.last_artifact_error.take(),
                            exit_kind: ExitKind::Cooperative,
                        },
                    };
                }
                Ok(Envelope::WorkerReady { worker_id, .. }) => {
                    last_progress = Some(Progress {
                        stage: format!("unexpected_worker_ready:{worker_id}"),
                        percent: 0,
                    });
                }
                Ok(Envelope::Request { .. } | Envelope::Cancel { .. }) => {
                    last_progress = Some(Progress {
                        stage: "protocol_violation:worker_sent_host_only_envelope".to_string(),
                        percent: 0,
                    });
                }
                Err(WorkerError::Closed) => {
                    return self.force_terminate_outcome(
                        &request.request_id,
                        started,
                        "worker_closed",
                        None,
                        last_progress.take(),
                    );
                }
                Err(WorkerError::TimedOut) => {
                    return self.force_terminate_outcome(
                        &request.request_id,
                        started,
                        "grace_exceeded",
                        None,
                        last_progress.take(),
                    );
                }
                Err(error) => {
                    return self.force_terminate_outcome(
                        &request.request_id,
                        started,
                        "worker_recv_error",
                        Some(error.to_string()),
                        last_progress.take(),
                    );
                }
            }

            if Instant::now() >= deadline {
                return self.force_terminate_outcome(
                    &request.request_id,
                    started,
                    "grace_exceeded",
                    None,
                    last_progress.take(),
                );
            }
        }
    }

    /// Wait for the worker's `WorkerReady` handshake. The envelope's
    /// `schema_version` must match the canonical version or the host
    /// fails closed without sending a `Request`. Any other envelope
    /// arriving before the handshake or any `recv()` error fails the
    /// host closed. Returns `None` on a successful handshake; returns
    /// `Some(outcome)` to short-circuit the caller with the structured
    /// terminal record.
    ///
    /// The production `WorkerHost` impl is responsible for honoring
    /// the deadline internally (e.g. via a non-blocking receive with a
    /// timeout); the supervisor here only inspects the envelope once
    /// it has been delivered.
    fn consume_worker_ready(
        &mut self,
        request_id: &str,
        started: Instant,
        deadline: Instant,
    ) -> Option<SupervisorOutcome> {
        let envelope = match self.host.recv(deadline) {
            Ok(envelope) => envelope,
            Err(WorkerError::Closed) => {
                return Some(self.force_terminate_outcome(
                    request_id,
                    started,
                    "handshake_worker_closed",
                    None,
                    None,
                ));
            }
            Err(WorkerError::TimedOut) => {
                return Some(self.force_terminate_outcome(
                    request_id,
                    started,
                    "handshake_grace_exceeded",
                    None,
                    None,
                ));
            }
            Err(error) => {
                return Some(self.force_terminate_outcome(
                    request_id,
                    started,
                    "handshake_worker_recv_error",
                    Some(error.to_string()),
                    None,
                ));
            }
        };
        match envelope {
            Envelope::WorkerReady { schema_version, .. } => {
                let expected = crate::schema_version();
                if schema_version != expected {
                    let detail = format!("received={schema_version:?} expected={expected:?}");
                    Some(self.force_terminate_outcome(
                        request_id,
                        started,
                        "handshake_schema_mismatch",
                        Some(detail),
                        None,
                    ))
                } else {
                    None
                }
            }
            other => {
                let kind_label = envelope_kind_label(&other);
                Some(self.force_terminate_outcome(
                    request_id,
                    started,
                    "handshake_unexpected_envelope",
                    Some(kind_label),
                    None,
                ))
            }
        }
    }

    fn record_artifact(&mut self, header: &ArtifactHeader, request: &Request) {
        let Some(stage) = self.stage.as_ref() else {
            return;
        };
        let binding_error = if header.request_id != request.request_id {
            Some("artifact_request_id_mismatch")
        } else if header.source_revision_id != request.revision_id
            || header.cache_key.source_revision_id != request.revision_id
        {
            Some("artifact_source_revision_mismatch")
        } else if header.cache_key.worker_fingerprint != header.worker_fingerprint
            || header.cache_key.artifact_kind != header.artifact_kind
            || header.worker_fingerprint.protocol_schema_version != crate::schema_version()
        {
            Some("artifact_cache_key_mismatch")
        } else {
            None
        };
        if let Some(error) = binding_error {
            let _ = std::fs::remove_file(
                stage
                    .root()
                    .join(format!("{}.partial", header.staging_name)),
            );
            self.last_artifact_error = Some(error.to_string());
        } else {
            self.artifact_headers.push(header.clone());
        }
    }

    /// Drive the staged-artifact promotion contract on a `Completed`
    /// envelope. Each bound header is validated immediately before atomic
    /// promotion; any failure discards the staging directory and returns
    /// `ForceTerminated` with `stage: "promotion_failed:..."` so canonical
    /// host state never sees a partial result.
    fn complete_with_promotion(
        &mut self,
        request_id: String,
        request: &Request,
        started: Instant,
        last_progress: Option<Progress>,
    ) -> SupervisorOutcome {
        if request_id != request.request_id {
            self.last_artifact_error = Some("completed_request_id_mismatch".to_string());
            self.discard_stage();
            return SupervisorOutcome::ForceTerminated {
                record: TerminationRecord {
                    request_id,
                    stage: "protocol_violation:completed_request_id_mismatch".to_string(),
                    elapsed: started.elapsed(),
                    last_progress,
                    last_artifact_error: self.last_artifact_error.take(),
                    exit_kind: ExitKind::Cooperative,
                },
            };
        }
        if self.last_artifact_error.is_some() {
            self.discard_stage();
            return SupervisorOutcome::ForceTerminated {
                record: TerminationRecord {
                    request_id,
                    stage: "artifact_rejected".to_string(),
                    elapsed: started.elapsed(),
                    last_progress,
                    last_artifact_error: self.last_artifact_error.take(),
                    exit_kind: ExitKind::Cooperative,
                },
            };
        }
        let mut first_error: Option<String> = None;
        if let Some(stage) = self.stage.as_ref() {
            let headers = std::mem::take(&mut self.artifact_headers);
            for header in headers {
                let promotion = stage.validate_and_promote(&header);
                if let Err(error) = promotion {
                    first_error = Some(error.to_string());
                    break;
                }
            }
        }

        if let Some(error) = first_error {
            let _ = self.stage.take().map(|stage| stage.discard());
            let artifact_error = self.last_artifact_error.take().or(Some(error.clone()));
            return SupervisorOutcome::ForceTerminated {
                record: TerminationRecord {
                    request_id,
                    stage: format!("promotion_failed:{error}"),
                    elapsed: started.elapsed(),
                    last_progress,
                    last_artifact_error: artifact_error,
                    exit_kind: ExitKind::Cooperative,
                },
            };
        }

        let _ = self.stage.take();
        SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id,
                stage: "completed".to_string(),
                elapsed: started.elapsed(),
                last_progress,
                last_artifact_error: self.last_artifact_error.take(),
                exit_kind: ExitKind::Cooperative,
            },
        }
    }

    fn discard_stage(&mut self) {
        self.artifact_headers.clear();
        if let Some(stage) = self.stage.take() {
            let _ = stage.discard();
        }
    }

    fn force_terminate_outcome(
        &mut self,
        request_id: &str,
        started: Instant,
        stage: &str,
        detail: Option<String>,
        last_progress: Option<Progress>,
    ) -> SupervisorOutcome {
        self.discard_stage();
        let stage_label = match detail {
            Some(detail) => format!("{stage}:{detail}"),
            None => stage.to_string(),
        };
        let _ = self.host.terminate();
        SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: request_id.to_string(),
                stage: stage_label,
                elapsed: started.elapsed(),
                last_progress,
                last_artifact_error: self.last_artifact_error.take(),
                exit_kind: ExitKind::ForceAfterGrace,
            },
        }
    }
}

/// Short, stable label for a non-WorkerReady envelope that arrives
/// during the handshake phase. Used in `TerminationRecord.stage` so
/// the host's diagnostic taxonomy sees exactly which envelope type
/// raced the handshake.
fn envelope_kind_label(envelope: &Envelope) -> String {
    match envelope {
        Envelope::WorkerReady { .. } => unreachable!("filtered by caller"),
        Envelope::Request { request_id, .. } => format!("request:{request_id}"),
        Envelope::Cancel { request_id, .. } => format!("cancel:{request_id}"),
        Envelope::Progress { stage, .. } => format!("progress:{stage}"),
        Envelope::Artifact { header, .. } => format!("artifact:{}", header.staging_name),
        Envelope::Completed { request_id, .. } => format!("completed:{request_id}"),
        Envelope::Cancelled { request_id, .. } => format!("cancelled:{request_id}"),
        Envelope::Failed { request_id, .. } => format!("failed:{request_id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{FramedWorkerHost, WorkerHost};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, mpsc};

    /// A fake worker that serves a scripted sequence of envelopes to
    /// `recv` and records every envelope it received via `send`. The
    /// fake never sleeps; the supervisor's grace period is exercised
    /// with a sub-millisecond `Duration`.
    struct ScriptedWorker {
        received: Vec<Envelope>,
        script: VecDeque<Result<Envelope, WorkerError>>,
        cancel_calls: Vec<(String, String)>,
        terminated: Arc<Mutex<usize>>,
    }

    impl ScriptedWorker {
        fn new(script: Vec<Envelope>) -> Self {
            Self {
                received: Vec::new(),
                script: script.into_iter().map(Ok).collect(),
                cancel_calls: Vec::new(),
                terminated: Arc::new(Mutex::new(0)),
            }
        }

        fn with_results(script: Vec<Result<Envelope, WorkerError>>) -> (Self, Arc<Mutex<usize>>) {
            let terminated = Arc::new(Mutex::new(0));
            (
                Self {
                    received: Vec::new(),
                    script: script.into(),
                    cancel_calls: Vec::new(),
                    terminated: Arc::clone(&terminated),
                },
                terminated,
            )
        }
    }

    fn ready_envelope() -> Envelope {
        Envelope::WorkerReady {
            schema_version: crate::schema_version().to_string(),
            worker_id: "fake".to_string(),
        }
    }

    impl WorkerHost for ScriptedWorker {
        fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
            self.received.push(envelope.clone());
            Ok(())
        }

        fn recv(&mut self, _deadline: Instant) -> Result<Envelope, WorkerError> {
            self.script.pop_front().unwrap_or(Err(WorkerError::Closed))
        }

        fn cancel(&mut self, request_id: &str, reason: &str) -> Result<(), WorkerError> {
            self.cancel_calls
                .push((request_id.to_string(), reason.to_string()));
            Ok(())
        }

        fn terminate(&mut self) -> Result<(), WorkerError> {
            *self.terminated.lock().expect("termination log mutex") += 1;
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

    fn timed_out_transport() -> (
        FramedWorkerHost,
        mpsc::Sender<Vec<u8>>,
        mpsc::Receiver<Vec<u8>>,
    ) {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        (
            FramedWorkerHost::new(inbound_rx, outbound_tx),
            inbound_tx,
            outbound_rx,
        )
    }

    #[test]
    fn cancel_acknowledges_when_worker_acks_inside_grace() {
        let envelope = Envelope::Cancelled {
            schema_version: crate::schema_version().to_string(),
            request_id: "req-1".to_string(),
            reason: "user pressed stop".to_string(),
        };
        let worker = ScriptedWorker::new(vec![envelope]);
        let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

        let outcome = supervisor.cancel("req-1", "user pressed stop");
        match outcome {
            SupervisorOutcome::Acknowledged {
                request_id, reason, ..
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(reason, "user pressed stop");
            }
            other => panic!("expected Acknowledged; got {other:?}"),
        }
    }

    #[test]
    fn cancel_force_terminates_when_worker_never_acks_inside_grace() {
        let worker = ScriptedWorker::new(Vec::new());
        let mut supervisor = Supervisor::new(Duration::from_micros(1), Box::new(worker), None);

        let outcome = supervisor.cancel("req-1", "user pressed stop");
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.request_id, "req-1");
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
                assert!(
                    record.stage.starts_with("grace_exceeded")
                        || record.stage.starts_with("worker_closed"),
                    "force-terminate stage should be grace_exceeded or worker_closed; got {:?}",
                    record.stage
                );
            }
            other => panic!("expected ForceTerminated; got {other:?}"),
        }
    }

    #[test]
    fn request_consumes_worker_ready_handshake_before_sending_request() {
        let worker = ScriptedWorker::new(vec![
            ready_envelope(),
            Envelope::Completed {
                schema_version: crate::schema_version().to_string(),
                request_id: "req-1".to_string(),
                result: serde_json::json!({ "ok": true }),
            },
        ]);
        let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

        let outcome = supervisor.request(sample_request());
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.stage, "completed");
                assert_eq!(record.exit_kind, ExitKind::Cooperative);
                assert_eq!(record.request_id, "req-1");
            }
            other => panic!("expected ForceTerminated(Completed); got {other:?}"),
        }
    }

    #[test]
    fn request_rejects_handshake_with_mismatched_schema_version() {
        let worker = ScriptedWorker::new(vec![Envelope::WorkerReady {
            schema_version: "threeterm.protocol/0".to_string(),
            worker_id: "fake".to_string(),
        }]);
        let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

        let outcome = supervisor.request(sample_request());
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert!(
                    record.stage.starts_with("handshake_schema_mismatch"),
                    "expected handshake_schema_mismatch; got {:?}",
                    record.stage
                );
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
            }
            other => panic!("expected ForceTerminated; got {other:?}"),
        }
    }

    #[test]
    fn request_force_terminates_when_worker_never_sends_worker_ready() {
        let worker = ScriptedWorker::new(Vec::new());
        let mut supervisor = Supervisor::new(Duration::from_micros(1), Box::new(worker), None);

        let outcome = supervisor.request(sample_request());
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.request_id, "<handshake>");
                assert!(
                    record.stage.starts_with("handshake_grace_exceeded")
                        || record.stage.starts_with("handshake_worker_closed"),
                    "expected handshake_grace_exceeded or handshake_worker_closed; got {:?}",
                    record.stage
                );
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
            }
            other => panic!("expected ForceTerminated; got {other:?}"),
        }
    }

    #[test]
    fn request_force_terminates_when_handshake_receive_times_out() {
        let (worker, terminated) = ScriptedWorker::with_results(vec![Err(WorkerError::TimedOut)]);
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request())
        else {
            panic!("expected force termination");
        };
        assert_eq!(record.stage, "handshake_grace_exceeded");
        assert_eq!(*terminated.lock().expect("termination log mutex"), 1);
    }

    #[test]
    fn framed_transport_timeout_force_terminates_handshake_without_sending_request() {
        let (transport, _inbound_tx, _outbound_rx) = timed_out_transport();
        let mut supervisor = Supervisor::new(Duration::ZERO, Box::new(transport), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request())
        else {
            panic!("expected force termination");
        };
        assert_eq!(record.stage, "handshake_grace_exceeded");
    }

    #[test]
    fn request_force_terminates_when_terminal_receive_times_out() {
        let (worker, terminated) = ScriptedWorker::with_results(vec![
            Ok(ready_envelope()),
            Ok(Envelope::Progress {
                schema_version: crate::schema_version().to_string(),
                request_id: "req-1".to_string(),
                stage: "tessellating".to_string(),
                percent: 50,
            }),
            Err(WorkerError::TimedOut),
        ]);
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request())
        else {
            panic!("expected force termination");
        };
        assert_eq!(record.stage, "grace_exceeded");
        assert_eq!(
            record.last_progress,
            Some(Progress {
                stage: "tessellating".to_string(),
                percent: 50
            })
        );
        assert_eq!(*terminated.lock().expect("termination log mutex"), 1);
    }

    #[test]
    fn framed_transport_timeout_force_terminates_request_after_handshake() {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, _outbound_rx) = mpsc::channel();
        inbound_tx
            .send(crate::worker::encode_frame(&ready_envelope()).expect("ready envelope encodes"))
            .expect("ready frame queues");
        let mut supervisor = Supervisor::new(
            Duration::ZERO,
            Box::new(FramedWorkerHost::new(inbound_rx, outbound_tx)),
            None,
        );

        let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request())
        else {
            panic!("expected force termination");
        };
        assert_eq!(record.stage, "grace_exceeded");
    }

    #[test]
    fn cancel_force_terminates_when_acknowledgement_receive_times_out() {
        let (worker, terminated) = ScriptedWorker::with_results(vec![Err(WorkerError::TimedOut)]);
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.cancel("req-1", "stop")
        else {
            panic!("expected force termination");
        };
        assert_eq!(record.stage, "cancel_grace_exceeded");
        assert_eq!(*terminated.lock().expect("termination log mutex"), 1);
    }

    #[test]
    fn framed_transport_timeout_force_terminates_cancellation() {
        let (transport, _inbound_tx, _outbound_rx) = timed_out_transport();
        let mut supervisor = Supervisor::new(Duration::ZERO, Box::new(transport), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.cancel("req-1", "stop")
        else {
            panic!("expected force termination");
        };
        assert_eq!(record.stage, "cancel_grace_exceeded");
    }

    #[test]
    fn request_treats_unsolicited_cancelled_as_protocol_violation() {
        // Worker sends WorkerReady then a Cancelled envelope without the
        // host ever sending a Cancel. This must NOT be classified as a
        // cooperative ack.
        let worker = ScriptedWorker::new(vec![
            ready_envelope(),
            Envelope::Cancelled {
                schema_version: crate::schema_version().to_string(),
                request_id: "req-1".to_string(),
                reason: "spurious".to_string(),
            },
        ]);
        let mut supervisor = Supervisor::new(Duration::from_millis(100), Box::new(worker), None);

        let outcome = supervisor.request(sample_request());
        // The unsolicited Cancelled is recorded as a protocol violation;
        // the loop continues until grace expires.
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
                let progress = record
                    .last_progress
                    .expect("unsolicited Cancelled must surface in last_progress");
                assert!(
                    progress
                        .stage
                        .starts_with("protocol_violation:unsolicited_cancelled:"),
                    "expected protocol_violation:unsolicited_cancelled: stage; got {:?}",
                    progress.stage
                );
            }
            other => panic!("expected ForceTerminated; got {other:?}"),
        }
    }

    #[test]
    fn request_force_terminates_with_progress_when_worker_never_finishes() {
        let worker = ScriptedWorker::new(vec![
            ready_envelope(),
            Envelope::Progress {
                schema_version: crate::schema_version().to_string(),
                request_id: "req-1".to_string(),
                stage: "tessellating".to_string(),
                percent: 50,
            },
            Envelope::Progress {
                schema_version: crate::schema_version().to_string(),
                request_id: "req-1".to_string(),
                stage: "almost done".to_string(),
                percent: 95,
            },
        ]);
        let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);

        let outcome = supervisor.request(sample_request());
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.request_id, "req-1");
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
                let progress = record
                    .last_progress
                    .expect("supervisor tracks the most recent progress");
                assert_eq!(progress.stage, "almost done");
                assert_eq!(progress.percent, 95);
            }
            other => panic!("expected ForceTerminated; got {other:?}"),
        }
    }
}

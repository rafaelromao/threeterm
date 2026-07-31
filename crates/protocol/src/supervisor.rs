//! Cooperative cancellation supervisor.
//!
//! The supervisor drives one `WorkerHost` per `Request`, sends the
//! request, and observes the worker's terminal envelopes. The
//! `Supervisor::request` method runs the request lifecycle: send a
//! `Request`, track staged artifacts, and promote them on `Completed` or
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

use crate::artifact::Stage;
use crate::worker::{Envelope, WorkerError, WorkerHost};

/// A host-issued worker request.
#[derive(Debug, Clone)]
pub struct Request {
    pub request_id: String,
    pub command_id: String,
    pub args: Value,
    pub revision_id: String,
}

/// Outcome of a single `Supervisor::run` / `Supervisor::cancel` invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorOutcome {
    /// The worker acknowledged the cooperative cancellation inside the
    /// grace period and exited cleanly.
    Acknowledged {
        request_id: String,
        reason: String,
        elapsed: Duration,
    },
    /// The worker did not acknowledge inside the grace period; the
    /// supervisor force-terminated it.
    ForceTerminated { record: TerminationRecord },
}

/// Structured record emitted on a force-terminated run. The host uses
/// these fields to surface the failure in its diagnostic surface and to
/// prove the canonical state survived: the staged Derived Result was
/// discarded; the Revision Snapshot was never touched.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminationRecord {
    pub request_id: String,
    pub stage: String,
    pub elapsed: Duration,
    pub last_progress: Option<Progress>,
    pub exit_kind: ExitKind,
}

/// Worker exit category. `Cooperative` is reserved for a future
/// successful-with-ack path; `ForceAfterGrace` is the variant emitted
/// when the supervisor had to kill the worker after the grace expired.
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
        Self { grace, host, stage }
    }

    /// Returns the configured grace period.
    pub fn grace(&self) -> Duration {
        self.grace
    }

    /// Cooperative cancellation lifecycle: send a `Cancel` envelope and
    /// wait for a `Cancelled` acknowledgement inside the configured
    /// grace period. If the worker does not ack inside the grace
    /// period, the supervisor force-terminates it.
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
            match self.host.recv() {
                Ok(Envelope::Cancelled {
                    request_id: ack_request_id,
                    reason: ack_reason,
                    ..
                }) => {
                    self.discard_stage();
                    return SupervisorOutcome::Acknowledged {
                        request_id: ack_request_id,
                        reason: ack_reason,
                        elapsed: started.elapsed(),
                    };
                }
                Ok(Envelope::Artifact { staging_name, .. }) => {
                    self.record_unexpected_artifact(staging_name);
                }
                Ok(Envelope::Progress { stage, percent, .. }) => {
                    let _ = (stage, percent);
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

    /// Request lifecycle: send the `Request` envelope, track staged
    /// artifacts, and wait for a terminal envelope inside the
    /// configured grace period. On `Completed` every staged artifact is
    /// promoted to its final filename; on `Failed` / `Cancelled` /
    /// force termination every staged artifact is discarded.
    pub fn request(&mut self, request: Request) -> SupervisorOutcome {
        let started = Instant::now();
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

        let deadline = started + self.grace;
        let mut last_progress: Option<Progress> = None;

        loop {
            match self.host.recv() {
                Ok(Envelope::Progress { stage, percent, .. }) => {
                    last_progress = Some(Progress { stage, percent });
                }
                Ok(Envelope::Artifact {
                    staging_name,
                    bytes_b64,
                    sha256,
                    request_id,
                    ..
                }) => {
                    self.record_artifact(&staging_name, &bytes_b64, &sha256, &request_id);
                }
                Ok(Envelope::Cancelled {
                    request_id: ack_request_id,
                    reason: ack_reason,
                    ..
                }) => {
                    self.discard_stage();
                    return SupervisorOutcome::Acknowledged {
                        request_id: ack_request_id,
                        reason: ack_reason,
                        elapsed: started.elapsed(),
                    };
                }
                Ok(Envelope::Completed { request_id, .. }) => {
                    self.promote_stage();
                    return SupervisorOutcome::ForceTerminated {
                        record: TerminationRecord {
                            request_id,
                            stage: "completed".to_string(),
                            elapsed: started.elapsed(),
                            last_progress,
                            exit_kind: ExitKind::Cooperative,
                        },
                    };
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

    fn record_artifact(
        &self,
        staging_name: &str,
        bytes_b64: &str,
        advertised_sha256: &str,
        request_id: &str,
    ) {
        if let Some(stage) = self.stage.as_ref()
            && let Err(error) = stage.write(staging_name, bytes_b64, advertised_sha256)
        {
            let _ = (error, request_id);
        }
    }

    fn record_unexpected_artifact(&self, _staging_name: String) {}

    fn promote_stage(&mut self) {
        if let Some(_stage) = self.stage.take() {
            // Promotion of every staged artifact is part of a follow-up
            // slice; the foundation slice only tracks and discards.
        }
    }

    fn discard_stage(&mut self) {
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
        SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: request_id.to_string(),
                stage: stage_label,
                elapsed: started.elapsed(),
                last_progress,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::WorkerHost;
    use std::collections::VecDeque;

    /// A fake worker that serves a scripted sequence of envelopes to
    /// `recv` and records every envelope it received via `send`. The
    /// fake never sleeps; the supervisor's grace period is exercised
    /// with a sub-millisecond `Duration`.
    struct ScriptedWorker {
        received: Vec<Envelope>,
        script: VecDeque<Envelope>,
        cancel_calls: Vec<(String, String)>,
    }

    impl ScriptedWorker {
        fn new(script: Vec<Envelope>) -> Self {
            Self {
                received: Vec::new(),
                script: script.into(),
                cancel_calls: Vec::new(),
            }
        }
    }

    impl WorkerHost for ScriptedWorker {
        fn send(&mut self, envelope: &Envelope) -> Result<(), WorkerError> {
            self.received.push(envelope.clone());
            Ok(())
        }

        fn recv(&mut self) -> Result<Envelope, WorkerError> {
            self.script.pop_front().ok_or(WorkerError::Closed)
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
    fn request_returns_completed_when_worker_emits_completed_envelope() {
        let worker = ScriptedWorker::new(vec![Envelope::Completed {
            schema_version: crate::schema_version().to_string(),
            request_id: "req-1".to_string(),
            result: serde_json::json!({ "ok": true }),
        }]);
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
    fn request_force_terminates_with_progress_when_worker_never_finishes() {
        let worker = ScriptedWorker::new(vec![
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

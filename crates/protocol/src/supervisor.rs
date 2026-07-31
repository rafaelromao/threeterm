//! Cooperative cancellation supervisor.
//!
//! The supervisor drives one `WorkerHost` per `Request`, sends the
//! request, and observes the worker's terminal envelopes. The supervisor
//! sends a cooperative cancellation first; if the worker acknowledges
//! with a `Cancelled` envelope inside the configured grace period the
//! outcome is `Acknowledged`. Otherwise the supervisor force-terminates
//! the worker, discards any staged Derived Result, and emits a
//! structured `TerminationRecord` so the host preserves its authoritative
//! Revision Snapshot.

use std::fmt;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::artifact::{ArtifactError, Stage};
use crate::worker::{Envelope, WorkerError, WorkerHost};

/// A host-issued worker request.
#[derive(Debug, Clone)]
pub struct Request {
    pub request_id: String,
    pub command_id: String,
    pub args: Value,
    pub revision_id: String,
}

/// Outcome of a single `Supervisor::run` invocation.
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
/// prove the canonical state survived (the staged Derived Result was
/// discarded; the Revision Snapshot was never touched).
#[derive(Debug, Clone, PartialEq)]
pub struct TerminationRecord {
    pub request_id: String,
    pub stage: String,
    pub elapsed: Duration,
    pub last_progress: Option<Progress>,
    pub exit_kind: ExitKind,
}

/// Worker exit category. `Cooperative` is reserved for a future
/// successful-with-ack path; `ForceAfterGrace` is the only variant the
/// foundation slice emits inside `TerminationRecord`.
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

/// Drives a single worker process through one `Request`. Each `Supervisor`
/// owns one `WorkerHost` for its lifetime; the production wiring spawns a
/// fresh disposable worker per `Supervisor` (closed issue 49: one
/// disposable worker per request).
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

    /// Run the supervisor against `request`. Returns when the worker
    /// acknowledges cancellation, completes, fails, or fails to
    /// acknowledge the cooperative cancel inside the grace period.
    pub fn run(&mut self, request: Request) -> SupervisorOutcome {
        let started = Instant::now();
        if let Err(error) = self.host.send(&Envelope::Request {
            schema_version: crate::schema_version().to_string(),
            request_id: request.request_id.clone(),
            command_id: request.command_id.clone(),
            args: request.args.clone(),
            revision_id: request.revision_id.clone(),
        }) {
            return self.force_terminate(
                &request,
                started,
                "host_send_failed",
                Some(error.to_string()),
                None,
            );
        }

        let deadline = started + self.grace;
        let mut last_progress: Option<Progress> = None;

        loop {
            // Always attempt at least one recv so the worker can deliver
            // a queued terminal envelope (e.g. a scripted Cancelled
            // acknowledgement). The deadline check happens after the
            // recv so the cooperative path can succeed even when the
            // grace period is shorter than the time to schedule the
            // first recv.
            let recv_result = self.host.recv();

            match recv_result {
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
                    if let Some(stage) = self.stage.as_ref()
                        && let Err(error) = stage.write(&staging_name, &bytes_b64, &sha256)
                    {
                        // A failed artifact is non-authoritative by
                        // contract; the next envelope decides the
                        // outcome. We surface it through last_progress
                        // so the termination record captures the
                        // failure stage.
                        last_progress = Some(Progress {
                            stage: format!("artifact_rejected:{error}"),
                            percent: 0,
                        });
                        let _ = request_id; // silence unused warning
                    }
                }
                Ok(Envelope::Cancelled {
                    request_id, reason, ..
                }) => {
                    return SupervisorOutcome::Acknowledged {
                        request_id,
                        reason,
                        elapsed: started.elapsed(),
                    };
                }
                Ok(Envelope::Completed { request_id, .. }) => {
                    return SupervisorOutcome::ForceTerminated {
                        record: TerminationRecord {
                            request_id,
                            stage: "completed_unexpectedly".to_string(),
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
                    last_progress = Some(Progress {
                        stage: format!("failed:{code}:{detail}"),
                        percent: 0,
                    });
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
                Ok(Envelope::Request { .. }) | Ok(Envelope::Cancel { .. }) => {
                    last_progress = Some(Progress {
                        stage: "protocol_violation:worker_sent_host_only_envelope".to_string(),
                        percent: 0,
                    });
                }
                Err(WorkerError::Closed) => {
                    return self.force_terminate(
                        &request,
                        started,
                        "worker_closed",
                        None,
                        last_progress.take(),
                    );
                }
                Err(error) => {
                    return self.force_terminate(
                        &request,
                        started,
                        "worker_recv_error",
                        Some(error.to_string()),
                        last_progress.take(),
                    );
                }
            }

            if Instant::now() >= deadline {
                return self.force_terminate(
                    &request,
                    started,
                    "grace_exceeded",
                    None,
                    last_progress.take(),
                );
            }
        }
    }

    fn force_terminate(
        &mut self,
        request: &Request,
        started: Instant,
        stage: &str,
        detail: Option<String>,
        last_progress: Option<Progress>,
    ) -> SupervisorOutcome {
        // Best-effort cooperative cancel first; ignore a second-order
        // error here because we are already on the termination path.
        let _ = self.host.cancel(&request.request_id, "force_terminate");
        if let Some(stage_dir) = self.stage.take() {
            let _ = stage_dir.discard();
        }
        let stage_label = match detail {
            Some(detail) => format!("{stage}:{detail}"),
            None => stage.to_string(),
        };
        SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: request.request_id.clone(),
                stage: stage_label,
                elapsed: started.elapsed(),
                last_progress,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        }
    }
}

/// Errors emitted by `Supervisor::new`. Currently only the artifact
/// stage can fail to open; other failures surface inside `SupervisorOutcome`.
#[derive(Debug)]
pub enum SupervisorError {
    Artifact(ArtifactError),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Artifact(error) => write!(formatter, "supervisor artifact stage error: {error}"),
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<ArtifactError> for SupervisorError {
    fn from(error: ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{Envelope, WorkerError, WorkerHost};
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
    fn supervisor_acknowledges_when_worker_cancels_inside_the_grace_period() {
        let envelope = Envelope::Cancelled {
            schema_version: crate::schema_version().to_string(),
            request_id: "req-1".to_string(),
            reason: "user pressed stop".to_string(),
        };
        let worker = ScriptedWorker::new(vec![envelope.clone()]);
        let mut supervisor = Supervisor::new(Duration::from_micros(1), Box::new(worker), None);

        let outcome = supervisor.run(sample_request());
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
    fn supervisor_force_terminates_when_worker_never_acks_inside_the_grace_period() {
        // Two progress envelopes, then Closed. The grace period is long
        // enough for the fake to deliver both, then the next recv sees
        // Closed and the supervisor force-terminates with the most
        // recent progress recorded.
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
                stage: "still tessellating".to_string(),
                percent: 60,
            },
        ]);
        let mut supervisor = Supervisor::new(Duration::from_millis(10), Box::new(worker), None);

        let outcome = supervisor.run(sample_request());
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.request_id, "req-1");
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
                assert!(
                    record.stage.starts_with("grace_exceeded")
                        || record.stage.starts_with("worker_closed"),
                    "force terminate should be grace_exceeded or worker_closed; got {:?}",
                    record.stage
                );
                let progress = record
                    .last_progress
                    .expect("supervisor tracks the most recent progress");
                assert_eq!(progress.stage, "still tessellating");
                assert_eq!(progress.percent, 60);
            }
            other => panic!("expected ForceTerminated; got {other:?}"),
        }
    }

    #[test]
    fn supervisor_force_terminates_when_worker_closes_without_acking() {
        let worker = ScriptedWorker::new(Vec::new());
        let mut supervisor = Supervisor::new(Duration::from_micros(1), Box::new(worker), None);

        let outcome = supervisor.run(sample_request());
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
                assert!(record.stage.starts_with("worker_closed"));
            }
            other => panic!("expected ForceTerminated; got {other:?}"),
        }
    }
}

//! Cooperative cancellation supervisor.
//!
//! The supervisor drives one `WorkerHost` per `Request`. The
//! `Supervisor::request` method runs the full request lifecycle:
//! consume the versioned `WorkerReady` handshake, send the `Request`
//! envelope, track staged artifact facts on `Completed`, or
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
    /// The worker completed a request. `result` is the worker's typed
    /// completion value; `artifact_headers` describe files that remain
    /// staged. The Host is solely responsible for accepting and
    /// publishing staged artifacts against its current Revision
    /// Snapshot.
    Completed {
        request_id: String,
        result: Value,
        artifact_headers: Vec<StagedArtifact>,
    },
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

/// A worker-reported staged artifact, including its envelope identity.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedArtifact {
    pub schema_version: String,
    pub header: ArtifactHeader,
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
    /// The actual Linux signal the worker exited by, when it did not
    /// exit cleanly (crash, forced kill). `None` for a normal exit.
    pub exit_signal: Option<i32>,
    /// The worker's numeric exit code, when it exited by calling
    /// `exit(n)` rather than by a signal. `None` for a signal exit or
    /// before the worker has been reaped.
    pub exit_code: Option<i32>,
    /// Bounded tail of the worker's stderr, preserved so the diagnostic
    /// surface keeps structured context on failure.
    pub stderr_tail: String,
    /// Stable failure identifier from a cooperative `Failed` envelope,
    /// surfaced structurally instead of only inside the free-form
    /// `stage` string.
    pub failed_code: Option<String>,
    /// Offending detail from a cooperative `Failed` envelope.
    pub failed_detail: Option<String>,
    pub exit_kind: ExitKind,
}

/// Structured fields carried by a cooperative `Failed` envelope.
#[derive(Debug, Clone, PartialEq)]
struct FailedFields {
    code: String,
    detail: String,
}

/// Diagnostic context accumulated during a request lifecycle, threaded
/// into every terminal record.
#[derive(Debug, Clone, PartialEq)]
struct TerminationContext {
    last_progress: Option<Progress>,
    last_artifact_error: Option<String>,
    failed: Option<FailedFields>,
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
    expected_worker_id: Option<String>,
    /// Artifact headers accumulated during the request lifecycle. The
    /// `Completed` arm returns them as worker lifecycle facts; `discard_stage`
    /// clears them without publishing.
    artifact_headers: Vec<StagedArtifact>,
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
            .field("expected_worker_id", &self.expected_worker_id)
            .finish_non_exhaustive()
    }
}

impl Supervisor {
    pub fn new(grace: Duration, host: Box<dyn WorkerHost>, stage: Option<Stage>) -> Self {
        Self {
            grace,
            host,
            stage,
            expected_worker_id: None,
            artifact_headers: Vec::new(),
            last_artifact_error: None,
        }
    }

    /// Returns the configured grace period.
    pub fn grace(&self) -> Duration {
        self.grace
    }

    /// Require the handshake to identify the configured worker before a
    /// request is dispatched.
    pub fn with_expected_worker_id(mut self, worker_id: impl Into<String>) -> Self {
        self.expected_worker_id = Some(worker_id.into());
        self
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
        self.cancel_with_deadline(request_id, reason, started, started + self.grace, None)
    }

    fn cancel_with_deadline(
        &mut self,
        request_id: &str,
        reason: &str,
        started: Instant,
        deadline: Instant,
        mut last_progress: Option<Progress>,
    ) -> SupervisorOutcome {
        if Instant::now() >= deadline {
            return self.force_terminate_outcome(
                request_id,
                started,
                "cancel_grace_exceeded",
                None,
                last_progress.take(),
            );
        }
        if let Err(error) = self.host.cancel(request_id, reason) {
            return self.force_terminate_outcome(
                request_id,
                started,
                "host_cancel_failed",
                Some(error.to_string()),
                last_progress.take(),
            );
        }

        loop {
            match self.host.recv(deadline) {
                Ok(envelope) if envelope.schema_version() != crate::schema_version() => {
                    // Every post-handshake envelope must carry the
                    // canonical protocol version, or cancellation fails
                    // closed (the version is part of the message
                    // binding contract).
                    return self.force_terminate_outcome(
                        request_id,
                        started,
                        "envelope_schema_mismatch",
                        Some(format!(
                            "received={:?} expected={:?}",
                            envelope.schema_version(),
                            crate::schema_version()
                        )),
                        last_progress.take(),
                    );
                }
                Ok(Envelope::Cancelled {
                    request_id: ack_request_id,
                    reason: ack_reason,
                    ..
                }) => {
                    if ack_request_id != request_id {
                        // A cancellation acknowledgement bound to a
                        // foreign request is a protocol violation; the
                        // staged output is discarded and the cancellation
                        // fails closed rather than being kept alive by a
                        // stream of misbound acknowledgements.
                        return self.force_terminate_outcome(
                            request_id,
                            started,
                            &format!("protocol_violation:mismatched_request_id:{ack_request_id}"),
                            None,
                            last_progress.take(),
                        );
                    }
                    self.discard_stage();
                    if let Err(error) = self.host.finish_terminal() {
                        let context = TerminationContext {
                            last_progress: last_progress.take(),
                            last_artifact_error: self.last_artifact_error.take(),
                            failed: None,
                        };
                        return SupervisorOutcome::ForceTerminated {
                            record: self.termination_record(
                                ack_request_id,
                                format!("cancel_terminal_finalize_failed:{error}"),
                                started,
                                context,
                                ExitKind::ForceAfterGrace,
                            ),
                        };
                    }
                    // The worker acknowledged cooperatively; verify it
                    // exited cleanly (no signal, exit code 0) before
                    // reporting an acknowledgement. A worker that acks
                    // and then dies by signal or with a non-zero status
                    // is surfaced as a structured termination.
                    if let Some(outcome) = self.verify_clean_exit(
                        &ack_request_id,
                        started,
                        "cancelled_reap",
                        last_progress.take(),
                    ) {
                        return outcome;
                    }
                    return SupervisorOutcome::Acknowledged {
                        request_id: ack_request_id,
                        reason: ack_reason,
                        elapsed: started.elapsed(),
                    };
                }
                Ok(Envelope::Progress {
                    request_id: progress_request_id,
                    stage,
                    percent,
                    ..
                }) => {
                    // Progress bound to a foreign request is a protocol
                    // violation; cancellation fails closed rather than
                    // accepting misbound progress.
                    if progress_request_id != request_id {
                        return self.force_terminate_outcome(
                            request_id,
                            started,
                            &format!(
                                "protocol_violation:mismatched_request_id:{progress_request_id}"
                            ),
                            None,
                            last_progress.take(),
                        );
                    }
                    last_progress = Some(Progress { stage, percent });
                }
                Ok(Envelope::Artifact { header, .. }) => {
                    if header.request_id != request_id {
                        return self.force_terminate_outcome(
                            request_id,
                            started,
                            &format!(
                                "protocol_violation:mismatched_request_id:{}",
                                header.request_id
                            ),
                            None,
                            last_progress.take(),
                        );
                    }
                }
                // A request-bearing terminal envelope while waiting for
                // a Cancelled acknowledgement is a protocol violation:
                // the worker neither acked the cancellation nor is it
                // talking about the active request. Fail closed and
                // discard the staged output. A WorkerReady left over
                // from the handshake is tolerated.
                Ok(
                    Envelope::Completed { .. }
                    | Envelope::Failed { .. }
                    | Envelope::Request { .. }
                    | Envelope::Cancel { .. },
                ) => {
                    return self.force_terminate_outcome(
                        request_id,
                        started,
                        "protocol_violation:expected_cancelled_ack",
                        None,
                        last_progress.take(),
                    );
                }
                Ok(Envelope::WorkerReady { .. }) => {}
                Err(WorkerError::Closed) => {
                    return self.force_terminate_outcome(
                        request_id,
                        started,
                        "worker_closed",
                        None,
                        last_progress.take(),
                    );
                }
                // A slice-level receive timeout is a poll tick: keep
                // waiting; the loop's deadline check below terminates
                // the cancellation when the grace expires.
                Err(WorkerError::TimedOut) => {}
                Err(error) => {
                    return self.force_terminate_outcome(
                        request_id,
                        started,
                        "worker_recv_error",
                        Some(error.to_string()),
                        last_progress.take(),
                    );
                }
            }

            if Instant::now() >= deadline {
                return self.force_terminate_outcome(
                    request_id,
                    started,
                    "cancel_grace_exceeded",
                    None,
                    last_progress.take(),
                );
            }
        }
    }

    /// Request lifecycle: consume the versioned `WorkerReady`
    /// handshake, send the `Request` envelope, track staged artifacts,
    /// and wait for a terminal envelope inside the configured grace
    /// period. On `Completed` staged artifact facts are returned to the
    /// caller; on `Failed` / unsolicited `Cancelled` / force termination
    /// every staged artifact is discarded.
    pub fn request(&mut self, request: Request) -> SupervisorOutcome {
        self.request_with_cancel(request, &std::sync::atomic::AtomicBool::new(false))
    }

    /// Request lifecycle with a cooperative cancellation trigger. The
    /// request loop polls `cancel` between receive slices; when it is
    /// set, the supervisor runs the cooperative cancellation lifecycle
    /// (send `Cancel`, await `Cancelled` inside the grace period, then
    /// force-terminate if the worker does not acknowledge).
    pub fn request_with_cancel(
        &mut self,
        request: Request,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> SupervisorOutcome {
        let started = Instant::now();

        // Phase 1: consume one WorkerReady handshake. The worker must
        // advertise the canonical schema_version or the host fails
        // closed before sending any `Request`.
        let deadline = started + self.grace;
        if let Some(outcome) = self.consume_worker_ready(&request.request_id, started, deadline) {
            return outcome;
        }
        if Instant::now() >= deadline {
            return self.force_terminate_outcome(
                &request.request_id,
                started,
                "handshake_grace_exceeded",
                None,
                None,
            );
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
            // Cooperative cancellation trigger: when the flag is set the
            // request lifecycle hands over to the cancellation lifecycle
            // (send `Cancel`, await `Cancelled` inside the grace period,
            // then force-terminate if the worker does not acknowledge).
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return self.cancel_with_deadline(
                    &request.request_id,
                    "cancelled by host",
                    started,
                    deadline,
                    last_progress.take(),
                );
            }
            if let Some(outcome) =
                self.receive_request_envelope(&request, started, deadline, &mut last_progress)
            {
                return outcome;
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

    /// Receives the next envelope for the active request and dispatches
    /// it. Returns `Some(outcome)` when the envelope terminates the
    /// request lifecycle; returns `None` to keep waiting. Progress
    /// facts are recorded into `last_progress` in place.
    fn receive_request_envelope(
        &mut self,
        request: &Request,
        started: Instant,
        deadline: Instant,
        last_progress: &mut Option<Progress>,
    ) -> Option<SupervisorOutcome> {
        match self.host.recv(deadline) {
            Ok(envelope) if envelope.schema_version() != crate::schema_version() => {
                // Every post-handshake envelope must carry the
                // canonical protocol version, or the request fails
                // closed (the version is part of the message
                // binding contract).
                Some(self.force_terminate_outcome(
                    &request.request_id,
                    started,
                    "envelope_schema_mismatch",
                    Some(format!(
                        "received={:?} expected={:?}",
                        envelope.schema_version(),
                        crate::schema_version()
                    )),
                    None,
                ))
            }
            Ok(Envelope::Progress {
                request_id: progress_request_id,
                stage,
                percent,
                ..
            }) => {
                // A message bound to a foreign request is a protocol
                // violation: the active request fails closed rather
                // than accepting misbound progress.
                if progress_request_id != request.request_id {
                    return Some(self.force_terminate_outcome(
                        &request.request_id,
                        started,
                        &format!("protocol_violation:mismatched_request_id:{progress_request_id}"),
                        None,
                        Some(Progress {
                            stage: format!(
                                "protocol_violation:mismatched_request_id:{progress_request_id}"
                            ),
                            percent: 0,
                        }),
                    ));
                }
                *last_progress = Some(Progress { stage, percent });
                None
            }
            Ok(Envelope::Artifact {
                schema_version,
                header,
            }) => {
                if header.request_id != request.request_id {
                    return Some(self.force_terminate_outcome(
                        &request.request_id,
                        started,
                        &format!(
                            "protocol_violation:mismatched_request_id:{}",
                            header.request_id
                        ),
                        None,
                        Some(Progress {
                            stage: format!(
                                "protocol_violation:mismatched_request_id:{}",
                                header.request_id
                            ),
                            percent: 0,
                        }),
                    ));
                }
                self.record_artifact(schema_version, *header, request);
                None
            }
            // An unsolicited Cancelled envelope during the request
            // lifecycle is a protocol violation: `request()` never
            // sends a `Cancel`, so a `Cancelled` arriving here means
            // the worker is misbehaving. Fail closed immediately:
            // the staged output is discarded and the request is
            // terminated rather than waiting out the grace period.
            Ok(Envelope::Cancelled {
                request_id: cancelled_request_id,
                ..
            }) => Some(self.force_terminate_outcome(
                &request.request_id,
                started,
                &format!("protocol_violation:unsolicited_cancelled:{cancelled_request_id}"),
                None,
                None,
            )),
            Ok(Envelope::Completed {
                request_id, result, ..
            }) => Some(self.complete_with_artifact_facts(
                request_id,
                result,
                request,
                started,
                last_progress.take(),
            )),
            Ok(Envelope::Failed {
                request_id,
                code,
                detail,
                ..
            }) => {
                // A Failed envelope bound to a foreign request is a
                // protocol violation: the failure facts must not be
                // accepted for the active request.
                if request_id != request.request_id {
                    return Some(self.force_terminate_outcome(
                        &request.request_id,
                        started,
                        &format!("protocol_violation:mismatched_request_id:{request_id}"),
                        None,
                        Some(Progress {
                            stage: format!("protocol_violation:mismatched_request_id:{request_id}"),
                            percent: 0,
                        }),
                    ));
                }
                self.discard_stage();
                let stage_label = format!("failed:{code}:{detail}");
                let context = TerminationContext {
                    last_progress: last_progress.take(),
                    last_artifact_error: self.last_artifact_error.take(),
                    failed: Some(FailedFields { code, detail }),
                };
                Some(self.cooperative_termination_outcome(
                    request_id,
                    stage_label,
                    started,
                    context,
                ))
            }
            Ok(Envelope::WorkerReady { worker_id, .. }) => {
                *last_progress = Some(Progress {
                    stage: format!("unexpected_worker_ready:{worker_id}"),
                    percent: 0,
                });
                None
            }
            Ok(Envelope::Request { .. } | Envelope::Cancel { .. }) => {
                // Host-only envelopes are a protocol violation: the
                // worker must never send a Request or Cancel. Fail the
                // request closed immediately rather than allowing a
                // host-only envelope followed by a valid completion.
                Some(self.force_terminate_outcome(
                    &request.request_id,
                    started,
                    "protocol_violation:worker_sent_host_only_envelope",
                    None,
                    None,
                ))
            }
            Err(WorkerError::Closed) => Some(self.force_terminate_outcome(
                &request.request_id,
                started,
                "worker_closed",
                None,
                last_progress.take(),
            )),
            // A slice-level receive timeout is a poll tick: the worker
            // has not delivered an envelope yet. Keep waiting; the
            // caller's loop checks the deadline and terminates when it
            // expires.
            Err(WorkerError::TimedOut) => None,
            Err(error) => Some(self.force_terminate_outcome(
                &request.request_id,
                started,
                "worker_recv_error",
                Some(error.to_string()),
                last_progress.take(),
            )),
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
        // Receive slices return `TimedOut` to hand control back to the
        // caller; loop here until the handshake deadline expires.
        let envelope = loop {
            if Instant::now() >= deadline {
                return Some(self.force_terminate_outcome(
                    request_id,
                    started,
                    "handshake_grace_exceeded",
                    None,
                    None,
                ));
            }
            match self.host.recv(deadline) {
                Ok(envelope) => break envelope,
                Err(WorkerError::Closed) => {
                    return Some(self.force_terminate_outcome(
                        request_id,
                        started,
                        "handshake_worker_closed",
                        None,
                        None,
                    ));
                }
                // A slice-level receive timeout is a poll tick: keep
                // waiting for the handshake.
                Err(WorkerError::TimedOut) => {}
                Err(error) => {
                    return Some(self.force_terminate_outcome(
                        request_id,
                        started,
                        "handshake_worker_recv_error",
                        Some(error.to_string()),
                        None,
                    ));
                }
            }
        };
        match envelope {
            Envelope::WorkerReady {
                schema_version,
                worker_id,
            } => {
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
                } else if let Some(expected_worker_id) = &self.expected_worker_id
                    && worker_id != *expected_worker_id
                {
                    Some(self.force_terminate_outcome(
                        request_id,
                        started,
                        "handshake_worker_id_mismatch",
                        Some(format!(
                            "received={worker_id:?} expected={expected_worker_id:?}"
                        )),
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

    fn record_artifact(
        &mut self,
        schema_version: String,
        header: ArtifactHeader,
        request: &Request,
    ) {
        let _ = request;
        self.artifact_headers.push(StagedArtifact {
            schema_version,
            header,
        });
    }

    /// Return the staged-artifact facts from a `Completed` envelope. The
    /// Host validates and promotes them; this transport boundary never
    /// publishes a Derived Result.
    fn complete_with_artifact_facts(
        &mut self,
        request_id: String,
        result: Value,
        request: &Request,
        started: Instant,
        last_progress: Option<Progress>,
    ) -> SupervisorOutcome {
        if request_id != request.request_id {
            self.last_artifact_error = Some("completed_request_id_mismatch".to_string());
            self.discard_stage();
            let context = TerminationContext {
                last_progress,
                last_artifact_error: self.last_artifact_error.take(),
                failed: None,
            };
            return self.cooperative_termination_outcome(
                request.request_id.clone(),
                "protocol_violation:completed_request_id_mismatch".to_string(),
                started,
                context,
            );
        }
        if let Err(error) = self.host.finish_terminal() {
            self.discard_stage();
            let context = TerminationContext {
                last_progress,
                last_artifact_error: self.last_artifact_error.take(),
                failed: None,
            };
            return SupervisorOutcome::ForceTerminated {
                record: self.termination_record(
                    request_id,
                    format!("completed_terminal_finalize_failed:{error}"),
                    started,
                    context,
                    ExitKind::ForceAfterGrace,
                ),
            };
        }
        // Exit status is diagnostic context, not a completion gate. The
        // protocol terminal envelope and the validated stream/reap state are
        // authoritative; a worker may report a useful result before exiting
        // with a domain-specific status code.
        if let Some(outcome) =
            self.verify_clean_exit(&request_id, started, "completed_reap", last_progress)
        {
            self.discard_stage();
            return outcome;
        }
        SupervisorOutcome::Completed {
            request_id,
            result,
            artifact_headers: std::mem::take(&mut self.artifact_headers),
        }
    }

    /// Verifies that no stream overflow occurred after a cooperative terminal
    /// envelope. Exit codes and signals remain available through the host's
    /// structured diagnostics, but are not completion gates.
    fn verify_clean_exit(
        &mut self,
        request_id: &str,
        started: Instant,
        stage_prefix: &str,
        last_progress: Option<Progress>,
    ) -> Option<SupervisorOutcome> {
        // A stream overflow is checked independently of the exit status:
        // a worker that exits cleanly (code 0) after flooding a stream
        // must still fail the terminal outcome closed.
        let overflow = self.host.stream_overflowed();
        let unclean = overflow.map(|stream| format!("{stage_prefix}:stream_overflow:{stream}"));
        unclean.map(|stage| {
            let context = TerminationContext {
                last_progress,
                last_artifact_error: self.last_artifact_error.take(),
                failed: None,
            };
            SupervisorOutcome::ForceTerminated {
                record: self.termination_record(
                    request_id.to_string(),
                    stage,
                    started,
                    context,
                    ExitKind::ForceAfterGrace,
                ),
            }
        })
    }

    fn cooperative_termination_outcome(
        &mut self,
        request_id: String,
        stage: String,
        started: Instant,
        context: TerminationContext,
    ) -> SupervisorOutcome {
        let termination_error = self.host.finish_terminal().err();
        let reaped = termination_error.is_none();
        SupervisorOutcome::ForceTerminated {
            record: self.termination_record(
                request_id,
                match termination_error {
                    Some(error) => format!("{stage}_reap_failed:{error}"),
                    None => stage,
                },
                started,
                context,
                if reaped {
                    ExitKind::Cooperative
                } else {
                    ExitKind::ForceAfterGrace
                },
            ),
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
        let termination_error = self.host.terminate().err();
        let context = TerminationContext {
            last_progress,
            last_artifact_error: self.last_artifact_error.take(),
            failed: None,
        };
        SupervisorOutcome::ForceTerminated {
            record: self.termination_record(
                request_id.to_string(),
                match termination_error {
                    Some(error) => format!("{stage_label}:terminate_failed:{error}"),
                    None => stage_label,
                },
                started,
                context,
                ExitKind::ForceAfterGrace,
            ),
        }
    }

    /// Assemble a structured terminal record, copying the worker's
    /// observed exit signal and bounded stderr tail from the host so
    /// signal-based exits keep their diagnostic context.
    fn termination_record(
        &mut self,
        request_id: String,
        stage: String,
        started: Instant,
        context: TerminationContext,
        exit_kind: ExitKind,
    ) -> TerminationRecord {
        let TerminationContext {
            last_progress,
            last_artifact_error,
            failed,
        } = context;
        let (failed_code, failed_detail) = match failed {
            Some(FailedFields { code, detail }) => (Some(code), Some(detail)),
            None => (None, None),
        };
        TerminationRecord {
            request_id,
            stage,
            elapsed: started.elapsed(),
            last_progress,
            last_artifact_error,
            exit_signal: self.host.exit_signal(),
            exit_code: self.host.exit_code(),
            stderr_tail: self.host.stderr_tail(),
            failed_code,
            failed_detail,
            exit_kind,
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
            // An exhausted script behaves like the real transport's
            // post-deadline receive: the worker has gone silent.
            self.script
                .pop_front()
                .unwrap_or(Err(WorkerError::TimedOut))
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
    fn cooperative_cancel_reaps_the_worker() {
        let (worker, terminated) = ScriptedWorker::with_results(vec![Ok(Envelope::Cancelled {
            schema_version: crate::schema_version().to_string(),
            request_id: "req-1".to_string(),
            reason: "stopped".to_string(),
        })]);
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        assert!(matches!(
            supervisor.cancel("req-1", "stop"),
            SupervisorOutcome::Acknowledged { .. }
        ));
        assert_eq!(*terminated.lock().expect("termination log mutex"), 1);
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
                    record.stage.starts_with("cancel_grace_exceeded")
                        || record.stage.starts_with("worker_closed"),
                    "force-terminate stage should be cancel_grace_exceeded or worker_closed; got {:?}",
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
            SupervisorOutcome::Completed {
                request_id,
                artifact_headers,
                ..
            } => {
                assert_eq!(request_id, "req-1");
                assert!(artifact_headers.is_empty());
            }
            other => panic!("expected Completed; got {other:?}"),
        }
    }

    #[test]
    fn completed_request_reaps_the_worker() {
        let (worker, terminated) = ScriptedWorker::with_results(vec![
            Ok(ready_envelope()),
            Ok(Envelope::Completed {
                schema_version: crate::schema_version().to_string(),
                request_id: "req-1".to_string(),
                result: serde_json::json!({}),
            }),
        ]);
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        assert!(matches!(
            supervisor.request(sample_request()),
            SupervisorOutcome::Completed { .. }
        ));
        assert_eq!(*terminated.lock().expect("termination log mutex"), 1);
    }

    #[test]
    fn request_failure_reaps_the_worker() {
        let (worker, terminated) = ScriptedWorker::with_results(vec![
            Ok(ready_envelope()),
            Ok(Envelope::Failed {
                schema_version: crate::schema_version().to_string(),
                request_id: "req-1".to_string(),
                code: "worker_failed".to_string(),
                detail: "unrecoverable".to_string(),
            }),
        ]);
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request())
        else {
            panic!("expected failed worker outcome");
        };
        assert_eq!(record.stage, "failed:worker_failed:unrecoverable");
        assert_eq!(record.exit_kind, ExitKind::Cooperative);
        assert_eq!(*terminated.lock().expect("termination log mutex"), 1);
    }

    #[test]
    fn completed_request_id_mismatch_reaps_the_worker() {
        let (worker, terminated) = ScriptedWorker::with_results(vec![
            Ok(ready_envelope()),
            Ok(Envelope::Completed {
                schema_version: crate::schema_version().to_string(),
                request_id: "wrong-request".to_string(),
                result: serde_json::json!({}),
            }),
        ]);
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request())
        else {
            panic!("expected mismatched completion outcome");
        };
        assert_eq!(
            record.stage,
            "protocol_violation:completed_request_id_mismatch"
        );
        assert_eq!(*terminated.lock().expect("termination log mutex"), 1);
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
                assert_eq!(record.request_id, "req-1");
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
    fn request_does_not_dispatch_after_a_late_handshake() {
        struct LateHandshakeWorker {
            sent: Arc<Mutex<usize>>,
            terminated: Arc<Mutex<usize>>,
        }

        impl WorkerHost for LateHandshakeWorker {
            fn send(&mut self, _: &Envelope) -> Result<(), WorkerError> {
                *self.sent.lock().expect("send count mutex") += 1;
                Ok(())
            }

            fn recv(&mut self, _: Instant) -> Result<Envelope, WorkerError> {
                Ok(ready_envelope())
            }

            fn cancel(&mut self, _: &str, _: &str) -> Result<(), WorkerError> {
                Ok(())
            }

            fn terminate(&mut self) -> Result<(), WorkerError> {
                *self.terminated.lock().expect("termination log mutex") += 1;
                Ok(())
            }
        }

        let sent = Arc::new(Mutex::new(0));
        let terminated = Arc::new(Mutex::new(0));
        let worker = LateHandshakeWorker {
            sent: Arc::clone(&sent),
            terminated: Arc::clone(&terminated),
        };
        let mut supervisor = Supervisor::new(Duration::ZERO, Box::new(worker), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.request(sample_request())
        else {
            panic!("expected force termination");
        };
        assert_eq!(record.stage, "handshake_grace_exceeded");
        assert_eq!(*sent.lock().expect("send count mutex"), 0);
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
    fn framed_transport_does_not_consume_a_buffered_handshake_after_grace() {
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
        assert_eq!(record.stage, "handshake_grace_exceeded");
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
    fn cancel_checks_deadline_after_a_mismatched_acknowledgement() {
        struct MismatchedAcknowledgementWorker {
            responses: VecDeque<Result<Envelope, WorkerError>>,
            recv_calls: Arc<Mutex<usize>>,
            terminated: Arc<Mutex<usize>>,
        }

        impl WorkerHost for MismatchedAcknowledgementWorker {
            fn send(&mut self, _: &Envelope) -> Result<(), WorkerError> {
                Ok(())
            }

            fn recv(&mut self, _: Instant) -> Result<Envelope, WorkerError> {
                *self.recv_calls.lock().expect("receive count mutex") += 1;
                self.responses
                    .pop_front()
                    .unwrap_or(Err(WorkerError::TimedOut))
            }

            fn cancel(&mut self, _: &str, _: &str) -> Result<(), WorkerError> {
                Ok(())
            }

            fn terminate(&mut self) -> Result<(), WorkerError> {
                *self.terminated.lock().expect("termination log mutex") += 1;
                Ok(())
            }
        }

        let recv_calls = Arc::new(Mutex::new(0));
        let terminated = Arc::new(Mutex::new(0));
        let worker = MismatchedAcknowledgementWorker {
            responses: vec![
                Ok(Envelope::Cancelled {
                    schema_version: crate::schema_version().to_string(),
                    request_id: "other-request".to_string(),
                    reason: "not this cancellation".to_string(),
                }),
                Err(WorkerError::TimedOut),
            ]
            .into(),
            recv_calls: Arc::clone(&recv_calls),
            terminated: Arc::clone(&terminated),
        };
        let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(worker), None);

        let SupervisorOutcome::ForceTerminated { record } = supervisor.cancel("req-1", "stop")
        else {
            panic!("expected force termination");
        };
        assert!(
            record
                .stage
                .starts_with("protocol_violation:mismatched_request_id:"),
            "a foreign acknowledgement must fail cancellation closed; got {:?}",
            record.stage
        );
        assert_eq!(*recv_calls.lock().expect("receive count mutex"), 1);
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
        // The unsolicited Cancelled fails the request closed immediately;
        // it is never classified as a cooperative ack.
        match outcome {
            SupervisorOutcome::ForceTerminated { record } => {
                assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
                assert!(
                    record
                        .stage
                        .starts_with("protocol_violation:unsolicited_cancelled:"),
                    "expected protocol_violation:unsolicited_cancelled: stage; got {:?}",
                    record.stage
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

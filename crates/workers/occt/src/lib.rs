//! OCCT geometry worker boundary.
//!
//! The C++ worker binary is built by `build.rs` against the system OCCT
//! install (or a `THREETERM_OCCT_DIR` override). The Rust side of this
//! crate exposes:
//!
//! * [`schema_version`] — the pinned worker protocol schema.
//! * [`ExtrudeRequest`], [`ExtrudeResult`], [`BooleanFuseRequest`],
//!   [`BooleanFuseResult`], [`Operation`], [`RevolveRequest`],
//!   [`RevolveResult`], [`MirrorRequest`], [`MirrorResult`],
//!   [`ShellRequest`], [`ShellResult`], [`DraftRequest`],
//!   [`DraftResult`] — the JSON envelopes exchanged with the worker,
//!   with `serde(deny_unknown_fields)` to fail closed on unexpected
//!   fields.
//! * [`OcctWorker`] — the boundary struct that spawns the worker
//!   binary, pipes the request in, reads the response, and returns
//!   either a typed result or an [`OcctDiagnostic`].
//!
//! The worker binary lives at `OUT_DIR/bin/threeterm-occt-worker` for
//! the running build. Tests can override the location through
//! `OcctWorker::with_binary_path` or by setting the
//! `THREETERM_OCCTBUILD_WORKER` environment variable when cargo
//! provides the path through the build script.

use std::env;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use threeterm_protocol::supervisor::{
    CancellationGracePolicy, Progress, Request as SupervisorRequest, Supervisor, SupervisorOutcome,
    TerminationRecord,
};
use threeterm_protocol::worker::{
    SubprocessWorkerHost, WorkerConfig, WorkerError as ProtocolWorkerError, WorkerHost,
    WorkerProcess,
};

pub mod envelope;
pub use envelope::{
    BooleanFuseRequest, BooleanFuseResult, BooleanPatternRequest, BooleanPatternResult,
    ChamferRequest, ChamferResult, CircularPatternRequest, CircularPatternResult, DraftRequest,
    DraftResult, ExtrudeRequest, ExtrudeResult, FilletRequest, FilletResult, HoleRequest,
    HoleResult, LinearPatternRequest, LinearPatternResult, LoftRequest, LoftResult, MirrorRequest,
    MirrorResult, Operation, RevolveRequest, RevolveResult, SCHEMA_VERSION, ShellRequest,
    ShellResult,
};

pub fn schema_version() -> &'static str {
    SCHEMA_VERSION
}

#[doc(hidden)]
pub const BUILT_WORKER_PATH: &str = include_str!(concat!(env!("OUT_DIR"), "/worker_path.txt"));

/// Worker-boundary diagnostic. The shape mirrors
/// `protocol::diagnostic::Diagnostic` so the host can convert without
/// losing information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcctDiagnostic {
    pub code: String,
    pub arg: String,
    pub schema_version: String,
}

impl OcctDiagnostic {
    pub fn new(code: impl Into<String>, arg: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            arg: arg.into(),
            schema_version: SCHEMA_VERSION.to_string(),
        }
    }
}

/// Errors that prevent the worker from returning a typed result.
#[derive(Debug)]
pub enum WorkerError {
    /// The worker binary could not be located or spawned.
    Spawn {
        binary: PathBuf,
        detail: String,
        /// The active request ID when spawning was attempted for a request.
        request_id: Option<String>,
    },
    /// The worker exited with a non-zero status.
    NonZeroExit { code: Option<i32>, stderr: String },
    /// A non-zero worker exit with a safely recovered active request ID.
    NonZeroExitWithContext {
        request_id: String,
        code: Option<i32>,
        stderr: String,
    },
    /// The worker exited due to a signal.
    Signalled { signal: i32, stderr: String },
    /// A signal-bearing worker exit with a safely recovered active request ID.
    SignalledWithContext {
        request_id: String,
        signal: i32,
        stderr: String,
    },
    /// The worker emitted output that is not valid JSON or not a parseable
    /// envelope.
    Malformed { detail: String },
    /// A malformed result after the active request identity was safely
    /// extracted.
    MalformedWithContext { request_id: String, detail: String },
    /// The worker emitted a JSON diagnostic instead of a response.
    Diagnostic(OcctDiagnostic),
    /// A worker diagnostic with a safely recovered active request ID.
    DiagnosticWithContext {
        request_id: String,
        diagnostic: OcctDiagnostic,
    },
    /// The request was cooperatively cancelled and the worker
    /// acknowledged the cancellation inside the grace period.
    /// `last_progress`, `elapsed`, `stderr_tail` and exit facts are
    /// retained so the diagnostic surface keeps cancellation context.
    Cancelled {
        request_id: String,
        reason: String,
        last_progress: Option<threeterm_protocol::supervisor::Progress>,
        elapsed: std::time::Duration,
        stderr_tail: String,
        exit_signal: Option<i32>,
        exit_code: Option<i32>,
    },
    /// The supervised lifecycle ended with a structured termination
    /// record that does not map to a typed failure: the record's stage,
    /// elapsed time, last progress, and stderr tail are preserved so
    /// callers retain the diagnostic context.
    Supervised { record: Box<TerminationRecord> },
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { binary, detail, .. } => {
                write!(
                    formatter,
                    "worker spawn failed at {}: {detail}",
                    binary.display()
                )
            }
            Self::NonZeroExit { code, stderr } => {
                write!(formatter, "worker exited with code {code:?}: {stderr}")
            }
            Self::NonZeroExitWithContext {
                request_id,
                code,
                stderr,
            } => write!(
                formatter,
                "worker request {request_id} exited with code {code:?}: {stderr}"
            ),
            Self::Signalled { signal, stderr } => {
                write!(formatter, "worker signalled with {signal}: {stderr}")
            }
            Self::SignalledWithContext {
                request_id,
                signal,
                stderr,
            } => write!(
                formatter,
                "worker request {request_id} signalled with {signal}: {stderr}"
            ),
            Self::Malformed { detail } => {
                write!(formatter, "malformed worker output: {detail}")
            }
            Self::MalformedWithContext { request_id, detail } => write!(
                formatter,
                "malformed worker output for request {request_id}: {detail}"
            ),
            Self::Diagnostic(diagnostic) => write!(
                formatter,
                "worker diagnostic {} {}: {}",
                diagnostic.code, diagnostic.arg, diagnostic.schema_version
            ),
            Self::DiagnosticWithContext {
                request_id,
                diagnostic,
            } => write!(
                formatter,
                "worker diagnostic for request {request_id} {} {}: {}",
                diagnostic.code, diagnostic.arg, diagnostic.schema_version
            ),
            Self::Cancelled {
                request_id,
                reason,
                last_progress,
                elapsed,
                ..
            } => {
                write!(
                    formatter,
                    "worker request {request_id} cancelled after {elapsed:?} reason={reason:?} last_progress={last_progress:?}"
                )
            }
            Self::Supervised { record } => {
                write!(
                    formatter,
                    "supervised worker termination at stage {:?} after {:?}",
                    record.stage, record.elapsed
                )
            }
        }
    }
}

impl std::error::Error for WorkerError {}

/// Process-backed OCCT geometry worker. Owns the binary path and
/// exposes `extrude`, `boolean_fuse`, `fillet`, `chamfer`, `hole`,
/// `revolve`, `mirror`, `linear_pattern`, `circular_pattern`, `shell`,
/// and `draft`.
///
/// The worker is **disposable**: each call spawns a fresh supervised
/// worker process in its own process group, negotiates the versioned
/// protocol handshake, pipes the request envelope in, and maps the
/// supervised outcome to a typed result or structured failure. The
/// worker has no persistent state.
#[derive(Debug, Clone)]
pub struct OcctWorker {
    binary_path: PathBuf,
    /// Supervisor grace period: the worker must complete the handshake
    /// and the request inside this deadline or it is force-terminated.
    grace: Duration,
    cancellation_grace: CancellationGracePolicy,
    /// Revision Snapshot the host authorized for the request. The worker
    /// protocol carries this outer identity even though the typed OCCT
    /// arguments remain operation-specific.
    revision_id: Option<String>,
    /// Expected worker identity negotiated during the protocol handshake.
    expected_worker_id: Option<String>,
}

/// Default supervisor grace for OCCT operations. Operations complete in
/// well under a second; this bound catches hangs without harming
/// legitimate geometry work.
pub const DEFAULT_SUPERVISOR_GRACE: Duration = Duration::from_secs(30);
pub const DEFAULT_CANCELLATION_GRACE: Duration = Duration::from_millis(250);
pub const BOOLEAN_PATTERN_CANCELLATION_GRACE: Duration = Duration::from_millis(100);

impl OcctWorker {
    /// Locate the worker binary. Prefers the path embedded at build
    /// time (the `OUT_DIR/bin/threeterm-occt-worker` produced by
    /// `build.rs`), then the `THREETERM_OCCTBUILD_WORKER` environment
    /// variable, and finally the `target/<profile>/bin/` heuristics.
    pub fn locate() -> Result<Self, WorkerError> {
        let built = PathBuf::from(BUILT_WORKER_PATH.trim());
        if built.is_file() {
            return Ok(Self::with_binary_path(built).with_expected_worker_id("occt"));
        }
        if let Some(path) = env::var_os("THREETERM_OCCTBUILD_WORKER") {
            let candidate = PathBuf::from(path);
            if candidate.is_file() {
                return Ok(Self::with_binary_path(candidate).with_expected_worker_id("occt"));
            }
        }
        let target_root = env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .or_else(|| env::current_dir().ok().map(|cwd| cwd.join("target")))
            .ok_or_else(|| WorkerError::Spawn {
                binary: PathBuf::from("threeterm-occt-worker"),
                detail: "could not determine target directory".to_string(),
                request_id: None,
            })?;
        for profile in ["debug", "release"] {
            let candidate = target_root.join(profile).join("bin/threeterm-occt-worker");
            if candidate.is_file() {
                return Ok(Self::with_binary_path(candidate).with_expected_worker_id("occt"));
            }
        }
        Err(WorkerError::Spawn {
            binary: target_root.join("debug/bin/threeterm-occt-worker"),
            detail: "worker binary not found; build the occt worker first".to_string(),
            request_id: None,
        })
    }

    pub fn with_binary_path(binary_path: PathBuf) -> Self {
        Self {
            binary_path,
            grace: DEFAULT_SUPERVISOR_GRACE,
            cancellation_grace: CancellationGracePolicy::new(DEFAULT_CANCELLATION_GRACE)
                .with_operation("boolean_pattern", BOOLEAN_PATTERN_CANCELLATION_GRACE),
            revision_id: None,
            expected_worker_id: None,
        }
    }

    /// Override the supervisor grace period (deadline) for every
    /// operation this worker executes.
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// Override the cooperative cancellation grace for one OCCT operation.
    pub fn with_operation_grace(mut self, operation: Operation, grace: Duration) -> Self {
        self.cancellation_grace = self
            .cancellation_grace
            .clone()
            .with_operation(operation.as_str(), grace);
        self
    }

    /// Bind subsequent requests to a host-owned Revision Snapshot.
    pub fn with_revision_id(mut self, revision_id: impl Into<String>) -> Self {
        self.revision_id = Some(revision_id.into());
        self
    }

    /// Require a specific worker identity during handshake negotiation.
    pub fn with_expected_worker_id(mut self, worker_id: impl Into<String>) -> Self {
        self.expected_worker_id = Some(worker_id.into());
        self
    }

    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    /// Extrude `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn extrude(&self, request: &ExtrudeRequest) -> Result<ExtrudeResult, WorkerError> {
        let bytes = bounded_serialize(request, "extrude", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_extrude()
    }

    /// Extrude `request` with a cooperative cancellation token. The
    /// caller sets the token to request cancellation; the supervisor
    /// sends `Cancel`, waits the grace period for the worker's
    /// acknowledgement, and force-terminates the worker's process group
    /// if the worker does not cooperate.
    pub fn extrude_with_cancel(
        &self,
        request: &ExtrudeRequest,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<ExtrudeResult, WorkerError> {
        let bytes = bounded_serialize(request, "extrude", &request.request_id)?;
        let value = self.run_with_cancel(&bytes, cancel)?;
        // The cancellable path must run the same bounded, digest-verified
        // decoder as the synchronous path: oversized, symlinked, or
        // mismatched staged output fails closed before it can reach the
        // host commit path.
        RawResult {
            value,
            request_id: request.request_id.clone(),
            expected_output: expected_output_path(&request.output_dir, &request.output_filename),
        }
        .into_extrude()
    }

    /// Boolean-fuse `request` by spawning the worker process. See
    /// module docs for the disposable-worker contract.
    pub fn boolean_fuse(
        &self,
        request: &BooleanFuseRequest,
    ) -> Result<BooleanFuseResult, WorkerError> {
        let bytes = bounded_serialize(request, "boolean-fuse", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_boolean_fuse()
    }

    pub fn boolean_pattern(
        &self,
        request: &BooleanPatternRequest,
    ) -> Result<BooleanPatternResult, WorkerError> {
        let bytes = bounded_serialize(request, "boolean_pattern", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_boolean_pattern()
    }

    pub fn boolean_pattern_with_cancel(
        &self,
        request: &BooleanPatternRequest,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<BooleanPatternResult, WorkerError> {
        let mut ignore_progress = |_progress: &Progress| {};
        self.boolean_pattern_with_cancel_and_progress(request, cancel, &mut ignore_progress)
    }

    pub fn boolean_pattern_with_cancel_and_progress(
        &self,
        request: &BooleanPatternRequest,
        cancel: &std::sync::atomic::AtomicBool,
        on_progress: &mut dyn FnMut(&Progress),
    ) -> Result<BooleanPatternResult, WorkerError> {
        let bytes = bounded_serialize(request, "boolean_pattern", &request.request_id)?;
        let value = self.run_with_cancel_and_progress(&bytes, cancel, on_progress)?;
        RawResult {
            value,
            request_id: request.request_id.clone(),
            expected_output: expected_output_path(&request.output_dir, &request.output_filename),
        }
        .into_boolean_pattern()
    }

    /// Fillet `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn fillet(&self, request: &FilletRequest) -> Result<FilletResult, WorkerError> {
        let bytes = bounded_serialize(request, "fillet", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_fillet()
    }

    /// Chamfer `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn chamfer(&self, request: &ChamferRequest) -> Result<ChamferResult, WorkerError> {
        let bytes = bounded_serialize(request, "chamfer", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_chamfer()
    }

    /// Hole `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn hole(&self, request: &HoleRequest) -> Result<HoleResult, WorkerError> {
        let bytes = bounded_serialize(request, "hole", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_hole()
    }

    /// Revolve `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn revolve(&self, request: &RevolveRequest) -> Result<RevolveResult, WorkerError> {
        let bytes = bounded_serialize(request, "revolve", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_revolve()
    }

    /// Mirror `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn mirror(&self, request: &MirrorRequest) -> Result<MirrorResult, WorkerError> {
        let bytes = bounded_serialize(request, "mirror", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_mirror()
    }

    /// Linear pattern `request` by spawning the worker process. See
    /// module docs for the disposable-worker contract.
    pub fn linear_pattern(
        &self,
        request: &LinearPatternRequest,
    ) -> Result<LinearPatternResult, WorkerError> {
        let bytes = bounded_serialize(request, "linear_pattern", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_linear_pattern()
    }

    /// Circular pattern `request` by spawning the worker process. See
    /// module docs for the disposable-worker contract.
    pub fn circular_pattern(
        &self,
        request: &CircularPatternRequest,
    ) -> Result<CircularPatternResult, WorkerError> {
        let bytes = bounded_serialize(request, "circular_pattern", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_circular_pattern()
    }

    /// Shell `request` by spawning the worker process. See module docs
    /// for the disposable-worker contract.
    pub fn shell(&self, request: &ShellRequest) -> Result<ShellResult, WorkerError> {
        let bytes = bounded_serialize(request, "shell", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_shell()
    }

    /// Draft `request` by spawning the worker process. See module docs
    /// for the disposable-worker contract.
    pub fn draft(&self, request: &DraftRequest) -> Result<DraftResult, WorkerError> {
        let bytes = bounded_serialize(request, "draft", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_draft()
    }

    /// Loft `request` by spawning the worker process. See module docs
    /// for the disposable-worker contract.
    pub fn loft(&self, request: &LoftRequest) -> Result<LoftResult, WorkerError> {
        let bytes = bounded_serialize(request, "loft", &request.request_id)?;
        self.invoke(
            &bytes,
            expected_output_path(&request.output_dir, &request.output_filename),
        )?
        .into_loft()
    }

    fn invoke(
        &self,
        envelope: &[u8],
        expected_output: Option<PathBuf>,
    ) -> Result<RawResult, WorkerError> {
        // Every production operation flows through the same cancellable
        // supervised path; the synchronous variants carry a never-set
        // token, so the deadline expiry remains the hard stop.
        let cancel = std::sync::atomic::AtomicBool::new(false);
        self.run_with_cancel(envelope, &cancel)
            .map(|value| RawResult {
                value,
                request_id: request_id_from_envelope(envelope),
                expected_output,
            })
    }

    /// Run `envelope` through the supervised lifecycle with a
    /// cooperative cancellation token. The token's `cancel()` triggers
    /// the supervisor's cancellation lifecycle: the worker receives a
    /// `Cancel` envelope and, if it does not acknowledge inside the
    /// grace period, is force-terminated and reaped with its process
    /// group. The flag is polled every receive slice (50 ms), so an
    /// in-flight operation observes the token well before the deadline.
    /// Returns the raw typed-result JSON; typed callers parse it into
    /// the operation-specific result.
    pub fn run_with_cancel(
        &self,
        envelope: &[u8],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<serde_json::Value, WorkerError> {
        let mut ignore_progress = |_progress: &Progress| {};
        self.run_with_cancel_and_progress(envelope, cancel, &mut ignore_progress)
    }

    pub fn run_with_cancel_and_progress(
        &self,
        envelope: &[u8],
        cancel: &std::sync::atomic::AtomicBool,
        on_progress: &mut dyn FnMut(&Progress),
    ) -> Result<serde_json::Value, WorkerError> {
        // Reject the raw input length before any JSON materialization: an
        // oversized request must never be parsed into a `serde_json::Value`
        // past the protocol's input bound.
        if envelope.len() > threeterm_protocol::frame::MAX_FRAME_BUFFER {
            let bounded_request_id = bounded_request_id_hint(envelope);
            return Err(malformed_for_request(
                &bounded_request_id,
                format!(
                    "request envelope of {} bytes exceeds the {} byte input bound",
                    envelope.len(),
                    threeterm_protocol::frame::MAX_FRAME_BUFFER
                ),
            ));
        }
        let request_id = request_id_from_envelope(envelope);
        let args: serde_json::Value = serde_json::from_slice(envelope).map_err(|error| {
            malformed_for_request(
                &request_id,
                format!("request serialization failed: {error}"),
            )
        })?;
        let request_id = args
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if request_id.is_empty() {
            return Err(malformed_for_request(
                &request_id,
                "request envelope is missing request_id",
            ));
        }
        let command_id = args
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if command_id.is_empty() {
            return Err(malformed_for_request(
                &request_id,
                "request envelope is missing operation",
            ));
        }
        let feature_id = args
            .get("feature_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let output_dir = args
            .get("output_dir")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let output_filename = args
            .get("output_filename")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let expected_output = if output_dir.is_empty() || output_filename.is_empty() {
            None
        } else {
            Some(Path::new(output_dir).join(output_filename))
        };

        let host = <Self as WorkerProcess>::spawn(WorkerConfig {
            worker_id: "occt",
            schema_version: threeterm_protocol::schema_version(),
            command_line: vec![self.binary_path.display().to_string()],
        })
        .map_err(|error| WorkerError::Spawn {
            binary: self.binary_path.clone(),
            detail: error.to_string(),
            request_id: Some(request_id.clone()),
        })?;
        let mut supervisor = Supervisor::new(self.grace, host, None)
            .with_cancellation_grace_policy(self.cancellation_grace.clone());
        if let Some(worker_id) = &self.expected_worker_id {
            supervisor = supervisor.with_expected_worker_id(worker_id.clone());
        }
        let outcome = supervisor.request_with_cancel_and_progress(
            SupervisorRequest {
                request_id: request_id.clone(),
                command_id: command_id.clone(),
                args,
                revision_id: self.revision_id.clone().unwrap_or_default(),
            },
            cancel,
            on_progress,
        );
        let cleanup_path = expected_output.clone();
        let mapped = map_outcome(
            outcome,
            &request_id,
            &command_id,
            &feature_id,
            expected_output,
        );
        let cleanup_required = mapped.as_ref().map_or(true, |result| {
            result
                .value
                .get("status")
                .and_then(serde_json::Value::as_str)
                != Some("ok")
        });
        if cleanup_required && let Some(path) = cleanup_path {
            cleanup_worker_output(&path);
        }
        mapped.map(|result| result.value)
    }
}

/// Maps a supervised outcome to the typed-result boundary: a completed
/// request carries the typed result JSON, a cooperative `Failed`
/// envelope becomes an [`OcctDiagnostic`], a signal-based exit keeps the
/// actual signal, and everything else fails closed.
///
/// The typed result is bound to the active request: its inner
/// `request_id`, `schema_version`, and `operation` must match the
/// request that was sent, or the completion fails closed.
fn map_outcome(
    outcome: SupervisorOutcome,
    request_id: &str,
    command_id: &str,
    expected_feature_id: &str,
    expected_output: Option<PathBuf>,
) -> Result<RawResult, WorkerError> {
    match outcome {
        SupervisorOutcome::Completed { result, .. } => {
            let result_request_id = result
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let result_schema = result
                .get("schema_version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let result_operation = result
                .get("operation")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if result_request_id != request_id {
                return Err(WorkerError::MalformedWithContext {
                    request_id: request_id.to_string(),
                    detail: format!(
                        "completed result is bound to {result_request_id:?}, expected {request_id:?}"
                    ),
                });
            }
            if result_schema != SCHEMA_VERSION {
                return Err(WorkerError::MalformedWithContext {
                    request_id: request_id.to_string(),
                    detail: format!(
                        "completed result schema {result_schema:?}, expected {SCHEMA_VERSION:?}"
                    ),
                });
            }
            if result_operation != command_id {
                return Err(WorkerError::MalformedWithContext {
                    request_id: request_id.to_string(),
                    detail: format!(
                        "completed result operation {result_operation:?}, expected {command_id:?}"
                    ),
                });
            }
            let result_feature_id = result
                .get("feature_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if !expected_feature_id.is_empty() && result_feature_id != expected_feature_id {
                return Err(WorkerError::MalformedWithContext {
                    request_id: request_id.to_string(),
                    detail: format!(
                        "completed result feature_id {result_feature_id:?}, expected {expected_feature_id:?}"
                    ),
                });
            }
            Ok(RawResult {
                value: result,
                request_id: request_id.to_string(),
                expected_output,
            })
        }
        SupervisorOutcome::Acknowledged {
            request_id,
            reason,
            elapsed,
            last_progress,
            stderr_tail,
            exit_signal,
            exit_code,
        } => Err(WorkerError::Cancelled {
            request_id,
            reason,
            last_progress,
            elapsed,
            stderr_tail,
            exit_signal,
            exit_code,
        }),
        SupervisorOutcome::ForceTerminated { record } => {
            if let (Some(code), Some(detail)) =
                (record.failed_code.clone(), record.failed_detail.clone())
            {
                // Keep the structured termination facts when a domain failure
                // is followed by a signal-bearing worker termination.
                if record.exit_signal.is_none() {
                    return Err(WorkerError::DiagnosticWithContext {
                        request_id: record.request_id.clone(),
                        diagnostic: OcctDiagnostic::new(code, detail),
                    });
                }
            }
            if record.stage.starts_with("handshake_schema_mismatch")
                || record.stage.starts_with("handshake_worker_id_mismatch")
                || record.stage.starts_with("envelope_schema_mismatch")
            {
                return Err(WorkerError::MalformedWithContext {
                    request_id: record.request_id.clone(),
                    detail: record.stage,
                });
            }
            // A worker that closes its stream before a terminal envelope is
            // an actual process failure, not merely a generic supervision
            // timeout. Preserve the reaped exit facts at the typed boundary.
            let worker_closed = record.stage.starts_with("worker_closed")
                || record.stage.starts_with("handshake_worker_closed");
            if worker_closed {
                if let Some(signal) = record.exit_signal {
                    return Err(WorkerError::SignalledWithContext {
                        request_id: record.request_id.clone(),
                        signal,
                        stderr: record.stderr_tail.clone(),
                    });
                }
                if let Some(code) = record.exit_code
                    && code != 0
                {
                    return Err(WorkerError::NonZeroExitWithContext {
                        request_id: record.request_id.clone(),
                        code: Some(code),
                        stderr: record.stderr_tail.clone(),
                    });
                }
            }
            // Preserve the structured termination context: request id,
            // stage, elapsed time, last progress, artifact errors, and
            // stderr tail all remain available to callers.
            Err(WorkerError::Supervised {
                record: Box::new(record),
            })
        }
    }
}

/// Serializes a typed request through a capped writer so an oversized
/// request fails during encoding instead of being fully materialized
/// in memory before the protocol's input bound rejects it.
fn bounded_serialize<T: serde::Serialize>(
    request: &T,
    operation: &str,
    request_id: &str,
) -> Result<Vec<u8>, WorkerError> {
    threeterm_protocol::worker::serialize_capped(
        request,
        threeterm_protocol::frame::MAX_FRAME_BUFFER,
    )
    .map_err(|error| {
        malformed_for_request(
            request_id,
            format!("{operation} request serialization failed: {error}"),
        )
    })
}

fn request_id_from_envelope(envelope: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(envelope)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Bounded request-ID hint for oversized envelopes. The hint scans only the
/// first 8 KiB of the raw envelope with a manual `request_id` search so a
/// hostile oversized payload is never materialized into a full
/// `serde_json::Value` before the input bound rejects it.
fn bounded_request_id_hint(envelope: &[u8]) -> String {
    const HINT_LIMIT: usize = 8192;
    const MAX_ID_LEN: usize = 512;
    let scan_len = envelope.len().min(HINT_LIMIT);
    let scan = &envelope[..scan_len];
    let Ok(text) = std::str::from_utf8(scan) else {
        return String::new();
    };
    let Some(key_idx) = text.find("\"request_id\"") else {
        return String::new();
    };
    let after_key = &text[key_idx + "\"request_id\"".len()..];
    let Some(colon_idx) = after_key.find(':') else {
        return String::new();
    };
    let after_colon = after_key[colon_idx + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return String::new();
    }
    let mut id = String::new();
    let mut escaped = false;
    for ch in after_colon[1..].chars() {
        if escaped {
            id.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            break;
        } else {
            id.push(ch);
        }
        if id.len() > MAX_ID_LEN {
            break;
        }
    }
    if id.is_empty() || id.len() > MAX_ID_LEN {
        String::new()
    } else {
        id
    }
}

fn malformed_for_request(request_id: &str, detail: impl Into<String>) -> WorkerError {
    if request_id.is_empty() {
        WorkerError::Malformed {
            detail: detail.into(),
        }
    } else {
        WorkerError::MalformedWithContext {
            request_id: request_id.to_string(),
            detail: detail.into(),
        }
    }
}

/// The private output location a request declares, when both parts are
/// present. The worker's advertised `brep_path` must match this.
fn expected_output_path(output_dir: &Path, output_filename: &str) -> Option<PathBuf> {
    if output_filename.is_empty() {
        return None;
    }
    Some(output_dir.join(output_filename))
}

/// Remove the final output and any interrupted sibling temporary files. The
/// C++ worker writes `<name>.tmp-<pid>` before its final rename; force
/// termination can interrupt that write before the worker can clean it.
fn cleanup_worker_output(path: &Path) {
    let _ = std::fs::remove_file(path);
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let temporary_prefix = format!("{file_name}.tmp-");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(&temporary_prefix))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Hex SHA-256 of a file's bytes, used to verify the worker's staged
/// artifact matches its advertised digest.
pub fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Production `WorkerProcess` wiring: spawns the OCCT binary in its own
/// process group with piped standard streams so the supervisor owns a
/// contained, reapable process tree.
impl WorkerProcess for OcctWorker {
    fn spawn(config: WorkerConfig) -> Result<Box<dyn WorkerHost>, ProtocolWorkerError> {
        let binary = config
            .command_line
            .first()
            .ok_or_else(|| ProtocolWorkerError::Io(std::io::Error::other("empty command line")))?;
        let child = Command::new(binary)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(ProtocolWorkerError::Io)?;
        SubprocessWorkerHost::new(child).map(|host| Box::new(host) as Box<dyn WorkerHost>)
    }
}

#[derive(Debug)]
struct RawResult {
    value: serde_json::Value,
    request_id: String,
    /// The private output location the request declared
    /// (`output_dir`/`output_filename`). When present, the worker's
    /// advertised `brep_path` must equal it: a worker pointing at any
    /// other file fails closed instead of letting the host promote
    /// foreign bytes.
    expected_output: Option<PathBuf>,
}

impl RawResult {
    /// Fail closed when the worker's staged output exceeds the staged
    /// artifact bound. The bound is enforced on the ACTUAL staged file
    /// (its on-disk size), not on the worker's advertised `brep_bytes`
    /// metadata, so a worker that under-reports its output cannot smuggle
    /// an oversized artifact past the host.
    fn bounded<T>(self) -> Result<T, WorkerError>
    where
        T: serde::de::DeserializeOwned,
    {
        let cleanup_path = self.expected_output.clone();
        let result = self.bounded_inner();
        if result.is_err()
            && let Some(path) = cleanup_path
        {
            cleanup_worker_output(&path);
        }
        result
    }

    fn bounded_inner<T>(self) -> Result<T, WorkerError>
    where
        T: serde::de::DeserializeOwned,
    {
        let request_id = self.request_id;
        let value = self.value;
        let expected_output = self.expected_output;
        let brep_bytes = value
            .get("brep_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let brep_path = value
            .get("brep_path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from);
        // The staged output must be the private location the request
        // declared: a worker advertising any other path fails closed so
        // a malformed or compromised worker can never direct the host to
        // promote foreign bytes.
        let Some(path) = brep_path.as_deref() else {
            return Err(malformed_for_request(
                &request_id,
                "worker response is missing brep_path",
            ));
        };
        if let Some(expected) = &expected_output
            && path != expected.as_path()
        {
            return Err(malformed_for_request(
                &request_id,
                format!(
                    "worker output at {path:?} is not the request's private output location {expected:?}"
                ),
            ));
        }
        // The staged output must exist as a regular file that is not a
        // symlink: a missing, dangling, or redirected path cannot be
        // verified and fails closed instead of trusting the
        // advertisement.
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            malformed_for_request(
                &request_id,
                format!("worker output at {path:?} could not be stat'd: {error}"),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(malformed_for_request(
                &request_id,
                format!("worker output at {path:?} must not be a symlink"),
            ));
        }
        if !metadata.is_file() {
            return Err(malformed_for_request(
                &request_id,
                format!("worker output at {path:?} is not a regular file"),
            ));
        }
        let actual_bytes = metadata.len();
        let bound = threeterm_protocol::worker::MAX_ARTIFACT_BYTES as u64;
        let largest = actual_bytes.max(brep_bytes);
        if largest > bound {
            return Err(malformed_for_request(
                &request_id,
                format!(
                    "worker staged output of {largest} bytes (advertised {brep_bytes}) exceeds the {bound} byte bound"
                ),
            ));
        }
        // The advertised byte count must equal the actual file size: an
        // under-reporting worker is treated as malformed rather than
        // being trusted on either side of the comparison.
        if actual_bytes != brep_bytes {
            return Err(malformed_for_request(
                &request_id,
                format!(
                    "worker staged output at {path:?} is {actual_bytes} bytes but advertises {brep_bytes}"
                ),
            ));
        }
        // Verify the staged file's SHA-256 digest matches the worker's
        // advertisement. A digest mismatch fails closed so a tampered
        // artifact can never reach the host's promotion path.
        let advertised = value.get("brep_sha256").and_then(serde_json::Value::as_str);
        if let Some(advertised) = advertised {
            let actual = crate::sha256_file(path).map_err(|error| {
                malformed_for_request(
                    &request_id,
                    format!("worker output at {path:?} could not be read: {error}"),
                )
            })?;
            if actual != advertised {
                return Err(malformed_for_request(
                    &request_id,
                    format!(
                        "worker output at {path:?} digest mismatch: advertised {advertised}, actual {actual}"
                    ),
                ));
            }
        }
        serde_json::from_value::<T>(value).map_err(|error| {
            malformed_for_request(
                &request_id,
                format!("worker response could not be parsed: {error}"),
            )
        })
    }

    fn into_extrude(self) -> Result<ExtrudeResult, WorkerError> {
        self.bounded()
    }

    fn into_boolean_fuse(self) -> Result<BooleanFuseResult, WorkerError> {
        self.bounded()
    }

    fn into_boolean_pattern(self) -> Result<BooleanPatternResult, WorkerError> {
        self.bounded()
    }

    fn into_fillet(self) -> Result<FilletResult, WorkerError> {
        self.bounded()
    }

    fn into_chamfer(self) -> Result<ChamferResult, WorkerError> {
        self.bounded()
    }

    fn into_hole(self) -> Result<HoleResult, WorkerError> {
        self.bounded()
    }

    fn into_revolve(self) -> Result<RevolveResult, WorkerError> {
        self.bounded()
    }

    fn into_mirror(self) -> Result<MirrorResult, WorkerError> {
        self.bounded()
    }

    fn into_linear_pattern(self) -> Result<LinearPatternResult, WorkerError> {
        self.bounded()
    }

    fn into_circular_pattern(self) -> Result<CircularPatternResult, WorkerError> {
        self.bounded()
    }

    fn into_shell(self) -> Result<ShellResult, WorkerError> {
        self.bounded()
    }

    fn into_draft(self) -> Result<DraftResult, WorkerError> {
        self.bounded()
    }

    fn into_loft(self) -> Result<LoftResult, WorkerError> {
        self.bounded()
    }
}

/// Helper for tests and consumers that need a deterministic request
/// id.
pub fn new_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("req-{nanos}-{}", std::process::id())
}

/// Parse a request from raw JSON. Used by tests that do not want the
/// typed builder.
pub fn parse_extrude_request(raw: &str) -> Result<ExtrudeRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_boolean_fuse_request(raw: &str) -> Result<BooleanFuseRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_fillet_request(raw: &str) -> Result<FilletRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_chamfer_request(raw: &str) -> Result<ChamferRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_hole_request(raw: &str) -> Result<HoleRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_revolve_request(raw: &str) -> Result<RevolveRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_mirror_request(raw: &str) -> Result<MirrorRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_linear_pattern_request(raw: &str) -> Result<LinearPatternRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_circular_pattern_request(
    raw: &str,
) -> Result<CircularPatternRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_shell_request(raw: &str) -> Result<ShellRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_draft_request(raw: &str) -> Result<DraftRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

pub fn parse_loft_request(raw: &str) -> Result<LoftRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

/// Stable ThreeTerm worker fingerprint for the OCCT kernel. Matches
/// the convention in the protocol's `Layer1` envelope so the host can
/// route OCCT-emitted artifacts through the same promotion path.
pub fn worker_fingerprint() -> threeterm_protocol::artifact::WorkerFingerprint {
    threeterm_protocol::artifact::WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: schema_version().to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    }
}

/// Stage a binary artifact in the host-managed staging directory and
/// return the `Envelope::Artifact` the worker would emit. The host
/// reuses this for direct OCCT operations that bypass the supervisor
/// (e.g. the extrude / Boolean-fuse commands in this slice).
pub fn emit_staged_artifact(
    artifact_root: impl AsRef<Path>,
    request: &threeterm_protocol::artifact::Layer1ArtifactRequest,
    bytes: &[u8],
) -> Result<threeterm_protocol::worker::Envelope, threeterm_protocol::artifact::ArtifactError> {
    use threeterm_protocol::artifact::{ArtifactHeader, Layer1CacheKey, Stage};
    use threeterm_protocol::worker::Envelope;

    let stage = Stage::open(artifact_root.as_ref())?;
    let staged = stage.stage_bytes(&request.staging_name, bytes)?;
    let fingerprint = worker_fingerprint();
    let cache_key = Layer1CacheKey::issue(request, &fingerprint);
    Ok(Envelope::Artifact {
        schema_version: threeterm_protocol::schema_version().to_string(),
        header: Box::new(ArtifactHeader {
            request_id: request.request_id.clone(),
            source_revision_id: request.source_revision_id.clone(),
            cache_key,
            worker_fingerprint: fingerprint,
            artifact_kind: request.artifact_kind.clone(),
            staging_name: staged.staging_name,
            byte_count: staged.byte_count,
            sha256: staged.sha256,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{BooleanFuseRequest, ExtrudeRequest};

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, "threeterm.workers.occt/1");
    }

    #[test]
    fn diagnostic_serializes_with_schema_version() {
        let diagnostic = OcctDiagnostic::new("request_malformed", "empty profile");
        let value = serde_json::to_value(&diagnostic).expect("diagnostic serializes");
        assert_eq!(value["code"], "request_malformed");
        assert_eq!(value["arg"], "empty profile");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn new_request_id_is_unique_per_call() {
        let first = new_request_id();
        let second = new_request_id();
        assert_ne!(first, second);
        assert!(first.starts_with("req-"));
    }

    #[test]
    fn envelope_helpers_emit_canonical_shape() {
        let request = ExtrudeRequest::new("req-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 1.0);
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["height"], 1.0);
    }

    #[test]
    fn envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "extrude",
            "profile": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            "height": 1.0,
            "rogue_key": true
        }"#;
        assert!(parse_extrude_request(raw).is_err());
    }

    #[test]
    fn boolean_fuse_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "boolean_fuse",
            "base_path": "/tmp/base.brep",
            "tool_path": "/tmp/tool.brep",
            "rogue_key": true
        }"#;
        assert!(parse_boolean_fuse_request(raw).is_err());
    }

    #[test]
    fn boolean_fuse_envelope_accepts_canonical_shape() {
        let request = BooleanFuseRequest::new("req-1", "/tmp/base.brep", "/tmp/tool.brep");
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "boolean_fuse");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["tool_path"], "/tmp/tool.brep");
    }

    #[test]
    fn extrude_result_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "extrude",
            "status": "ok",
            "brep_path": "/tmp/out.brep",
            "brep_sha256": "deadbeef",
            "brep_bytes": 42,
            "feature_id": "box-1",
            "rogue_key": true
        }"#;
        let result = serde_json::from_str::<ExtrudeResult>(raw);
        assert!(result.is_err(), "unknown key must be rejected");
    }

    #[test]
    fn revolve_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "revolve",
            "profile": [[0.0, 0.5], [1.0, 0.5], [1.0, -0.5]],
            "axis_point": [0.0, 0.5, 0.0],
            "axis_direction": [0.0, 1.0, 0.0],
            "angle": 6.283185307179586,
            "output_filename": "out.brep",
            "feature_id": "rev-1",
            "rogue_key": true
        }"#;
        assert!(parse_revolve_request(raw).is_err());
    }

    #[test]
    fn revolve_envelope_accepts_canonical_shape() {
        let request = RevolveRequest::new(
            "req-1",
            vec![(0.0, 0.5), (1.0, 0.5), (1.0, -0.5)],
            [0.0, 0.5, 0.0],
            [0.0, 1.0, 0.0],
            std::f64::consts::TAU,
        );
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "revolve");
        assert_eq!(value["axis_point"], serde_json::json!([0.0, 0.5, 0.0]));
        assert_eq!(value["axis_direction"], serde_json::json!([0.0, 1.0, 0.0]));
        assert_eq!(value["angle"], std::f64::consts::TAU);
    }

    #[test]
    fn mirror_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "mirror",
            "base_path": "/tmp/base.brep",
            "plane_point": [0.0, 0.0, 0.0],
            "plane_normal": [1.0, 0.0, 0.0],
            "output_filename": "out.brep",
            "feature_id": "mirror-1",
            "rogue_key": true
        }"#;
        assert!(parse_mirror_request(raw).is_err());
    }

    #[test]
    fn mirror_envelope_accepts_canonical_shape() {
        let request =
            MirrorRequest::new("req-1", "/tmp/base.brep", [0.0, 0.0, 0.0], [1.0, 0.0, 0.0])
                .with_feature_id("mirror-1");
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "mirror");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["plane_point"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(value["plane_normal"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(value["feature_id"], "mirror-1");
    }

    #[test]
    fn linear_pattern_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "linear_pattern",
            "base_path": "/tmp/base.brep",
            "direction": [1.0, 0.0, 0.0],
            "count": 3,
            "spacing": 2.0,
            "output_filename": "out.brep",
            "feature_id": "lin-1",
            "rogue_key": true
        }"#;
        assert!(parse_linear_pattern_request(raw).is_err());
    }

    #[test]
    fn linear_pattern_envelope_accepts_canonical_shape() {
        let request = LinearPatternRequest::new("req-1", "/tmp/base.brep", [1.0, 0.0, 0.0], 3, 2.0)
            .with_feature_id("lin-1");
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "linear_pattern");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["direction"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(value["count"], 3);
        assert_eq!(value["spacing"], 2.0);
        assert_eq!(value["feature_id"], "lin-1");
    }

    #[test]
    fn circular_pattern_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "circular_pattern",
            "base_path": "/tmp/base.brep",
            "axis_point": [0.0, 0.0, 0.0],
            "axis_normal": [0.0, 0.0, 1.0],
            "angle_step": 1.5707963267948966,
            "count": 4,
            "output_filename": "out.brep",
            "feature_id": "cir-1",
            "rogue_key": true
        }"#;
        assert!(parse_circular_pattern_request(raw).is_err());
    }

    #[test]
    fn circular_pattern_envelope_accepts_canonical_shape() {
        let request = CircularPatternRequest::new(
            "req-1",
            "/tmp/base.brep",
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            std::f64::consts::FRAC_PI_2,
            4,
        )
        .with_feature_id("cir-1");
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "circular_pattern");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["axis_point"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(value["axis_normal"], serde_json::json!([0.0, 0.0, 1.0]));
        assert_eq!(value["angle_step"], std::f64::consts::FRAC_PI_2);
        assert_eq!(value["count"], 4);
        assert_eq!(value["feature_id"], "cir-1");
    }

    #[test]
    fn shell_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "shell",
            "base_path": "/tmp/base.brep",
            "thickness": 0.5,
            "output_filename": "out.brep",
            "feature_id": "shell-1",
            "rogue_key": true
        }"#;
        assert!(parse_shell_request(raw).is_err());
    }

    #[test]
    fn shell_envelope_accepts_canonical_shape() {
        let request = ShellRequest::new("req-1", "/tmp/base.brep", 0.5).with_feature_id("shell-1");
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "shell");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["thickness"], 0.5);
        assert_eq!(value["feature_id"], "shell-1");
    }

    #[test]
    fn draft_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "draft",
            "base_path": "/tmp/base.brep",
            "angle": 0.2617993877991494,
            "pull_direction": [0.0, 0.0, 1.0],
            "output_filename": "out.brep",
            "feature_id": "draft-1",
            "rogue_key": true
        }"#;
        assert!(parse_draft_request(raw).is_err());
    }

    #[test]
    fn draft_envelope_accepts_canonical_shape() {
        let request = DraftRequest::new(
            "req-1",
            "/tmp/base.brep",
            std::f64::consts::FRAC_PI_2 / 6.0,
            [0.0, 0.0, 1.0],
        )
        .with_feature_id("draft-1");
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "draft");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["angle"], std::f64::consts::FRAC_PI_2 / 6.0);
        assert_eq!(value["pull_direction"], serde_json::json!([0.0, 0.0, 1.0]));
        assert_eq!(value["feature_id"], "draft-1");
    }

    #[test]
    fn loft_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "loft",
            "profiles": [[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]]],
            "rogue_key": true
        }"#;
        assert!(parse_loft_request(raw).is_err());
    }

    #[test]
    fn loft_envelope_accepts_canonical_shape() {
        let request = LoftRequest::new(
            "req-1",
            vec![
                vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
                vec![
                    [2.5, 2.5, 5.0],
                    [7.5, 2.5, 5.0],
                    [7.5, 7.5, 5.0],
                    [2.5, 7.5, 5.0],
                ],
            ],
        )
        .with_feature_id("loft-1");
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["operation"], "loft");
        assert_eq!(value["is_solid"], true);
        assert_eq!(value["ruled"], false);
        assert_eq!(
            value["profiles"],
            serde_json::json!([
                [
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0]
                ],
                [
                    [2.5, 2.5, 5.0],
                    [7.5, 2.5, 5.0],
                    [7.5, 7.5, 5.0],
                    [2.5, 7.5, 5.0]
                ]
            ])
        );
        assert_eq!(value["feature_id"], "loft-1");
    }

    #[test]
    fn map_outcome_completed_carries_the_typed_result_value() {
        let outcome = SupervisorOutcome::Completed {
            request_id: "req-1".to_string(),
            result: serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "request_id": "req-1",
                "operation": "extrude",
                "feature_id": "box-1",
                "status": "ok",
            }),
            artifact_headers: vec![],
        };
        let result = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect("completed outcome maps");
        assert_eq!(result.value["status"], "ok");
    }

    #[test]
    fn map_outcome_rejects_a_completed_result_bound_to_another_request() {
        let outcome = SupervisorOutcome::Completed {
            request_id: "req-1".to_string(),
            result: serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "request_id": "other-request",
                "operation": "extrude",
                "status": "ok",
            }),
            artifact_headers: vec![],
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("foreign request_id must fail closed");
        assert!(
            matches!(error, WorkerError::MalformedWithContext { .. }),
            "expected ID-bearing Malformed; got {error:?}"
        );
    }

    #[test]
    fn map_outcome_rejects_a_completed_result_bound_to_a_foreign_output_path() {
        let outcome = SupervisorOutcome::Completed {
            request_id: "req-1".to_string(),
            result: serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "request_id": "req-1",
                "operation": "extrude",
                "feature_id": "box-1",
                "status": "ok",
                "brep_path": "/tmp/other.brep",
            }),
            artifact_headers: vec![],
        };
        let error = map_outcome(
            outcome,
            "req-1",
            "extrude",
            "box-1",
            Some(PathBuf::from("/tmp/out.brep")),
        )
        .expect("completed outcome maps")
        .into_extrude()
        .expect_err("foreign output path must fail closed");
        match error {
            WorkerError::MalformedWithContext { request_id, detail } => {
                assert_eq!(request_id, "req-1");
                assert!(
                    detail.contains("output location"),
                    "detail must name the output binding; got {detail:?}"
                );
            }
            other => panic!("expected ID-bearing Malformed; got {other:?}"),
        }
    }

    #[test]
    fn map_outcome_accepts_a_completed_result_at_the_expected_output_path() {
        let output_path = std::env::temp_dir().join(format!("{}.brep", new_request_id()));
        std::fs::write(&output_path, []).expect("expected output fixture writes");
        let outcome = SupervisorOutcome::Completed {
            request_id: "req-1".to_string(),
            result: serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "request_id": "req-1",
                "operation": "extrude",
                "feature_id": "box-1",
                "status": "ok",
                "brep_path": output_path,
                "brep_sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "brep_bytes": 0,
            }),
            artifact_headers: vec![],
        };
        let result = map_outcome(
            outcome,
            "req-1",
            "extrude",
            "box-1",
            Some(output_path.clone()),
        )
        .expect("expected output path maps")
        .into_extrude()
        .expect("expected output path is accepted");
        std::fs::remove_file(output_path).expect("expected output fixture removes");
        assert_eq!(result.status, "ok");
    }

    #[test]
    fn map_outcome_rejects_a_completed_result_with_foreign_operation() {
        let outcome = SupervisorOutcome::Completed {
            request_id: "req-1".to_string(),
            result: serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "request_id": "req-1",
                "operation": "boolean_fuse",
                "status": "ok",
            }),
            artifact_headers: vec![],
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("foreign operation must fail closed");
        assert!(
            matches!(error, WorkerError::MalformedWithContext { .. }),
            "expected ID-bearing Malformed; got {error:?}"
        );
    }

    #[test]
    fn map_outcome_failed_envelope_becomes_a_structured_diagnostic() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "failed:brep_invalid:BRepCheck_Analyzer failed".to_string(),
                cancel_reason: None,
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: None,
                exit_code: None,
                stderr_tail: String::new(),
                failed_code: Some("brep_invalid".to_string()),
                failed_detail: Some("BRepCheck_Analyzer failed".to_string()),
                exit_kind: ExitKind::Cooperative,
            },
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("failed envelope must not map to success");
        match error {
            WorkerError::DiagnosticWithContext {
                request_id,
                diagnostic,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(diagnostic.code, "brep_invalid");
                assert_eq!(diagnostic.arg, "BRepCheck_Analyzer failed");
                assert_eq!(diagnostic.schema_version, SCHEMA_VERSION);
            }
            other => panic!("expected Diagnostic; got {other:?}"),
        }
    }

    #[test]
    fn map_outcome_failed_envelope_with_signal_preserves_termination_context() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "failed:brep_invalid:BRepCheck_Analyzer failed".to_string(),
                cancel_reason: None,
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: Some(9),
                exit_code: None,
                stderr_tail: "worker crashed".to_string(),
                failed_code: Some("brep_invalid".to_string()),
                failed_detail: Some("BRepCheck_Analyzer failed".to_string()),
                // A Failed envelope can be observed before the worker dies;
                // the signal must still keep the structured termination record.
                exit_kind: ExitKind::Cooperative,
            },
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("signal-bearing failure must not lose termination context");
        match error {
            WorkerError::Supervised { record } => {
                assert_eq!(record.exit_signal, Some(9));
                assert_eq!(record.failed_code.as_deref(), Some("brep_invalid"));
                assert_eq!(record.stderr_tail, "worker crashed");
            }
            other => panic!("expected Supervised; got {other:?}"),
        }
    }

    #[test]
    fn map_outcome_signal_exit_reports_the_actual_signal() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "grace_exceeded".to_string(),
                cancel_reason: None,
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: Some(11),
                exit_code: None,
                stderr_tail: String::new(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("signal exit must not map to success");
        match error {
            WorkerError::Supervised { record } => {
                assert_eq!(record.exit_signal, Some(11));
                assert_eq!(record.stage, "grace_exceeded");
            }
            other => panic!("expected Supervised; got {other:?}"),
        }
    }

    #[test]
    fn map_outcome_natural_signal_exit_becomes_typed_signalled_error() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "worker_closed".to_string(),
                cancel_reason: None,
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: Some(11),
                exit_code: None,
                stderr_tail: "segmentation fault".to_string(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("natural signal exit must fail closed");
        match error {
            WorkerError::SignalledWithContext {
                request_id,
                signal,
                stderr,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(signal, 11);
                assert_eq!(stderr, "segmentation fault");
            }
            other => panic!("expected Signalled; got {other:?}"),
        }
    }

    #[test]
    fn map_outcome_natural_nonzero_exit_becomes_typed_nonzero_error() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "worker_closed".to_string(),
                cancel_reason: None,
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: None,
                exit_code: Some(2),
                stderr_tail: "malformed request".to_string(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("natural nonzero exit must fail closed");
        match error {
            WorkerError::NonZeroExitWithContext {
                request_id,
                code,
                stderr,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(code, Some(2));
                assert_eq!(stderr, "malformed request");
            }
            other => panic!("expected NonZeroExit; got {other:?}"),
        }
    }

    #[test]
    fn map_outcome_handshake_schema_mismatch_fails_closed() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "<handshake>".to_string(),
                stage: "handshake_schema_mismatch:received=threeterm.protocol/0 expected=threeterm.protocol/1"
                    .to_string(),
                cancel_reason: None,
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: Some(9),
                exit_code: None,
                stderr_tail: String::new(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("schema mismatch must fail closed");
        assert!(
            matches!(error, WorkerError::MalformedWithContext { .. }),
            "expected ID-bearing Malformed; got {error:?}"
        );
    }

    #[test]
    fn map_outcome_closed_worker_preserves_stderr_tail() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "worker_closed".to_string(),
                cancel_reason: None,
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: None,
                exit_code: None,
                stderr_tail: "worker trace".to_string(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome, "req-1", "extrude", "box-1", None)
            .expect_err("closed worker must fail closed");
        match error {
            WorkerError::Supervised { record } => {
                assert_eq!(record.stderr_tail, "worker trace");
                assert_eq!(record.request_id, "req-1");
            }
            other => panic!("expected Supervised; got {other:?}"),
        }
    }

    #[test]
    fn run_with_cancel_rejects_oversized_envelope_before_unbounded_parse() {
        use std::sync::atomic::AtomicBool;
        let worker = OcctWorker::with_binary_path(std::path::PathBuf::from("/no/such/worker"));
        let cancel = AtomicBool::new(false);
        // Build a valid JSON envelope that exceeds the frame bound but carries the
        // request_id near the front so the bounded hint can recover it.
        let prefix = format!(
            r#"{{"request_id":"req-oversized","operation":"extrude","schema_version":"{}","feature_id":"box-1","output_dir":"/tmp","output_filename":"out.brep","profile":[[0.0,0.0]],"height":1.0,"padding":""#,
            SCHEMA_VERSION
        );
        let suffix = "\"}";
        let pad_len =
            threeterm_protocol::frame::MAX_FRAME_BUFFER - prefix.len() - suffix.len() + 1024;
        let mut envelope = prefix;
        envelope.push_str(&"x".repeat(pad_len));
        envelope.push_str(suffix);
        assert!(
            envelope.len() > threeterm_protocol::frame::MAX_FRAME_BUFFER,
            "envelope must exceed the bound"
        );
        let error = worker
            .run_with_cancel(envelope.as_bytes(), &cancel)
            .expect_err("oversized envelope must fail closed");
        match error {
            WorkerError::MalformedWithContext { request_id, detail } => {
                assert_eq!(request_id, "req-oversized");
                assert!(
                    detail.contains("exceeds the"),
                    "detail must name the bound; got {detail:?}"
                );
            }
            other => panic!("expected ID-bearing Malformed; got {other:?}"),
        }
    }

    #[test]
    fn bounded_request_id_hint_is_bounded() {
        let mut envelope = br#"{"request_id":"req-hint","operation":"extrude"#.to_vec();
        envelope.resize(threeterm_protocol::frame::MAX_FRAME_BUFFER + 1024, b'x');
        let hint = bounded_request_id_hint(&envelope);
        assert_eq!(hint, "req-hint");
        // Non-UTF8 prefix must not panic and yields empty hint.
        let mut non_utf8 = vec![0xff, 0xfe];
        non_utf8.extend_from_slice(b"request_id");
        non_utf8.resize(threeterm_protocol::frame::MAX_FRAME_BUFFER + 1, b'x');
        assert_eq!(bounded_request_id_hint(&non_utf8), "");
    }
}

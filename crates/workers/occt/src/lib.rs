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

use threeterm_protocol::supervisor::{Request as SupervisorRequest, Supervisor, SupervisorOutcome};
use threeterm_protocol::worker::{
    SubprocessWorkerHost, WorkerConfig, WorkerError as ProtocolWorkerError, WorkerHost,
    WorkerProcess,
};

pub mod envelope;
pub use envelope::{
    BooleanFuseRequest, BooleanFuseResult, ChamferRequest, ChamferResult, CircularPatternRequest,
    CircularPatternResult, DraftRequest, DraftResult, ExtrudeRequest, ExtrudeResult, FilletRequest,
    FilletResult, HoleRequest, HoleResult, LinearPatternRequest, LinearPatternResult, LoftRequest,
    LoftResult, MirrorRequest, MirrorResult, Operation, RevolveRequest, RevolveResult,
    SCHEMA_VERSION, ShellRequest, ShellResult,
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
    Spawn { binary: PathBuf, detail: String },
    /// The worker exited with a non-zero status.
    NonZeroExit { code: Option<i32>, stderr: String },
    /// The worker exited due to a signal.
    Signalled { signal: i32 },
    /// The worker emitted output that is not valid JSON or not a parseable
    /// envelope.
    Malformed { detail: String },
    /// The worker emitted a JSON diagnostic instead of a response.
    Diagnostic(OcctDiagnostic),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { binary, detail } => {
                write!(
                    formatter,
                    "worker spawn failed at {}: {detail}",
                    binary.display()
                )
            }
            Self::NonZeroExit { code, stderr } => {
                write!(formatter, "worker exited with code {code:?}: {stderr}")
            }
            Self::Signalled { signal } => {
                write!(formatter, "worker signalled with {signal}")
            }
            Self::Malformed { detail } => {
                write!(formatter, "malformed worker output: {detail}")
            }
            Self::Diagnostic(diagnostic) => write!(
                formatter,
                "worker diagnostic {} {}: {}",
                diagnostic.code, diagnostic.arg, diagnostic.schema_version
            ),
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
}

/// Default supervisor grace for OCCT operations. Operations complete in
/// well under a second; this bound catches hangs without harming
/// legitimate geometry work.
pub const DEFAULT_SUPERVISOR_GRACE: Duration = Duration::from_secs(30);

impl OcctWorker {
    /// Locate the worker binary. Prefers the path embedded at build
    /// time (the `OUT_DIR/bin/threeterm-occt-worker` produced by
    /// `build.rs`), then the `THREETERM_OCCTBUILD_WORKER` environment
    /// variable, and finally the `target/<profile>/bin/` heuristics.
    pub fn locate() -> Result<Self, WorkerError> {
        let built = PathBuf::from(BUILT_WORKER_PATH.trim());
        if built.is_file() {
            return Ok(Self::with_binary_path(built));
        }
        if let Some(path) = env::var_os("THREETERM_OCCTBUILD_WORKER") {
            let candidate = PathBuf::from(path);
            if candidate.is_file() {
                return Ok(Self::with_binary_path(candidate));
            }
        }
        let target_root = env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .or_else(|| env::current_dir().ok().map(|cwd| cwd.join("target")))
            .ok_or_else(|| WorkerError::Spawn {
                binary: PathBuf::from("threeterm-occt-worker"),
                detail: "could not determine target directory".to_string(),
            })?;
        for profile in ["debug", "release"] {
            let candidate = target_root.join(profile).join("bin/threeterm-occt-worker");
            if candidate.is_file() {
                return Ok(Self::with_binary_path(candidate));
            }
        }
        Err(WorkerError::Spawn {
            binary: target_root.join("debug/bin/threeterm-occt-worker"),
            detail: "worker binary not found; build the occt worker first".to_string(),
        })
    }

    pub fn with_binary_path(binary_path: PathBuf) -> Self {
        Self {
            binary_path,
            grace: DEFAULT_SUPERVISOR_GRACE,
        }
    }

    /// Override the supervisor grace period (deadline) for every
    /// operation this worker executes.
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    /// Extrude `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn extrude(&self, request: &ExtrudeRequest) -> Result<ExtrudeResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("extrude request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_extrude()
    }

    /// Boolean-fuse `request` by spawning the worker process. See
    /// module docs for the disposable-worker contract.
    pub fn boolean_fuse(
        &self,
        request: &BooleanFuseRequest,
    ) -> Result<BooleanFuseResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("boolean-fuse request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_boolean_fuse()
    }

    /// Fillet `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn fillet(&self, request: &FilletRequest) -> Result<FilletResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("fillet request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_fillet()
    }

    /// Chamfer `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn chamfer(&self, request: &ChamferRequest) -> Result<ChamferResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("chamfer request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_chamfer()
    }

    /// Hole `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn hole(&self, request: &HoleRequest) -> Result<HoleResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("hole request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_hole()
    }

    /// Revolve `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn revolve(&self, request: &RevolveRequest) -> Result<RevolveResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("revolve request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_revolve()
    }

    /// Mirror `request` by spawning the worker process. See module
    /// docs for the disposable-worker contract.
    pub fn mirror(&self, request: &MirrorRequest) -> Result<MirrorResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("mirror request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_mirror()
    }

    /// Linear pattern `request` by spawning the worker process. See
    /// module docs for the disposable-worker contract.
    pub fn linear_pattern(
        &self,
        request: &LinearPatternRequest,
    ) -> Result<LinearPatternResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("linear_pattern request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_linear_pattern()
    }

    /// Circular pattern `request` by spawning the worker process. See
    /// module docs for the disposable-worker contract.
    pub fn circular_pattern(
        &self,
        request: &CircularPatternRequest,
    ) -> Result<CircularPatternResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("circular_pattern request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_circular_pattern()
    }

    /// Shell `request` by spawning the worker process. See module docs
    /// for the disposable-worker contract.
    pub fn shell(&self, request: &ShellRequest) -> Result<ShellResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("shell request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_shell()
    }

    /// Draft `request` by spawning the worker process. See module docs
    /// for the disposable-worker contract.
    pub fn draft(&self, request: &DraftRequest) -> Result<DraftResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("draft request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_draft()
    }

    /// Loft `request` by spawning the worker process. See module docs
    /// for the disposable-worker contract.
    pub fn loft(&self, request: &LoftRequest) -> Result<LoftResult, WorkerError> {
        let bytes = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("loft request serialization failed: {error}"),
        })?;
        self.invoke(&bytes)?.into_loft()
    }

    fn invoke(&self, envelope: &[u8]) -> Result<RawResult, WorkerError> {
        // The OCCT envelope carries its own request_id (the protocol
        // binds every message to it), so extract it for the supervisor.
        let args: serde_json::Value =
            serde_json::from_slice(envelope).map_err(|error| WorkerError::Malformed {
                detail: format!("request serialization failed: {error}"),
            })?;
        let request_id = args
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let command_id = args
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();

        let host = <Self as WorkerProcess>::spawn(WorkerConfig {
            worker_id: "occt",
            schema_version: threeterm_protocol::schema_version(),
            command_line: vec![self.binary_path.display().to_string()],
        })
        .map_err(|error| WorkerError::Spawn {
            binary: self.binary_path.clone(),
            detail: error.to_string(),
        })?;
        let mut supervisor = Supervisor::new(self.grace, host, None);
        let outcome = supervisor.request(SupervisorRequest {
            request_id,
            command_id,
            args,
            revision_id: String::new(),
        });
        map_outcome(outcome)
    }
}

/// Maps a supervised outcome to the typed-result boundary: a completed
/// request carries the typed result JSON, a cooperative `Failed`
/// envelope becomes an [`OcctDiagnostic`], a signal-based exit keeps the
/// actual signal, and everything else fails closed.
fn map_outcome(outcome: SupervisorOutcome) -> Result<RawResult, WorkerError> {
    match outcome {
        SupervisorOutcome::Completed { result, .. } => Ok(RawResult { value: result }),
        SupervisorOutcome::Acknowledged { .. } => Err(WorkerError::Malformed {
            detail: "worker acknowledged a cancellation without a request".to_string(),
        }),
        SupervisorOutcome::ForceTerminated { record } => {
            if let (Some(code), Some(detail)) = (record.failed_code, record.failed_detail) {
                return Err(WorkerError::Diagnostic(OcctDiagnostic::new(code, detail)));
            }
            if record.stage.starts_with("handshake_schema_mismatch") {
                return Err(WorkerError::Malformed {
                    detail: record.stage,
                });
            }
            if let Some(signal) = record.exit_signal {
                return Err(WorkerError::Signalled { signal });
            }
            Err(WorkerError::NonZeroExit {
                code: None,
                stderr: record.stderr_tail,
            })
        }
    }
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
}

impl RawResult {
    fn into_extrude(self) -> Result<ExtrudeResult, WorkerError> {
        match serde_json::from_value::<ExtrudeResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_extrude response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_boolean_fuse(self) -> Result<BooleanFuseResult, WorkerError> {
        match serde_json::from_value::<BooleanFuseResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_boolean_fuse response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_fillet(self) -> Result<FilletResult, WorkerError> {
        match serde_json::from_value::<FilletResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_fillet response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_chamfer(self) -> Result<ChamferResult, WorkerError> {
        match serde_json::from_value::<ChamferResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_chamfer response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_hole(self) -> Result<HoleResult, WorkerError> {
        match serde_json::from_value::<HoleResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_hole response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_revolve(self) -> Result<RevolveResult, WorkerError> {
        match serde_json::from_value::<RevolveResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_revolve response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_mirror(self) -> Result<MirrorResult, WorkerError> {
        match serde_json::from_value::<MirrorResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_mirror response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_linear_pattern(self) -> Result<LinearPatternResult, WorkerError> {
        match serde_json::from_value::<LinearPatternResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_linear_pattern response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_circular_pattern(self) -> Result<CircularPatternResult, WorkerError> {
        match serde_json::from_value::<CircularPatternResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_circular_pattern response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_shell(self) -> Result<ShellResult, WorkerError> {
        match serde_json::from_value::<ShellResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_shell response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_draft(self) -> Result<DraftResult, WorkerError> {
        match serde_json::from_value::<DraftResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_draft response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
    }

    fn into_loft(self) -> Result<LoftResult, WorkerError> {
        match serde_json::from_value::<LoftResult>(self.value.clone()) {
            Ok(result) => Ok(result),
            Err(error) => Err(WorkerError::Malformed {
                detail: format!(
                    "into_loft response could not be parsed: {error}; value={}",
                    self.value
                ),
            }),
        }
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
            result: serde_json::json!({ "status": "ok", "operation": "extrude" }),
            artifact_headers: vec![],
        };
        let result = map_outcome(outcome).expect("completed outcome maps");
        assert_eq!(result.value["status"], "ok");
    }

    #[test]
    fn map_outcome_failed_envelope_becomes_a_structured_diagnostic() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "failed:brep_invalid:BRepCheck_Analyzer failed".to_string(),
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: None,
                stderr_tail: String::new(),
                failed_code: Some("brep_invalid".to_string()),
                failed_detail: Some("BRepCheck_Analyzer failed".to_string()),
                exit_kind: ExitKind::Cooperative,
            },
        };
        let error = map_outcome(outcome).expect_err("failed envelope must not map to success");
        match error {
            WorkerError::Diagnostic(diagnostic) => {
                assert_eq!(diagnostic.code, "brep_invalid");
                assert_eq!(diagnostic.arg, "BRepCheck_Analyzer failed");
                assert_eq!(diagnostic.schema_version, SCHEMA_VERSION);
            }
            other => panic!("expected Diagnostic; got {other:?}"),
        }
    }

    #[test]
    fn map_outcome_signal_exit_reports_the_actual_signal() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "grace_exceeded".to_string(),
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: Some(11),
                stderr_tail: String::new(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome).expect_err("signal exit must not map to success");
        match error {
            WorkerError::Signalled { signal } => assert_eq!(signal, 11),
            other => panic!("expected Signalled; got {other:?}"),
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
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: Some(9),
                stderr_tail: String::new(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome).expect_err("schema mismatch must fail closed");
        assert!(
            matches!(error, WorkerError::Malformed { .. }),
            "expected Malformed; got {error:?}"
        );
    }

    #[test]
    fn map_outcome_closed_worker_preserves_stderr_tail() {
        use threeterm_protocol::supervisor::{ExitKind, TerminationRecord};
        let outcome = SupervisorOutcome::ForceTerminated {
            record: TerminationRecord {
                request_id: "req-1".to_string(),
                stage: "worker_closed".to_string(),
                elapsed: Duration::from_millis(1),
                last_progress: None,
                last_artifact_error: None,
                exit_signal: None,
                stderr_tail: "worker trace".to_string(),
                failed_code: None,
                failed_detail: None,
                exit_kind: ExitKind::ForceAfterGrace,
            },
        };
        let error = map_outcome(outcome).expect_err("closed worker must fail closed");
        match error {
            WorkerError::NonZeroExit { stderr, .. } => {
                assert_eq!(stderr, "worker trace");
            }
            other => panic!("expected NonZeroExit; got {other:?}"),
        }
    }
}

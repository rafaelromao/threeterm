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
//!   [`ShellRequest`], [`ShellResult`] — the JSON envelopes
//!   exchanged with the worker, with `serde(deny_unknown_fields)` to
//!   fail closed on unexpected fields.
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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub mod envelope;
pub use envelope::{
    BooleanFuseRequest, BooleanFuseResult, ChamferRequest, ChamferResult, CircularPatternRequest,
    CircularPatternResult, ExtrudeRequest, ExtrudeResult, FilletRequest, FilletResult, HoleRequest,
    HoleResult, LinearPatternRequest, LinearPatternResult, MirrorRequest, MirrorResult, Operation,
    RevolveRequest, RevolveResult, SCHEMA_VERSION, ShellRequest, ShellResult,
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
    Signalled { signal: i32, stderr: String },
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
            Self::Signalled { signal, stderr } => {
                write!(formatter, "worker signalled with {signal}: {stderr}")
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
/// `revolve`, `mirror`, `linear_pattern`, `circular_pattern`, and
/// `shell`.
///
/// The worker is **disposable**: each call spawns a fresh process, pipes
/// the request to its stdin, reads one JSON line from its stdout, and
/// kills the process on exit. The worker has no persistent state.
#[derive(Debug, Clone)]
pub struct OcctWorker {
    binary_path: PathBuf,
}

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
        Self { binary_path }
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

    fn invoke(&self, envelope: &[u8]) -> Result<RawResult, WorkerError> {
        let mut child = Command::new(&self.binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| WorkerError::Spawn {
                binary: self.binary_path.clone(),
                detail: error.to_string(),
            })?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(envelope)
                .map_err(|error| WorkerError::Spawn {
                    binary: self.binary_path.clone(),
                    detail: format!("stdin write failed: {error}"),
                })?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| WorkerError::Spawn {
                binary: self.binary_path.clone(),
                detail: format!("wait failed: {error}"),
            })?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        match output.status.code() {
            Some(0) => {}
            Some(2) => {
                return Err(WorkerError::Diagnostic(OcctDiagnostic::new(
                    "request_malformed",
                    stderr.trim().to_string(),
                )));
            }
            Some(3) => {
                return Err(WorkerError::Diagnostic(OcctDiagnostic::new(
                    "brep_invalid",
                    stderr.trim().to_string(),
                )));
            }
            Some(code) => {
                return Err(WorkerError::NonZeroExit {
                    code: Some(code),
                    stderr,
                });
            }
            None => {
                return Err(WorkerError::Signalled { signal: 0, stderr });
            }
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|line| !line.trim().is_empty())
            .ok_or_else(|| WorkerError::Malformed {
                detail: "worker emitted empty stdout".to_string(),
            })?;
        Ok(RawResult {
            line: line.to_string(),
        })
    }
}

struct RawResult {
    line: String,
}

impl RawResult {
    fn into_extrude(self) -> Result<ExtrudeResult, WorkerError> {
        match serde_json::from_str::<ExtrudeResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "extrude response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_boolean_fuse(self) -> Result<BooleanFuseResult, WorkerError> {
        match serde_json::from_str::<BooleanFuseResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "boolean-fuse response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_fillet(self) -> Result<FilletResult, WorkerError> {
        match serde_json::from_str::<FilletResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "fillet response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_chamfer(self) -> Result<ChamferResult, WorkerError> {
        match serde_json::from_str::<ChamferResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "chamfer response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_hole(self) -> Result<HoleResult, WorkerError> {
        match serde_json::from_str::<HoleResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "hole response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_revolve(self) -> Result<RevolveResult, WorkerError> {
        match serde_json::from_str::<RevolveResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "revolve response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_mirror(self) -> Result<MirrorResult, WorkerError> {
        match serde_json::from_str::<MirrorResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "mirror response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_linear_pattern(self) -> Result<LinearPatternResult, WorkerError> {
        match serde_json::from_str::<LinearPatternResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "linear_pattern response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_circular_pattern(self) -> Result<CircularPatternResult, WorkerError> {
        match serde_json::from_str::<CircularPatternResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "circular_pattern response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
        }
    }

    fn into_shell(self) -> Result<ShellResult, WorkerError> {
        match serde_json::from_str::<ShellResult>(&self.line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<OcctDiagnostic>(&self.line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!(
                        "shell response could not be parsed: {error}; line={}",
                        self.line
                    ),
                }),
            },
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
}

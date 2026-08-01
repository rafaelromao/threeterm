//! Sketch constraint solver worker boundary.
//!
//! The C++ worker binary is built by `build.rs` from vendored SolveSpace
//! `libslvs` sources. The Rust side of this crate exposes:
//!
//! * [`SchemaVersion`] — the pinned worker protocol schema.
//! * [`SketchRequest`], [`SolveResult`] — the JSON envelope exchanged with
//!   the worker, with `serde(deny_unknown_fields)` to fail closed on
//!   unexpected fields.
//! * [`SlvsWorker`] — the boundary struct that spawns the worker binary,
//!   pipes the request in, reads the response, and returns either a typed
//!   [`SolveResult`] or a [`SolveDiagnostic`].
//!
//! The worker binary lives at `OUT_DIR/bin/threeterm-slvs-worker` for the
//! running build. Tests can override the location through
//! `SlvsWorker::with_binary_path` or by setting the
//! `THREETERM_SLVSBUILD_WORKER` environment variable when cargo provides the
//! path through the build script.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "threeterm.workers.slvs/1";

pub fn schema_version() -> &'static str {
    SCHEMA_VERSION
}

pub mod envelope;
pub use envelope::{SketchEntity, SketchParam, SketchRequest, SketchConstraint, SolveResult};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coordinate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
}

/// Worker-boundary diagnostic. The shape mirrors `protocol::diagnostic::Diagnostic`
/// so the host can convert without losing information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolveDiagnostic {
    pub code: String,
    pub arg: String,
    pub schema_version: String,
}

impl SolveDiagnostic {
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
    Diagnostic(SolveDiagnostic),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { binary, detail } => {
                write!(formatter, "worker spawn failed at {}: {detail}", binary.display())
            }
            Self::NonZeroExit { code, stderr } => {
                write!(formatter, "worker exited with code {code:?}: {stderr}")
            }
            Self::Signalled { signal, stderr } => {
                write!(formatter, "worker signalled with {signal}: {stderr}")
            }
            Self::Malformed { detail } => write!(formatter, "malformed worker output: {detail}"),
            Self::Diagnostic(diagnostic) => write!(
                formatter,
                "worker diagnostic {} {}: {}",
                diagnostic.code, diagnostic.arg, diagnostic.schema_version
            ),
        }
    }
}

impl std::error::Error for WorkerError {}

/// Process-backed sketch solver. Owns the binary path and exposes `solve`.
///
/// The worker is **disposable**: each call to `solve` spawns a fresh process,
/// pipes the request to its stdin, reads one JSON line from its stdout, and
/// kills the process on exit. The worker has no persistent state.
#[derive(Debug, Clone)]
pub struct SlvsWorker {
    binary_path: PathBuf,
}

impl SlvsWorker {
    /// Locate the worker binary. Prefers the `THREETERM_SLVSBUILD_WORKER`
    /// environment variable (set by `build.rs`), then falls back to the
    /// `target/<profile>/` and `OUT_DIR/bin/` heuristics, and finally to
    /// `which`-style lookup under `target/`.
    pub fn locate() -> Result<Self, WorkerError> {
        if let Some(path) = env::var_os("THREETERM_SLVSBUILD_WORKER") {
            let candidate = PathBuf::from(path);
            if candidate.is_file() {
                return Ok(Self::with_binary_path(candidate));
            }
        }
        if let Ok(out_dir) = env::var("OUT_DIR") {
            let candidate = PathBuf::from(out_dir).join("bin/threeterm-slvs-worker");
            if candidate.is_file() {
                return Ok(Self::with_binary_path(candidate));
            }
        }
        let target_root = env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .or_else(|| env::current_dir().ok().map(|cwd| cwd.join("target")))
            .ok_or_else(|| WorkerError::Spawn {
                binary: PathBuf::from("threeterm-slvs-worker"),
                detail: "could not determine target directory".to_string(),
            })?;
        for profile in ["debug", "release"] {
            let candidate = target_root
                .join(profile)
                .join("bin/threeterm-slvs-worker");
            if candidate.is_file() {
                return Ok(Self::with_binary_path(candidate));
            }
        }
        Err(WorkerError::Spawn {
            binary: target_root.join("debug/bin/threeterm-slvs-worker"),
            detail: "worker binary not found; build the slvs worker first".to_string(),
        })
    }

    pub fn with_binary_path(binary_path: PathBuf) -> Self {
        Self { binary_path }
    }

    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    /// Solve `request` by spawning the worker process. See module docs for
    /// the disposable-worker contract.
    pub fn solve(&self, request: &SketchRequest) -> Result<SolveResult, WorkerError> {
        let envelope = serde_json::to_vec(request).map_err(|error| WorkerError::Malformed {
            detail: format!("request serialization failed: {error}"),
        })?;
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
            stdin.write_all(&envelope).map_err(|error| WorkerError::Spawn {
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
                return Err(WorkerError::Diagnostic(SolveDiagnostic::new(
                    "request_malformed",
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
                return Err(WorkerError::Signalled {
                    signal: 0,
                    stderr,
                });
            }
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|line| !line.trim().is_empty())
            .ok_or_else(|| WorkerError::Malformed {
                detail: "worker emitted empty stdout".to_string(),
            })?;
        match serde_json::from_str::<SolveResult>(line) {
            Ok(result) => Ok(result),
            Err(_) => match serde_json::from_str::<SolveDiagnostic>(line) {
                Ok(diagnostic) => Err(WorkerError::Diagnostic(diagnostic)),
                Err(error) => Err(WorkerError::Malformed {
                    detail: format!("response could not be parsed as result or diagnostic: {error}; line={line}"),
                }),
            },
        }
    }
}

/// Helper for tests and consumers that need a deterministic request id.
pub fn new_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("req-{nanos}-{}", std::process::id())
}

/// Helper for building a sketch request directly from `Value` JSON for tests
/// that do not want to use the typed builder.
pub fn parse_request(raw: &str) -> Result<SketchRequest, serde_json::Error> {
    serde_json::from_str(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, "threeterm.workers.slvs/1");
    }

    #[test]
    fn diagnostic_serializes_with_schema_version() {
        let diagnostic = SolveDiagnostic::new("request_malformed", "empty stdin");
        let value = serde_json::to_value(&diagnostic).expect("diagnostic serializes");
        assert_eq!(value["code"], "request_malformed");
        assert_eq!(value["arg"], "empty stdin");
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
        let request = SketchRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            entities: vec![],
            constraints: vec![],
        };
        let value = serde_json::to_value(&request).expect("serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["entities"], serde_json::json!([]));
        assert_eq!(value["constraints"], serde_json::json!([]));
    }

    #[test]
    fn envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.slvs/1",
            "request_id": "req-1",
            "entities": [],
            "constraints": [],
            "rogue_key": true
        }"#;
        assert!(parse_request(raw).is_err());
    }

    #[test]
    fn solve_result_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.slvs/1",
            "request_id": "req-1",
            "status": "ok",
            "dof": 0,
            "resolved_entity_ids": [],
            "failed_constraint_ids": [],
            "coordinates": {},
            "rogue_key": true
        }"#;
        let result = serde_json::from_str::<SolveResult>(raw);
        assert!(result.is_err(), "unknown key must be rejected");
    }

    #[test]
    fn solve_result_accepts_canonical_shape() {
        let raw = r#"{
            "schema_version": "threeterm.workers.slvs/1",
            "request_id": "req-1",
            "status": "ok",
            "dof": 0,
            "resolved_entity_ids": ["p1", "p2"],
            "failed_constraint_ids": [],
            "coordinates": {
                "p1": [1.0, 2.0],
                "p2": [3.0, 4.0]
            }
        }"#;
        let result: SolveResult = serde_json::from_str(raw).expect("canonical shape parses");
        assert_eq!(result.status, "ok");
        assert_eq!(result.dof, 0);
        assert_eq!(result.resolved_entity_ids, vec!["p1".to_string(), "p2".to_string()]);
        assert!(result.failed_constraint_ids.is_empty());
        let coords = result.coordinates.expect("coordinates populated");
        assert_eq!(coords.get("p1"), Some(&[1.0, 2.0]));
        assert_eq!(coords.get("p2"), Some(&[3.0, 4.0]));
    }
}
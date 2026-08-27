#![allow(clippy::result_large_err)]

use std::env;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use threeterm_protocol::supervisor::{
    CancellationGracePolicy, Request as SupervisorRequest, Supervisor, SupervisorOutcome,
};
use threeterm_protocol::worker::{
    SubprocessWorkerHost, WorkerConfig, WorkerError as ProtocolWorkerError, WorkerHost,
    WorkerProcess,
};

pub mod envelope;
pub use envelope::{
    SCHEMA_VERSION, SketchConstraint, SketchDiagnostic, SketchEntity, SketchSolveRequest,
    SketchSolveResponse, SolvedCoordinate,
};

pub const BUILT_WORKER_PATH: &str = include_str!(concat!(env!("OUT_DIR"), "/worker_path.txt"));
pub const DEFAULT_SUPERVISOR_GRACE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlvsDiagnostic {
    pub code: String,
    pub detail: String,
}

#[derive(Debug)]
pub enum WorkerError {
    Spawn {
        binary: PathBuf,
        detail: String,
    },
    IdentityMismatch {
        binary: PathBuf,
        expected: String,
        actual: String,
    },
    Malformed {
        detail: String,
    },
    Diagnostic(SlvsDiagnostic),
    Supervised {
        stage: String,
        request_id: String,
    },
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { binary, detail } => {
                write!(f, "could not spawn {}: {detail}", binary.display())
            }
            Self::IdentityMismatch {
                binary,
                expected,
                actual,
            } => write!(
                f,
                "worker identity mismatch at {}: expected_sha256={expected} actual_sha256={actual}",
                binary.display()
            ),
            Self::Malformed { detail } => write!(f, "malformed libslvs worker response: {detail}"),
            Self::Diagnostic(diagnostic) => write!(f, "{}: {}", diagnostic.code, diagnostic.detail),
            Self::Supervised { stage, request_id } => {
                write!(f, "worker request {request_id} ended at {stage}")
            }
        }
    }
}

impl std::error::Error for WorkerError {}

#[derive(Debug, Clone)]
pub struct SlvsWorker {
    binary_path: PathBuf,
    grace: Duration,
    revision_id: Option<String>,
    expected_binary_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BinaryFingerprint {
    pub worker_kind: String,
    pub source_repository: String,
    pub source_commit: String,
    pub binary_sha256: String,
}

pub const SOURCE_REPOSITORY: &str = "https://github.com/solvespace/solvespace";
pub const SOURCE_COMMIT: &str = "27b6a080c8b669421bd4d444650c3b8eddec5687";

impl SlvsWorker {
    pub fn locate() -> Result<Self, WorkerError> {
        let built = PathBuf::from(BUILT_WORKER_PATH.trim());
        if built.is_file() {
            return Ok(Self::with_binary_path(built).with_expected_binary_sha256_from_env());
        }
        if let Some(path) = env::var_os("THREETERM_SLVSBUILD_WORKER") {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(Self::with_binary_path(path).with_expected_binary_sha256_from_env());
            }
        }
        Err(WorkerError::Spawn {
            binary: built,
            detail:
                "libslvs worker binary not found; configure THREETERM_SLVS_DIR or build the worker"
                    .to_string(),
        })
    }

    pub fn with_binary_path(path: impl Into<PathBuf>) -> Self {
        Self {
            binary_path: path.into(),
            grace: DEFAULT_SUPERVISOR_GRACE,
            revision_id: None,
            expected_binary_sha256: None,
        }
    }

    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn with_revision_id(mut self, revision: impl Into<String>) -> Self {
        self.revision_id = Some(revision.into());
        self
    }

    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    pub fn with_expected_binary_sha256(mut self, digest: impl Into<String>) -> Self {
        self.expected_binary_sha256 = Some(digest.into());
        self
    }

    fn with_expected_binary_sha256_from_env(mut self) -> Self {
        self.expected_binary_sha256 = env::var("THREETERM_SLVS_WORKER_SHA256").ok();
        self
    }

    pub fn verify_identity(&self) -> Result<BinaryFingerprint, WorkerError> {
        let metadata =
            std::fs::symlink_metadata(&self.binary_path).map_err(|error| WorkerError::Spawn {
                binary: self.binary_path.clone(),
                detail: format!("worker binary cannot be inspected: {error}"),
            })?;
        if !metadata.file_type().is_file() {
            return Err(WorkerError::Spawn {
                binary: self.binary_path.clone(),
                detail: "worker binary is not a regular file".to_string(),
            });
        }
        let actual = sha256_file(&self.binary_path).map_err(|error| WorkerError::Spawn {
            binary: self.binary_path.clone(),
            detail: format!("worker binary hash failed: {error}"),
        })?;
        if let Some(expected) = &self.expected_binary_sha256
            && expected != &actual
        {
            return Err(WorkerError::IdentityMismatch {
                binary: self.binary_path.clone(),
                expected: expected.clone(),
                actual,
            });
        }
        Ok(BinaryFingerprint {
            worker_kind: "slvs".to_string(),
            source_repository: SOURCE_REPOSITORY.to_string(),
            source_commit: SOURCE_COMMIT.to_string(),
            binary_sha256: actual,
        })
    }

    pub fn solve(&self, request: &SketchSolveRequest) -> Result<SketchSolveResponse, WorkerError> {
        self.solve_with_cancel(request, &AtomicBool::new(false))
    }

    pub fn solve_with_cancel(
        &self,
        request: &SketchSolveRequest,
        cancel: &AtomicBool,
    ) -> Result<SketchSolveResponse, WorkerError> {
        request
            .validate()
            .map_err(|detail| WorkerError::Malformed { detail })?;
        let args = serde_json::to_value(request).map_err(|error| WorkerError::Malformed {
            detail: error.to_string(),
        })?;
        self.verify_identity()?;
        let mut supervisor = Supervisor::new(
            self.grace,
            <Self as WorkerProcess>::spawn(WorkerConfig {
                worker_id: "slvs",
                schema_version: threeterm_protocol::schema_version(),
                command_line: vec![self.binary_path.display().to_string()],
            })
            .map_err(|error| WorkerError::Spawn {
                binary: self.binary_path.clone(),
                detail: error.to_string(),
            })?,
            None,
        )
        .with_expected_worker_id("slvs")
        .with_cancellation_grace_policy(CancellationGracePolicy::new(Duration::from_millis(100)));
        let request_id = request.request_id.clone();
        let outcome = supervisor.request_with_cancel(
            SupervisorRequest {
                request_id: request_id.clone(),
                command_id: "sketch_solve".to_string(),
                args,
                revision_id: self
                    .revision_id
                    .clone()
                    .unwrap_or_else(|| request.source_revision.clone()),
            },
            cancel,
        );
        let response = map_outcome(outcome, &request_id)?;
        response
            .validate_for(request)
            .map_err(|detail| WorkerError::Malformed { detail })?;
        Ok(response)
    }
}

fn map_outcome(
    outcome: SupervisorOutcome,
    request_id: &str,
) -> Result<SketchSolveResponse, WorkerError> {
    match outcome {
        SupervisorOutcome::Completed { result, .. } => {
            serde_json::from_value(result).map_err(|error| WorkerError::Malformed {
                detail: error.to_string(),
            })
        }
        SupervisorOutcome::ForceTerminated { record } => {
            if let (Some(code), Some(detail)) = (record.failed_code, record.failed_detail) {
                Err(WorkerError::Diagnostic(SlvsDiagnostic { code, detail }))
            } else {
                Err(WorkerError::Supervised {
                    stage: record.stage,
                    request_id: request_id.to_string(),
                })
            }
        }
        SupervisorOutcome::Acknowledged { .. } => Err(WorkerError::Supervised {
            stage: "cancelled".to_string(),
            request_id: request_id.to_string(),
        }),
    }
}

impl WorkerProcess for SlvsWorker {
    fn spawn(config: WorkerConfig) -> Result<Box<dyn WorkerHost>, ProtocolWorkerError> {
        let binary = config.command_line.first().ok_or_else(|| {
            ProtocolWorkerError::Io(std::io::Error::other("empty worker command line"))
        })?;
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

pub fn schema_version() -> &'static str {
    SCHEMA_VERSION
}

pub fn new_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    format!("slvs-{nanos}-{}", std::process::id())
}

pub fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.workers.slvs/1");
    }

    #[test]
    fn binary_identity_rejects_a_substituted_worker_before_spawn() {
        let worker_path =
            std::env::temp_dir().join(format!("threeterm-slvs-identity-{}", new_request_id()));
        std::fs::write(&worker_path, b"not the approved worker").expect("worker fixture writes");
        let worker = SlvsWorker::with_binary_path(worker_path.clone())
            .with_expected_binary_sha256("0".repeat(64));

        let error = worker
            .verify_identity()
            .expect_err("substituted worker must fail closed");

        assert!(matches!(error, WorkerError::IdentityMismatch { .. }));
        let _ = std::fs::remove_file(worker_path);
    }

    #[test]
    fn rectangle_request_rejects_duplicate_constraint_entity_ids() {
        let request = SketchSolveRequest::new(
            "req-1",
            "sketch-1",
            vec![SketchEntity::Point {
                id: "p1".into(),
                x: 0.0,
                y: 0.0,
            }],
            vec![SketchConstraint {
                id: "p1".into(),
                kind: "fixed".into(),
                entities: vec!["p1".into()],
                value: None,
            }],
        );
        assert!(request.validate().is_err());
    }
}

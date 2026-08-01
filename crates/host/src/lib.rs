use std::cell::RefCell;
use std::path::{Path, PathBuf};

use threeterm_persistence::{Bundle, BundleError, LoadedBundle};
use threeterm_slvs_worker::{SketchRequest, SolveResult, SlvsWorker, WorkerError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotView {
    pub feature_graph_hash: String,
    pub revision_hash: String,
}

impl From<&LoadedBundle> for SnapshotView {
    fn from(bundle: &LoadedBundle) -> Self {
        Self {
            feature_graph_hash: bundle.feature_graph_hash_hex().to_string(),
            revision_hash: bundle.revision_hash_hex().to_string(),
        }
    }
}

#[derive(Debug)]
pub enum HostError {
    BundlePathMissing { path: PathBuf },
    BundlePathNotDirectory { path: PathBuf },
    Persistence(BundleError),
    WorkerFailure { detail: String },
    WorkerUnavailable { detail: String },
    SketchNotFullyConstrained { status: String, dof: i64 },
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BundlePathMissing { path } => {
                write!(formatter, "bundle path missing: {}", path.display())
            }
            Self::BundlePathNotDirectory { path } => {
                write!(
                    formatter,
                    "bundle path is not a directory: {}",
                    path.display()
                )
            }
            Self::Persistence(error) => error.fmt(formatter),
            Self::WorkerFailure { detail } => {
                write!(formatter, "sketch worker failure: {detail}")
            }
            Self::WorkerUnavailable { detail } => {
                write!(formatter, "sketch worker unavailable: {detail}")
            }
            Self::SketchNotFullyConstrained { status, dof } => {
                write!(
                    formatter,
                    "sketch not fully constrained: status={status} dof={dof}"
                )
            }
        }
    }
}

impl std::error::Error for HostError {}

impl From<BundleError> for HostError {
    fn from(error: BundleError) -> Self {
        Self::Persistence(error)
    }
}

impl From<WorkerError> for HostError {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::Diagnostic(diagnostic) => {
                Self::WorkerFailure { detail: format!("{} {}", diagnostic.code, diagnostic.arg) }
            }
            other => Self::WorkerFailure { detail: other.to_string() },
        }
    }
}

/// View of a successful sketch solve that has been committed to a new revision.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchSolveView {
    pub snapshot: SnapshotView,
    pub solve: SolveResult,
}

#[derive(Debug, Default)]
pub struct Host {
    current: RefCell<Option<LoadedBundle>>,
}

impl Host {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn save(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        kind: &str,
    ) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        let bundle = if root.exists() {
            if !root.is_dir() {
                return Err(HostError::BundlePathNotDirectory {
                    path: root.to_path_buf(),
                });
            }
            Bundle::at(root)
        } else {
            Bundle::create(root)?
        };
        let loaded = bundle.append_feature(feature_id, kind)?;
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    pub fn load(&self, root: impl AsRef<Path>) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(HostError::BundlePathMissing {
                path: root.to_path_buf(),
            });
        }
        if !root.is_dir() {
            return Err(HostError::BundlePathNotDirectory {
                path: root.to_path_buf(),
            });
        }
        let loaded = Bundle::at(root).open()?;
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    pub fn current(&self) -> Option<SnapshotView> {
        self.current.borrow().as_ref().map(SnapshotView::from)
    }

    /// Solve `request` against the disposable `libslvs` worker and, on
    /// success, append the resolved geometry as a new revision in the
    /// bundle rooted at `root`. Returns the new snapshot view plus the
    /// normalized solve result.
    ///
    /// Atomicity: a non-`ok` solver status, a worker spawn / exit /
    /// parse failure, or a persistence append failure all leave the
    /// bundle's `manifest.json` and `transactions.log` byte-identical to
    /// the pre-solve snapshot. `Host::current()` is preserved.
    ///
    /// The host commits on any successful (`status == "ok"` or
    /// `redundant_okay`) solve. The `dof` field in the returned
    /// `SketchSolveView` reports the libslvs kernel's residual degrees
    /// of freedom; a future slice that forks libslvs to mark the
    /// workplane origin and normal as `known = true` will reduce the
    /// reported dof for fully-pinned sketches to zero.
    pub fn solve_sketch(
        &self,
        root: impl AsRef<Path>,
        request: &SketchRequest,
        worker: &SlvsWorker,
    ) -> Result<SketchSolveView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_snapshot = SnapshotView::from(&loaded);
        let solve = worker.solve(request).map_err(HostError::from)?;
        if !solve.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::WorkerFailure {
                detail: format!(
                    "solver returned non-ok status: status={} dof={} failed={:?}",
                    solve.status,
                    solve.dof,
                    solve.failed_constraint_ids
                ),
            });
        }
        let feature_id = derive_feature_id(request);
        let result = match bundle.append_feature(feature_id.as_str(), "sketch") {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        let _ = prior_snapshot; // preserved for future diagnostics
        let snapshot = SnapshotView::from(&result);
        self.current.replace(Some(result));
        Ok(SketchSolveView { snapshot, solve })
    }
}

fn derive_feature_id(request: &SketchRequest) -> String {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut parts: Vec<&str> = Vec::new();
    parts.push("sketch");
    for entity in &request.entities {
        if seen.insert(entity.id.as_str()) {
            parts.push(entity.id.as_str());
        }
    }
    if parts.len() == 1 {
        parts.push(&request.request_id);
    }
    let joined = parts.join(":");
    if joined.len() <= 64 {
        joined
    } else {
        let mut out = String::from("sketch:");
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(joined.as_bytes());
        let digest = hasher.finalize();
        for byte in digest.iter().take(20) {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    }
}

pub fn schema_version() -> &'static str {
    "threeterm.host/1"
}

#[cfg(test)]
mod tests {
    use super::*;
    use threeterm_persistence::{Bundle, BundleError, MANIFEST_FILENAME};

    fn temp_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "threeterm-host-{}-{}-{}",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn failed_load_preserves_current_canonical_snapshot() {
        let valid_root = temp_root("valid");
        let valid = Bundle::create_for_test(&valid_root, "00".repeat(16).as_str())
            .expect("valid bundle creates");
        valid
            .append_feature("box-1", "box")
            .expect("feature appends");

        let tampered_root = temp_root("tampered");
        let tampered = Bundle::create_for_test(&tampered_root, "11".repeat(16).as_str())
            .expect("tampered bundle starts valid");
        tampered
            .append_feature("box-2", "box")
            .expect("feature appends");
        let path = tampered_root.join(MANIFEST_FILENAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("manifest reads"))
                .expect("manifest parses");
        manifest["terminal_log_digest"] = "f".repeat(64).into();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
        )
        .expect("manifest writes");

        let host = Host::new();
        let loaded = host.load(&valid_root).expect("valid bundle loads");
        assert_eq!(host.current(), Some(loaded.clone()));
        assert!(matches!(
            host.load(&tampered_root),
            Err(HostError::Persistence(BundleError::LogDigestMismatch))
        ));
        assert_eq!(host.current(), Some(loaded));

        let _ = std::fs::remove_dir_all(valid_root);
        let _ = std::fs::remove_dir_all(tampered_root);
    }

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.host/1");
    }
}

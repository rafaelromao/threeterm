use std::cell::RefCell;
use std::path::{Path, PathBuf};

use threeterm_persistence::{Bundle, BundleError, LoadedBundle};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    BundlePathMissing { path: PathBuf },
    BundlePathNotDirectory { path: PathBuf },
    Persistence(BundleError),
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
        }
    }
}

impl std::error::Error for HostError {}

impl From<BundleError> for HostError {
    fn from(error: BundleError) -> Self {
        Self::Persistence(error)
    }
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
        assert_eq!(
            host.load(&tampered_root),
            Err(HostError::Persistence(BundleError::LogDigestMismatch))
        );
        assert_eq!(host.current(), Some(loaded));

        let _ = std::fs::remove_dir_all(valid_root);
        let _ = std::fs::remove_dir_all(tampered_root);
    }

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.host/1");
    }
}

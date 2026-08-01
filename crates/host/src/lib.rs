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

#[derive(Debug)]
pub enum HostError {
    BundlePathMissing { path: PathBuf },
    BundlePathNotDirectory { path: PathBuf },
    Validation { detail: String },
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
            Self::Validation { detail } => write!(formatter, "host.validation: {detail}"),
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

    /// Persist an L-bracket into `root` by appending the two plate features
    /// (`<bracket_id>-plate-vertical` and `<bracket_id>-plate-horizontal`)
    /// atomically. Returns the post-write `SnapshotView` and updates the
    /// canonical current snapshot.
    ///
    /// The numeric dimensions are validated here so both the CLI and MCP
    /// transports enforce the same contract end-to-end. The dimensions
    /// themselves are not yet persisted on the canonical transaction log
    /// in this slice — that is the responsibility of a future worker
    /// slice that will round-trip dimensions through the geometric
    /// kernel. The host intentionally records only the two plate features
    /// so the canonical state stays stable until OCCT geometry is wired
    /// in. The four dimensions must each be strictly positive finite
    /// numbers; a zero, negative, NaN, or infinite value would describe a
    /// degenerate solid or corrupt the canonical log, so the host rejects
    /// those inputs up-front.
    pub fn save_bracket(
        &self,
        root: impl AsRef<Path>,
        bracket_id: &str,
        length: f64,
        width: f64,
        height: f64,
        thickness: f64,
    ) -> Result<SnapshotView, HostError> {
        if bracket_id.is_empty() {
            return Err(HostError::Validation {
                detail: "bracket_id must not be empty".to_string(),
            });
        }
        for (name, value) in [
            ("length", length),
            ("width", width),
            ("height", height),
            ("thickness", thickness),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(HostError::Validation {
                    detail: format!(
                        "{name} must be a strictly positive finite number, got {value}"
                    ),
                });
            }
        }

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
        let vertical_id = format!("{bracket_id}-plate-vertical");
        let horizontal_id = format!("{bracket_id}-plate-horizontal");
        let entries = [
            (vertical_id.as_str(), "plate-vertical"),
            (horizontal_id.as_str(), "plate-horizontal"),
        ];
        let loaded = bundle.append_features(&entries)?;
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

    #[test]
    fn save_bracket_appends_two_plate_features_and_preserves_canonical_state() {
        let root = temp_root("bracket");
        let host = Host::new();
        let view = host
            .save_bracket(&root, "l-1", 60.0, 30.0, 40.0, 3.0)
            .expect("save_bracket succeeds");
        assert_eq!(host.current(), Some(view.clone()));
        let manifest_path = root.join(threeterm_persistence::MANIFEST_FILENAME);
        let manifest_bytes = std::fs::read(&manifest_path).expect("manifest is readable");
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).expect("manifest parses");
        assert!(manifest.is_object());
        assert_eq!(
            manifest["transaction_count"], 2,
            "save_bracket must record exactly two transactions"
        );
        let transactions =
            std::fs::read_to_string(root.join(threeterm_persistence::TRANSACTIONS_LOG_FILENAME))
                .expect("canonical transaction log is readable");
        assert!(transactions.contains("plate-vertical"));
        assert!(transactions.contains("plate-horizontal"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn save_bracket_does_not_mutate_a_tampered_bundle() {
        let root = temp_root("tampered-bracket");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        bundle
            .append_feature("seed-box", "box")
            .expect("seed feature appends");
        let manifest_path = root.join(MANIFEST_FILENAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).expect("manifest reads"))
                .expect("manifest parses");
        manifest["terminal_log_digest"] = "f".repeat(64).into();
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
        )
        .expect("manifest writes");

        let host = Host::new();
        let result = host.save_bracket(&root, "l-1", 60.0, 30.0, 40.0, 3.0);
        assert!(
            matches!(
                result,
                Err(HostError::Persistence(BundleError::LogDigestMismatch))
            ),
            "tampered bundle must surface a LogDigestMismatch, got {result:?}"
        );
        assert!(host.current().is_none());

        let _ = std::fs::remove_dir_all(root);
    }
}

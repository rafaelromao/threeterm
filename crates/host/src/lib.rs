//! ThreeTerm host: the seam that owns the canonical Revision Snapshot and
//! guards the contract that a failed load never mutates canonical state.
//!
//! The slice (#235) ships:
//!
//! - [`Host::new`] — a host with no current bundle loaded.
//! - [`Host::save`] — append a feature to a bundle on disk, replacing the
//!   host's `current` snapshot.
//! - [`Host::load`] — read & verify a bundle on disk, replacing the host's
//!   `current` snapshot ONLY on success.
//! - [`Host::current`] — read-only access to the canonical snapshot.
//!
//! The host is single-threaded; the canonical state lives behind a
//! [`RefCell`] so the public surface takes `&self` and the contract is
//! easy to verify in unit tests.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use threeterm_persistence::{Bundle, BundleError, LoadedBundle};

/// Snapshot view returned by the host's `save` / `load` methods. The
/// field names mirror the response JSON keys so the CLI can serialize
/// them directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotView {
    pub feature_graph_hash_hex: String,
    pub revision_hash_hex: String,
}

impl SnapshotView {
    pub fn from_loaded(loaded: &LoadedBundle) -> Self {
        Self {
            feature_graph_hash_hex: loaded.feature_graph_hash_hex().to_string(),
            revision_hash_hex: loaded.revision_hash_hex().to_string(),
        }
    }
}

/// All errors surfaced from the host to the CLI. The CLI translates each
/// `HostError` variant into a structured `Diagnostic` envelope on stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The bundle path is missing or not a directory.
    BundlePathMissing { path: PathBuf },
    /// A persistence-layer error during save or load.
    Persistence(BundleError),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::BundlePathMissing { path } => {
                write!(f, "bundle_path_missing: {}", path.display())
            }
            HostError::Persistence(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<BundleError> for HostError {
    fn from(err: BundleError) -> Self {
        HostError::Persistence(err)
    }
}

/// Owns the single mutable `LoadedBundle` snapshot for the lifetime of
/// the host.
#[derive(Debug)]
pub struct Host {
    current: RefCell<Option<LoadedBundle>>,
    /// Echo of the bundle root that produced `current`. Used by the CLI
    /// when `load` doesn't carry an explicit request to switch roots.
    current_root: RefCell<Option<PathBuf>>,
}

impl Host {
    pub fn new() -> Self {
        Self {
            current: RefCell::new(None),
            current_root: RefCell::new(None),
        }
    }

    /// Open or create the bundle at `root`, append the feature, and
    /// replace the host's `current` snapshot. Returns the new
    /// [`SnapshotView`] for the CLI to print.
    pub fn save(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        kind: &str,
    ) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        if !root.exists() || !root.is_dir() {
            return Err(HostError::BundlePathMissing {
                path: root.to_path_buf(),
            });
        }

        let bundle = if root.join("manifest.json").exists() {
            Bundle::at(root)
        } else {
            Bundle::create(root)?
        };

        let loaded = bundle.append_feature(feature_id, kind)?;
        let view = SnapshotView::from_loaded(&loaded);
        *self.current.borrow_mut() = Some(loaded);
        *self.current_root.borrow_mut() = Some(root.to_path_buf());
        Ok(view)
    }

    /// Read & integrity-verify the bundle at `root`, then replace the
    /// host's `current` snapshot. On failure, `current` is NOT mutated;
    /// the canonical state preserved contract holds.
    pub fn load(&self, root: impl AsRef<Path>) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(HostError::BundlePathMissing {
                path: root.to_path_buf(),
            });
        }
        if !root.is_dir() {
            return Err(HostError::BundlePathMissing {
                path: root.to_path_buf(),
            });
        }

        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let view = SnapshotView::from_loaded(&loaded);
        *self.current.borrow_mut() = Some(loaded);
        *self.current_root.borrow_mut() = Some(root.to_path_buf());
        Ok(view)
    }

    /// Constructor for tests: install a known-good snapshot without
    /// touching the disk.
    #[cfg(test)]
    pub fn with_current(loaded: LoadedBundle) -> Self {
        Self {
            current: RefCell::new(Some(loaded)),
            current_root: RefCell::new(None),
        }
    }

    /// Read-only view of the canonical state. `None` if the host has
    /// never successfully loaded or saved a bundle.
    pub fn current(&self) -> Option<SnapshotView> {
        self.current
            .borrow()
            .as_ref()
            .map(SnapshotView::from_loaded)
    }

    /// Path of the bundle that last produced `current`.
    pub fn current_root(&self) -> Option<PathBuf> {
        self.current_root.borrow().clone()
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use threeterm_persistence::{
        Bundle, LoadedBundle, PROJECT_GENERATION_HEX_LEN,
    };

    fn counter() -> &'static AtomicU64 {
        static COUNTER: OnceLock<AtomicU64> = OnceLock::new();
        COUNTER.get_or_init(|| AtomicU64::new(0))
    }

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "threeterm-235-host-{}-{}-{}",
            std::process::id(),
            label,
            counter().fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("temp_root create");
        dir
    }

    fn empty_loaded() -> LoadedBundle {
        let root = temp_root("empty_loaded");
        let bundle = Bundle::create_for_test(&root, "00".repeat(PROJECT_GENERATION_HEX_LEN / 2).as_str())
            .expect("create_for_test");
        let loaded = bundle.open().expect("open");
        let _ = std::fs::remove_dir_all(&root);
        loaded
    }

    #[test]
    fn new_host_has_no_current_snapshot() {
        let host = Host::new();
        assert!(host.current().is_none());
    }

    #[test]
    fn failed_load_does_not_mutate_current() {
        let root = temp_root("failed_load");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str())
            .expect("create");
        let loaded = bundle.append_feature("box-1", "box").expect("append");
        let prior_view = SnapshotView::from_loaded(&loaded);

        // Tamper with the manifest's terminal_log_digest so load fails.
        let manifest_path = root.join("manifest.json");
        let raw = std::fs::read_to_string(&manifest_path).expect("manifest readable");
        let mut value: serde_json::Value =
            serde_json::from_str(&raw).expect("manifest is parseable JSON");
        let terminal = value["terminal_log_digest_hex"]
            .as_str()
            .expect("terminal is a string")
            .to_string();
        let flipped = match terminal.chars().next() {
            Some('0') => format!("1{}", &terminal[1..]),
            Some(_) => format!("2{}", &terminal[1..]),
            None => "1".repeat(64),
        };
        *value.get_mut("terminal_log_digest_hex").unwrap() =
            serde_json::Value::from(flipped);
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&value).expect("re-serialize"),
        )
        .expect("manifest rewritten");

        let host = Host::with_current(loaded);
        let prior = host.current().expect("host has prior snapshot");
        assert_eq!(prior, prior_view, "host current is the prior view");

        let err = host.load(&root).expect_err("tampered load is rejected");
        match err {
            HostError::Persistence(BundleError::LogDigestMismatch) => {}
            other => panic!("expected LogDigestMismatch, got {other:?}"),
        }

        let after = host.current().expect("host still has prior snapshot");
        assert_eq!(
            after, prior,
            "canonical state preserved across a failed load"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_save_creates_bundle_then_replaces_current() {
        let root = temp_root("save_creates");
        let host = Host::new();
        let view = host
            .save(&root, "box-1", "box")
            .expect("first save creates the bundle");
        assert_eq!(view.feature_graph_hash_hex.len(), 64);
        assert_eq!(view.revision_hash_hex.len(), 64);
        assert_eq!(host.current().unwrap(), view);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_save_then_load_round_trip_yields_same_view() {
        let root = temp_root("save_load");
        let host = Host::new();
        let saved = host
            .save(&root, "box-1", "box")
            .expect("save");
        let loaded = host.load(&root).expect("load");
        assert_eq!(
            saved, loaded,
            "the same view is recoverable through the host"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_save_missing_path_returns_bundle_path_missing() {
        let root = temp_root("missing_path").join("does-not-exist");
        let host = Host::new();
        let err = host.save(&root, "box-1", "box").expect_err("path missing");
        match err {
            HostError::BundlePathMissing { .. } => {}
            other => panic!("expected BundlePathMissing, got {other:?}"),
        }
    }

    #[test]
    fn empty_loaded_smoke() {
        let _ = empty_loaded();
    }
}

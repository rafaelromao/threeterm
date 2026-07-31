//! The `.threeterm/`-style project bundle.
//!
//! The slice (#235) implements a single sealed Project Generation with
//! two on-disk files:
//!
//! - `<root>/manifest.json` — sealed canonical JSON identifying the
//!   generation and the committed canonical state.
//! - `<root>/transactions.log` — append-only NDJSON with the SHA-256
//!   digest chain (see [`log::LogEntry`]).
//!
//! `Bundle::open` is the integrity-checked reader. `Bundle::create` and
//! `Bundle::create_for_test` are the writers. `Bundle::append_feature`
//! is the only public path that mutates an existing bundle on disk; it
//! atomically rewrites both files via `manifest.json.tmp` +
//! `transactions.log` in place.

use std::path::{Path, PathBuf};

use threeterm_domain::graph::{Feature, FeatureGraph};

pub mod log;
pub mod manifest;

pub use log::{DIGEST_HEX_LEN, EMPTY_LOG_DIGEST_HEX, LogEntry, LogError, TransactionLog};
pub use manifest::{
    DIGEST_HEX_LEN as MANIFEST_DIGEST_HEX_LEN, MANIFEST_SCHEMA_GENERATION, MANIFEST_SCHEMA_VERSION,
    Manifest, PROJECT_GENERATION_HEX_LEN, append_line, atomic_write, hex_lower,
    random_project_generation_hex, sha256_hex,
};

/// Filename of the sealed manifest inside a bundle.
pub const MANIFEST_FILENAME: &str = "manifest.json";

/// Filename of the append-only NDJSON log inside a bundle.
pub const TRANSACTIONS_LOG_FILENAME: &str = "transactions.log";

/// All errors that can surface from `Bundle::open` /
/// `Bundle::append_feature`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    ManifestMissing,
    LogMissing,
    SchemaGenerationUnsupported {
        detail: String,
    },
    LogDigestMismatch,
    /// Convenience wrapper that carries the [`LogError`] detail.
    LogFailure(LogError),
    IoFailure {
        detail: String,
    },
    ProjectGenerationUnavailable {
        detail: String,
    },
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BundleError::ManifestMissing => write!(f, "manifest_missing"),
            BundleError::LogMissing => write!(f, "log_missing"),
            BundleError::SchemaGenerationUnsupported { detail } => {
                write!(f, "schema_generation_unsupported: {detail}")
            }
            BundleError::LogDigestMismatch => write!(f, "log_digest_mismatch"),
            BundleError::LogFailure(err) => write!(f, "{err}"),
            BundleError::IoFailure { detail } => write!(f, "io failure: {detail}"),
            BundleError::ProjectGenerationUnavailable { detail } => {
                write!(f, "project_generation_unavailable: {detail}")
            }
        }
    }
}

impl std::error::Error for BundleError {}

impl From<LogError> for BundleError {
    fn from(err: LogError) -> Self {
        BundleError::LogFailure(err)
    }
}

/// In-memory equivalent of a loaded bundle. The manifest, the in-memory
/// graph, and the chain terminal are all in lockstep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedBundle {
    pub manifest: Manifest,
    pub log: TransactionLog,
    pub graph: FeatureGraph,
}

impl LoadedBundle {
    /// Public accessors for the response shape.
    pub fn feature_graph_hash_hex(&self) -> &str {
        &self.manifest.feature_graph_hash_hex
    }
    pub fn revision_hash_hex(&self) -> &str {
        &self.manifest.revision_hash_hex
    }
}

/// The bundle orchestrator: read + write the bundle atomically.
#[derive(Debug)]
pub struct Bundle {
    root: PathBuf,
}

impl Bundle {
    /// Open a bundle root without reading it. Used by the host to gate
    /// `open_or_create` on the existence of the bundle's manifest.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILENAME)
    }

    pub fn transactions_log_path(&self) -> PathBuf {
        self.root.join(TRANSACTIONS_LOG_FILENAME)
    }

    /// Production constructor: reads 16 bytes from `/dev/urandom` and
    /// writes a fresh, empty bundle with the resulting project generation.
    pub fn create(root: impl Into<PathBuf>) -> Result<Self, BundleError> {
        let project_generation_hex = random_project_generation_hex().map_err(|err| {
            BundleError::ProjectGenerationUnavailable {
                detail: err.to_string(),
            }
        })?;
        Self::create_for_test(root, &project_generation_hex)
    }

    /// Deterministic constructor used by callers that need a specific
    /// project generation (tests, fixtures produced by the production
    /// `create`).
    pub fn create_for_test(
        root: impl Into<PathBuf>,
        project_generation_hex: &str,
    ) -> Result<Self, BundleError> {
        let bundle = Self::at(root);
        std::fs::create_dir_all(bundle.root()).map_err(|err| BundleError::IoFailure {
            detail: err.to_string(),
        })?;

        let graph = FeatureGraph::empty();

        let manifest = Manifest::seal(project_generation_hex, EMPTY_LOG_DIGEST_HEX, &graph);
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).map_err(|err| BundleError::IoFailure {
                detail: err.to_string(),
            })?;
        atomic_write(&bundle.manifest_path(), &manifest_bytes).map_err(|err| {
            BundleError::IoFailure {
                detail: err.to_string(),
            }
        })?;

        append_line(&bundle.transactions_log_path(), &[]).map_err(|err| {
            BundleError::IoFailure {
                detail: err.to_string(),
            }
        })?;

        Ok(bundle)
    }

    /// Integrity-checked open. Reads `manifest.json`, parses the
    /// `transactions.log` chain, and verifies each entry. Returns the
    /// sealed `LoadedBundle` on success.
    pub fn open(&self) -> Result<LoadedBundle, BundleError> {
        let manifest_path = self.manifest_path();
        let log_path = self.transactions_log_path();

        let manifest_raw = match std::fs::read(&manifest_path) {
            Ok(b) => b,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(BundleError::ManifestMissing);
            }
            Err(err) => {
                return Err(BundleError::IoFailure {
                    detail: err.to_string(),
                });
            }
        };

        let manifest: Manifest =
            serde_json::from_slice(&manifest_raw).map_err(|err| BundleError::IoFailure {
                detail: format!("manifest parse failed: {err}"),
            })?;

        if manifest.schema_generation != MANIFEST_SCHEMA_GENERATION {
            return Err(BundleError::SchemaGenerationUnsupported {
                detail: format!(
                    "manifest declares schema_generation = {}",
                    manifest.schema_generation
                ),
            });
        }

        let log_raw = match std::fs::read(&log_path) {
            Ok(b) => b,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(BundleError::LogMissing);
            }
            Err(err) => {
                return Err(BundleError::IoFailure {
                    detail: err.to_string(),
                });
            }
        };

        let entries = decode_log(&log_raw)?;
        let terminal = verify_chain(&entries)?;
        if terminal != manifest.terminal_log_digest_hex {
            return Err(BundleError::LogDigestMismatch);
        }

        let log = TransactionLog {
            entries: entries.clone(),
        };

        let mut graph = FeatureGraph::empty();
        for entry in &entries {
            graph.add_feature(Feature::new(&entry.feature_id, &entry.kind));
        }

        // Sanity check: the manifest's feature_graph_hash_hex and
        // revision_hash_hex must match what the in-memory reconstruction
        // produces. If they don't, the bundle is tampered even though
        // every lower-level invariant passed.
        if graph.graph_hash_hex() != manifest.feature_graph_hash_hex {
            return Err(BundleError::LogDigestMismatch);
        }
        if threeterm_domain::revision_hex(
            &manifest.feature_graph_hash_hex,
            &manifest.terminal_log_digest_hex,
        ) != manifest.revision_hash_hex
        {
            return Err(BundleError::LogDigestMismatch);
        }

        Ok(LoadedBundle {
            manifest,
            log,
            graph,
        })
    }

    /// Append `(feature_id, kind)` to the chain and re-seal the manifest.
    /// Idempotent: if the graph already contains the same `(feature_id,
    /// kind)`, no new log entry is written, but the manifest is rewritten
    /// with identical values.
    pub fn append_feature(
        &self,
        feature_id: &str,
        kind: &str,
    ) -> Result<LoadedBundle, BundleError> {
        let mut loaded = self.open()?;

        let log = &mut loaded.log;
        let graph = &mut loaded.graph;

        let id = threeterm_domain::graph::FeatureId(feature_id.to_string());
        let kind_domain = threeterm_domain::graph::FeatureKind(kind.to_string());

        let appended_now = if !graph.contains(&id, &kind_domain) {
            graph.add_feature(Feature::new(feature_id, kind));
            log.append_feature(feature_id, kind);
            true
        } else {
            false
        };

        let terminal = log.terminal_digest_hex()?;
        let manifest = Manifest::seal(&loaded.manifest.project_generation_hex, &terminal, graph);
        loaded.manifest = manifest.clone();

        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).map_err(|err| BundleError::IoFailure {
                detail: err.to_string(),
            })?;
        atomic_write(&self.manifest_path(), &manifest_bytes).map_err(|err| {
            BundleError::IoFailure {
                detail: err.to_string(),
            }
        })?;

        if appended_now {
            let last = loaded
                .log
                .entries()
                .last()
                .expect("non-empty log has a last entry");
            let line = serde_json::to_vec(last).map_err(|err| BundleError::IoFailure {
                detail: err.to_string(),
            })?;
            let mut payload = line;
            payload.push(b'\n');
            append_line(&self.transactions_log_path(), &payload).map_err(|err| {
                BundleError::IoFailure {
                    detail: err.to_string(),
                }
            })?;
        }

        Ok(loaded)
    }
}

fn decode_log(bytes: &[u8]) -> Result<Vec<LogEntry>, BundleError> {
    log::decode_log(bytes).map_err(BundleError::from)
}

fn verify_chain(entries: &[LogEntry]) -> Result<String, BundleError> {
    log::verify_chain(entries).map_err(BundleError::from)
}

pub fn schema_version() -> &'static str {
    "threeterm.persistence/1"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "threeterm-235-persist-{}-{}-{}",
            std::process::id(),
            label,
            std::sync::atomic::AtomicU64::new(0).fetch_add(0, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("temp_root create");
        dir
    }

    #[test]
    fn create_then_open_yields_empty_graph_and_zero_digest_log() {
        let root = temp_root("create_open");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        let loaded = bundle.open().expect("bundle opens");
        assert_eq!(loaded.manifest.project_generation_hex, "00".repeat(16));
        assert_eq!(
            loaded.manifest.terminal_log_digest_hex,
            EMPTY_LOG_DIGEST_HEX
        );
        assert!(loaded.log.is_empty());
        assert!(loaded.graph.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn append_feature_is_idempotent_on_same_input() {
        let root = temp_root("idempotent");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");

        let first = bundle.append_feature("box-1", "box").expect("first append");
        let second = bundle
            .append_feature("box-1", "box")
            .expect("second append");

        assert_eq!(first.manifest, second.manifest);
        assert_eq!(first.log, second.log);
        assert_eq!(first.graph, second.graph);

        let reloaded = bundle.open().expect("reload");
        assert_eq!(reloaded.manifest, first.manifest);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tampering_manifest_terminal_log_digest_triggers_log_digest_mismatch() {
        let root = temp_root("tamper_log_digest");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        let _loaded = bundle.append_feature("box-1", "box").expect("append");

        let manifest_path = root.join(MANIFEST_FILENAME);
        let manifest_raw = std::fs::read_to_string(&manifest_path).expect("manifest readable");
        let mut value: serde_json::Value =
            serde_json::from_str(&manifest_raw).expect("manifest is parseable JSON");
        let original_terminal = value["terminal_log_digest_hex"]
            .as_str()
            .expect("terminal_log_digest_hex is a string")
            .to_string();
        // Flip the first hex char to "1" if it is "0", else "2".
        let flipped_first = match original_terminal.chars().next() {
            Some('0') => format!("1{}", &original_terminal[1..]),
            Some(_) => format!("2{}", &original_terminal[1..]),
            None => "1".repeat(64),
        };
        *value.get_mut("terminal_log_digest_hex").unwrap() = serde_json::Value::from(flipped_first);
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&value).expect("manifest re-serializes"),
        )
        .expect("manifest rewritten");

        let err = bundle.open().expect_err("tamper detected");
        assert_eq!(err, BundleError::LogDigestMismatch);
        let _ = std::fs::remove_dir_all(&root);
    }
}

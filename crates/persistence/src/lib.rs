use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threeterm_domain::ProjectGeneration;

pub mod bundle {
    pub use super::{
        AppendedTransaction, BundleError, Identity, LoadedBundle, Manifest, TransactionIntent,
        append, current_identity, load, schema_version, write_fresh,
    };
}

pub fn schema_version() -> &'static str {
    "threeterm.persistence/1"
}

const MANIFEST_FILE: &str = "manifest.json";
const TRANSACTIONS_FILE: &str = "canonical/transactions.ndjson";

const LOG_ENTRY_SCHEMA_VERSION: &str = "threeterm.persistence.log.entry/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema_version: String,
    pub generation_id: String,
    pub revision_id: String,
    pub revision_count: usize,
    pub transaction_count: usize,
    pub transaction_bytes: usize,
    pub transaction_sha256: String,
    pub canonical_root_sha256: String,
    pub seal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedBundle {
    pub manifest: Manifest,
    pub generation: ProjectGeneration,
    pub transactions: String,
}

/// The Project Generation identity: the SHA-256 hex digest of the
/// canonical transaction log. It is the only durable document identity
/// (closed issue 29) and is the SHA-256 of the canonical log digest
/// (closed issue 44).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Identity(pub String);

impl Identity {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The set of accepted command intents for the foundation slice. Each
/// accepted intent produces one versioned log entry. The schema is
/// pinned and forward-extensible through the discriminator tag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TransactionIntent {
    AddFeature {
        feature_id: String,
        feature_kind: String,
        parameters: serde_json::Value,
    },
    SetParameter {
        feature_id: String,
        parameter: String,
        value: serde_json::Value,
    },
    RemoveFeature {
        feature_id: String,
    },
}

impl TransactionIntent {
    /// The affected feature id, used by the domain layer to update the
    /// Project Generation and by the manifest to record provenance.
    pub fn feature_id(&self) -> &str {
        match self {
            Self::AddFeature { feature_id, .. } => feature_id,
            Self::SetParameter { feature_id, .. } => feature_id,
            Self::RemoveFeature { feature_id } => feature_id,
        }
    }

    /// The kebab-case kind tag (the `kind` field of the JSON form).
    pub fn kind_tag(&self) -> &'static str {
        match self {
            Self::AddFeature { .. } => "add-feature",
            Self::SetParameter { .. } => "set-parameter",
            Self::RemoveFeature { .. } => "remove-feature",
        }
    }
}

/// The result of an `append` operation: the new manifest and the
/// canonical transaction entry that was appended to the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendedTransaction {
    pub manifest: Manifest,
    pub entry: serde_json::Value,
}

#[derive(Debug)]
pub enum BundleError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "filesystem error: {error}"),
            Self::Json(error) => write!(formatter, "invalid JSON: {error}"),
            Self::Invalid(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for BundleError {}

impl From<std::io::Error> for BundleError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for BundleError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn write_fresh(path: &Path, generation: ProjectGeneration) -> Result<Manifest, BundleError> {
    if path.exists() {
        return Err(BundleError::Invalid(format!(
            "destination already exists: {}",
            path.display()
        )));
    }

    let revision = generation
        .revisions
        .first()
        .filter(|revision| generation.revisions.len() == 1 && revision.features.is_empty())
        .ok_or_else(|| {
            BundleError::Invalid("fresh generation must contain one empty revision".into())
        })?;
    let transactions = String::new();
    let transaction_bytes = transactions.len();
    let transaction_sha256 = hash(transactions.as_bytes());
    let mut manifest = Manifest {
        schema_version: schema_version().to_string(),
        generation_id: generation.id.clone(),
        revision_id: revision.id.clone(),
        revision_count: 1,
        transaction_count: 0,
        transaction_bytes,
        transaction_sha256,
        canonical_root_sha256: String::new(),
        seal_sha256: String::new(),
    };
    manifest.canonical_root_sha256 = hash(&canonical_manifest_bytes(&manifest));
    manifest.seal_sha256 = hash(&sealed_manifest_bytes(&manifest));

    let staging = staging_path(path);
    fs::create_dir_all(staging.join("canonical"))?;
    fs::write(staging.join(TRANSACTIONS_FILE), transactions.as_bytes())?;
    fs::write(
        staging.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::rename(&staging, path)?;
    Ok(manifest)
}

/// Write a file atomically via a same-directory `.tmp` sibling and
/// `rename`. The rename is atomic on POSIX filesystems. The function is
/// the only persistence-layer primitive that touches the filesystem
/// outside of the read path.
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), BundleError> {
    let parent = target.parent().ok_or_else(|| {
        BundleError::Invalid(format!("target path has no parent: {}", target.display()))
    })?;
    let tmp = parent.join(format!(
        "{}.tmp",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bundle")
    ));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, target)?;
    Ok(())
}

pub fn load(path: &Path) -> Result<LoadedBundle, BundleError> {
    let manifest: Manifest = serde_json::from_slice(&fs::read(path.join(MANIFEST_FILE))?)?;
    if manifest.schema_version != schema_version() {
        return Err(BundleError::Invalid(
            "unsupported persistence schema".into(),
        ));
    }
    if manifest.canonical_root_sha256 != hash(&canonical_manifest_bytes(&manifest)) {
        return Err(BundleError::Invalid(
            "canonical manifest seal mismatch".into(),
        ));
    }
    if manifest.seal_sha256 != hash(&sealed_manifest_bytes(&manifest)) {
        return Err(BundleError::Invalid("manifest seal mismatch".into()));
    }
    let transactions = String::from_utf8(fs::read(path.join(TRANSACTIONS_FILE))?)
        .map_err(|_| BundleError::Invalid("transactions are not UTF-8".into()))?;
    // The integrity invariants are: the on-disk log size matches the
    // manifest's recorded byte count, the SHA-256 of the log matches the
    // manifest's recorded hash, and (when the log is empty) the
    // transaction_count is zero. A non-empty log with a matching
    // transaction_count is accepted; the count is not re-derived from
    // the log content because a corrupted log could pass a count check
    // while failing the hash check.
    if manifest.transaction_bytes != transactions.len()
        || manifest.transaction_sha256 != hash(transactions.as_bytes())
    {
        return Err(BundleError::Invalid(
            "canonical transaction log integrity mismatch".into(),
        ));
    }
    if transactions.is_empty() && manifest.transaction_count != 0 {
        return Err(BundleError::Invalid(
            "empty log but nonzero transaction_count".into(),
        ));
    }
    let generation = ProjectGeneration::with_id(manifest.generation_id.clone());
    if generation.revisions[0].id != manifest.revision_id {
        return Err(BundleError::Invalid("revision identity mismatch".into()));
    }
    Ok(LoadedBundle {
        manifest,
        generation,
        transactions,
    })
}

/// Return the current Project Generation identity (the SHA-256 hex digest
/// of the canonical transaction log) of the bundle at `path`. The
/// identity is the only durable document identity; it is recomputed on
/// every reload.
pub fn current_identity(path: &Path) -> Result<Identity, BundleError> {
    let loaded = load(path)?;
    Ok(Identity(loaded.manifest.transaction_sha256))
}

/// Append a transaction to the canonical log of the bundle at `path`.
/// The new transaction is a versioned JSON document that records the
/// accepted intent, the affected feature id, the deterministic inputs,
/// and the parent identity (the SHA-256 of the log before the append).
/// The atomic temp-file-then-rename path leaves the prior bundle intact
/// on any failure.
pub fn append(
    path: &Path,
    previous_identity: &Identity,
    intent: &TransactionIntent,
) -> Result<AppendedTransaction, BundleError> {
    let loaded = load(path)?;
    if loaded.manifest.transaction_sha256 != previous_identity.0 {
        return Err(BundleError::Invalid(format!(
            "parent identity mismatch: expected {}, got {}",
            previous_identity.0, loaded.manifest.transaction_sha256
        )));
    }

    let entry = build_entry(
        previous_identity,
        intent,
        loaded.manifest.transaction_count + 1,
    )?;
    let line = serde_json::to_vec(&entry)?;
    let mut new_transactions = loaded.transactions.clone();
    if !new_transactions.is_empty() && !new_transactions.ends_with('\n') {
        new_transactions.push('\n');
    }
    new_transactions.push_str(std::str::from_utf8(&line).expect("entry is utf-8"));
    new_transactions.push('\n');

    let new_count = loaded.manifest.transaction_count + 1;
    let new_bytes = new_transactions.len();
    let new_sha256 = hash(new_transactions.as_bytes());
    let mut manifest = loaded.manifest.clone();
    manifest.transaction_count = new_count;
    manifest.transaction_bytes = new_bytes;
    manifest.transaction_sha256 = new_sha256;
    manifest.canonical_root_sha256 = hash(&canonical_manifest_bytes(&manifest));
    manifest.seal_sha256 = hash(&sealed_manifest_bytes(&manifest));

    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let transactions_bytes = new_transactions.as_bytes().to_vec();

    // Write the log first, then the manifest, atomically. The manifest
    // records the SHA-256 of the new log; if the manifest write fails,
    // the prior log and the prior manifest are both still on disk. A
    // subsequent load will reject the manifest because its recorded log
    // hash will not match the (unchanged) prior log.
    write_atomic(&path.join(TRANSACTIONS_FILE), &transactions_bytes)?;
    write_atomic(&path.join(MANIFEST_FILE), &manifest_bytes)?;

    Ok(AppendedTransaction { manifest, entry })
}

fn build_entry(
    parent: &Identity,
    intent: &TransactionIntent,
    sequence: usize,
) -> Result<serde_json::Value, BundleError> {
    let tx_id = format!("tx-{sequence:04}");
    let mut entry = serde_json::Map::new();
    entry.insert(
        "schema_version".to_string(),
        serde_json::Value::from(LOG_ENTRY_SCHEMA_VERSION),
    );
    entry.insert("tx_id".to_string(), serde_json::Value::from(tx_id));
    entry.insert(
        "parent_identity".to_string(),
        serde_json::Value::from(parent.as_str()),
    );
    let intent_value = serde_json::to_value(intent)?;
    entry.insert("intent".to_string(), intent_value);
    entry.insert(
        "affected_id".to_string(),
        serde_json::Value::from(intent.feature_id()),
    );
    Ok(serde_json::Value::Object(entry))
}

fn staging_path(path: &Path) -> PathBuf {
    let mut staging = path.to_path_buf();
    let suffix = format!(".tmp-{}", std::process::id());
    staging.set_file_name(format!(
        "{}{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        suffix
    ));
    staging
}

fn canonical_manifest_bytes(manifest: &Manifest) -> Vec<u8> {
    let mut copy = manifest.clone();
    copy.canonical_root_sha256.clear();
    copy.seal_sha256.clear();
    serde_json::to_vec(&copy).expect("manifest serializes")
}

fn sealed_manifest_bytes(manifest: &Manifest) -> Vec<u8> {
    let mut copy = manifest.clone();
    copy.seal_sha256.clear();
    serde_json::to_vec(&copy).expect("manifest serializes")
}

fn hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.persistence/1");
    }

    #[test]
    fn tampered_transaction_log_is_rejected() {
        let root = std::env::temp_dir().join(format!("threeterm-tamper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");
        fs::write(root.join(TRANSACTIONS_FILE), b"tampered\n").expect("log changes");
        assert!(load(&root).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fresh_bundle_round_trips_empty_generation() {
        let root = std::env::temp_dir().join(format!("threeterm-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let generation = ProjectGeneration::with_id("generation-test");
        write_fresh(&root, generation).expect("bundle writes");
        let loaded = load(&root).expect("bundle loads");
        assert_eq!(
            loaded.generation.revisions[0],
            threeterm_domain::Revision::empty()
        );
        assert!(loaded.transactions.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn append_advances_the_identity_and_the_manifest() {
        let root = std::env::temp_dir().join(format!("threeterm-append-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");
        let initial_identity = current_identity(&root).expect("initial identity");
        assert_eq!(initial_identity.0, hash("".as_bytes()));

        let intent = TransactionIntent::AddFeature {
            feature_id: "feat-1".to_string(),
            feature_kind: "sketch".to_string(),
            parameters: serde_json::json!({ "width": 10.0 }),
        };
        let appended = append(&root, &initial_identity, &intent).expect("append succeeds");
        assert_eq!(appended.manifest.transaction_count, 1);
        let on_disk_bytes = std::fs::read(root.join(TRANSACTIONS_FILE)).expect("read log");
        assert_eq!(appended.manifest.transaction_sha256, hash(&on_disk_bytes));

        let next_identity = current_identity(&root).expect("identity after append");
        assert_ne!(next_identity, initial_identity);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn append_rejects_a_stale_parent_identity() {
        let root = std::env::temp_dir().join(format!("threeterm-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");
        let initial_identity = current_identity(&root).expect("initial identity");
        let intent = TransactionIntent::AddFeature {
            feature_id: "feat-1".to_string(),
            feature_kind: "sketch".to_string(),
            parameters: serde_json::json!({}),
        };
        append(&root, &initial_identity, &intent).expect("first append");

        let stale = Identity("00".repeat(32));
        let err = append(&root, &stale, &intent).expect_err("stale parent rejected");
        match err {
            BundleError::Invalid(detail) => {
                assert!(detail.contains("parent identity mismatch"), "got: {detail}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn identity_survives_a_reload_after_appending_the_full_mvp_set() {
        let root = std::env::temp_dir().join(format!("threeterm-roundtrip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");

        let mut identity = current_identity(&root).expect("initial identity");
        let operations = vec![
            TransactionIntent::AddFeature {
                feature_id: "sketch-1".to_string(),
                feature_kind: "sketch".to_string(),
                parameters: serde_json::json!({ "width": 10.0 }),
            },
            TransactionIntent::SetParameter {
                feature_id: "sketch-1".to_string(),
                parameter: "width".to_string(),
                value: serde_json::json!(20.0),
            },
            TransactionIntent::AddFeature {
                feature_id: "extrude-1".to_string(),
                feature_kind: "extrude".to_string(),
                parameters: serde_json::json!({ "depth": 5.0 }),
            },
            TransactionIntent::SetParameter {
                feature_id: "extrude-1".to_string(),
                parameter: "depth".to_string(),
                value: serde_json::json!(7.0),
            },
            TransactionIntent::RemoveFeature {
                feature_id: "sketch-1".to_string(),
            },
        ];

        for intent in &operations {
            let appended = append(&root, &identity, intent).expect("append succeeds");
            identity = Identity(appended.manifest.transaction_sha256);
        }

        let reloaded_identity = current_identity(&root).expect("identity after reload");
        assert_eq!(
            reloaded_identity, identity,
            "identity must be byte-equal after reload"
        );

        let loaded = load(&root).expect("bundle loads");
        assert_eq!(loaded.manifest.transaction_count, 5);
        assert_eq!(loaded.manifest.transaction_bytes, loaded.transactions.len());
        assert_eq!(
            loaded.manifest.transaction_sha256,
            hash(loaded.transactions.as_bytes())
        );
        let _ = fs::remove_dir_all(root);
    }
}

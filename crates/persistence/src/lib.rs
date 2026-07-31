use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threeterm_domain::{CommandIntent, ProjectGeneration};

pub mod bundle {
    pub use super::{
        BundleError, LoadedBundle, Manifest, append_transaction, compute_log_identity, load,
        log_identity_hex, schema_version, write_fresh,
    };
}

pub fn schema_version() -> &'static str {
    "threeterm.persistence/1"
}

const MANIFEST_FILE: &str = "manifest.json";
const TRANSACTIONS_FILE: &str = "canonical/transactions.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema_version: String,
    pub generation_id: String,
    pub revision_id: String,
    pub revision_count: usize,
    pub transaction_count: usize,
    pub transaction_bytes: usize,
    pub log_identity: String,
    pub canonical_root_sha256: String,
    pub seal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedBundle {
    pub manifest: Manifest,
    pub generation: ProjectGeneration,
    pub transactions: String,
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
    let log_identity = log_identity_hex(transactions.as_bytes());
    let mut manifest = Manifest {
        schema_version: schema_version().to_string(),
        // The durable identity is the canonical log digest; the
        // caller-supplied ProjectGeneration::id is intentionally ignored
        // so the identity always reflects the log content.
        generation_id: log_identity.clone(),
        revision_id: revision.id.clone(),
        revision_count: 1,
        transaction_count: 0,
        transaction_bytes,
        log_identity,
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
    if manifest.transaction_bytes != transactions.len()
        || manifest.log_identity != log_identity_hex(transactions.as_bytes())
    {
        return Err(BundleError::Invalid(
            "canonical log identity mismatch".into(),
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

/// Append an accepted command transaction to an existing bundle.
///
/// The current bundle is loaded first; if `load` fails (no bundle, tampered
/// manifest, ...) the call returns the error without touching any files.
/// After a successful append, both `canonical/transactions.ndjson` and
/// `manifest.json` are updated via per-file atomic rename so each file is
/// either the prior state or the new state — never partial.
///
/// On success the new sealed `Manifest` is returned. The `log_identity`
/// reflects the canonical log content including the appended transaction;
/// `generation_id`, `schema_version`, and `revision_id` are preserved.
pub fn append_transaction(path: &Path, intent: &CommandIntent) -> Result<Manifest, BundleError> {
    let bundle = load(path)?;
    let transaction = threeterm_domain::CommandTransaction::new(intent.clone());
    let line = transaction.canonical_line();

    let mut new_transactions = bundle.transactions;
    new_transactions.push_str(&line);
    let new_log_identity = log_identity_hex(new_transactions.as_bytes());

    let mut new_manifest = bundle.manifest;
    new_manifest.transaction_count += 1;
    new_manifest.transaction_bytes = new_transactions.len();
    new_manifest.log_identity = new_log_identity.clone();
    // The durable identity is the canonical log digest; keep
    // generation_id in lock-step so every accepted transaction produces
    // a single byte-equal identity across the manifest fields.
    new_manifest.generation_id = new_log_identity;
    new_manifest.canonical_root_sha256 = hash(&canonical_manifest_bytes(&new_manifest));
    new_manifest.seal_sha256 = hash(&sealed_manifest_bytes(&new_manifest));

    let manifest_tmp = path.join(format!("{MANIFEST_FILE}.tmp"));
    fs::write(&manifest_tmp, serde_json::to_vec_pretty(&new_manifest)?)?;
    fs::rename(&manifest_tmp, path.join(MANIFEST_FILE))?;

    let tx_tmp = path.join(format!("{TRANSACTIONS_FILE}.tmp"));
    fs::write(&tx_tmp, new_transactions.as_bytes())?;
    fs::rename(&tx_tmp, path.join(TRANSACTIONS_FILE))?;

    Ok(new_manifest)
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

/// SHA-256 of the canonical NDJSON log content.
///
/// The Project Generation identity is the canonical log digest: same bytes in
/// → same 32 bytes out. The encoding is the raw byte sequence of the
/// canonical transaction log file (`canonical/transactions.ndjson`), with
/// each accepted transaction serialized as one UTF-8 line terminated by a
/// newline. Object keys are sorted by `serde_json::Value::Object`'s default
/// `BTreeMap` backing, so two equivalent intents serialize to the same
/// bytes.
pub fn compute_log_identity(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Lowercase hex SHA-256 of the canonical NDJSON log content. This is the
/// shape the manifest records and the CLI surfaces through the `identity`
/// command.
pub fn log_identity_hex(bytes: &[u8]) -> String {
    let digest = compute_log_identity(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use threeterm_domain::Revision;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.persistence/1");
    }

    #[test]
    fn compute_log_identity_is_a_32_byte_sha256_digest() {
        let digest = compute_log_identity(b"hello\n");
        assert_eq!(digest.len(), 32, "SHA-256 produces 32 raw bytes");
    }

    #[test]
    fn compute_log_identity_is_deterministic_for_same_bytes() {
        let first = compute_log_identity(b"transaction\n");
        let second = compute_log_identity(b"transaction\n");
        assert_eq!(first, second);
    }

    #[test]
    fn compute_log_identity_differs_for_different_bytes() {
        let a = compute_log_identity(b"alpha\n");
        let b = compute_log_identity(b"beta\n");
        assert_ne!(a, b);
    }

    #[test]
    fn compute_log_identity_hex_matches_a_64_char_lowercase_hex_string() {
        let hex = log_identity_hex(b"hello\n");
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hex must be lowercase; got {hex:?}"
        );
    }

    #[test]
    fn compute_log_identity_hex_matches_underlying_bytes() {
        let bytes = b"hello\n";
        let raw = compute_log_identity(bytes);
        let hex = log_identity_hex(bytes);
        let mut rebuilt = String::with_capacity(64);
        for byte in raw {
            use std::fmt::Write as _;
            let _ = write!(&mut rebuilt, "{byte:02x}");
        }
        assert_eq!(hex, rebuilt);
    }

    #[test]
    fn write_fresh_records_empty_log_identity_in_manifest() {
        let root =
            std::env::temp_dir().join(format!("threeterm-log-identity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let manifest = write_fresh(&root, ProjectGeneration::with_id("generation-test"))
            .expect("bundle writes");
        let expected = log_identity_hex(b"");
        assert_eq!(manifest.log_identity, expected);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_fresh_uses_log_identity_as_generation_id() {
        let root = std::env::temp_dir().join(format!("threeterm-gen-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let manifest = write_fresh(&root, ProjectGeneration::with_id("arbitrary-handle"))
            .expect("bundle writes");
        // The generation_id field is the canonical log digest, not the
        // caller-supplied handle. The caller-supplied handle is intentionally
        // ignored so the durable identity always reflects the log content.
        assert_eq!(
            manifest.generation_id,
            log_identity_hex(b""),
            "manifest.generation_id must equal the canonical log digest"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn domain_empty_log_identity_constant_matches_persistence_computation() {
        // The domain crate pins a constant for the empty-log identity so
        // ProjectGeneration::fresh() can produce a stable ID without a
        // hashing dependency. This test guarantees the constant does not
        // drift from the persistence layer's authoritative computation.
        let expected = log_identity_hex(b"");
        assert_eq!(
            threeterm_domain::EMPTY_LOG_IDENTITY,
            expected,
            "domain::EMPTY_LOG_IDENTITY must match persistence::log_identity_hex(b\"\")"
        );
    }

    #[test]
    fn load_rejects_manifest_with_tampered_log_identity() {
        let root = std::env::temp_dir().join(format!("threeterm-tamper-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");

        let manifest_path = root.join(MANIFEST_FILE);
        let raw = fs::read_to_string(&manifest_path).expect("manifest readable");
        let mut value: serde_json::Value = serde_json::from_str(&raw).expect("manifest is json");
        value["log_identity"] = serde_json::Value::String("0".repeat(64));
        fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap())
            .expect("manifest rewritten");

        let error = load(&root).expect_err("tampered log_identity must be rejected");
        match error {
            BundleError::Invalid(detail) => {
                assert!(
                    detail.contains("log_identity") || detail.contains("canonical"),
                    "diagnostic must name the failing field; got {detail:?}"
                );
            }
            other => panic!("expected BundleError::Invalid, got {other:?}"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn append_transaction_appends_line_and_recomputes_identity() {
        use threeterm_domain::graph::CommandIntent;
        let root = std::env::temp_dir().join(format!("threeterm-append-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");

        let initial = load(&root).expect("load").manifest.log_identity.clone();

        let transaction = CommandIntent::AddFeature {
            feature_id: threeterm_domain::graph::FeatureId::new("sketch-1").unwrap(),
            feature_kind: "sketch".to_string(),
            parameters: serde_json::json!({"plane": "xy"}),
        };
        let new_manifest = append_transaction(&root, &transaction).expect("append succeeds");

        assert_ne!(
            new_manifest.log_identity, initial,
            "log_identity must change when a transaction is appended"
        );
        assert_eq!(new_manifest.transaction_count, 1);

        let reloaded = load(&root).expect("reload after append");
        assert_eq!(
            reloaded.manifest.log_identity, new_manifest.log_identity,
            "log_identity must be byte-equal across reload"
        );
        assert_eq!(reloaded.transactions.lines().count(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn append_transaction_preserves_bundle_when_load_fails() {
        use threeterm_domain::graph::CommandIntent;
        let root = std::env::temp_dir().join(format!("threeterm-preserve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        // No bundle written — load must fail and append must surface that failure
        // without touching any files.
        let transaction = CommandIntent::AddFeature {
            feature_id: threeterm_domain::graph::FeatureId::new("sketch-1").unwrap(),
            feature_kind: "sketch".to_string(),
            parameters: serde_json::json!({}),
        };
        let result = append_transaction(&root, &transaction);
        assert!(result.is_err(), "append to a missing bundle must fail");
        assert!(
            !root.exists(),
            "failed append must not create the bundle directory"
        );

        // Now create a valid bundle and verify the prior failure left the
        // filesystem untouched (no stray partial files).
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");
        let reloaded = load(&root).expect("reload");
        assert_eq!(reloaded.manifest.transaction_count, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn append_transaction_recomputes_manifest_seal() {
        use threeterm_domain::graph::CommandIntent;
        let root = std::env::temp_dir().join(format!("threeterm-seal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-test")).expect("bundle writes");

        let initial = load(&root).expect("load").manifest;
        let transaction = CommandIntent::RemoveFeature {
            feature_id: threeterm_domain::graph::FeatureId::new("sketch-1").unwrap(),
        };
        let new_manifest = append_transaction(&root, &transaction).expect("append");

        assert_ne!(
            new_manifest.seal_sha256, initial.seal_sha256,
            "manifest seal must change after the log changes"
        );
        assert_ne!(
            new_manifest.canonical_root_sha256, initial.canonical_root_sha256,
            "canonical root hash must change after the log changes"
        );

        // Reload and confirm the seal still validates
        let reloaded = load(&root).expect("reload");
        assert_eq!(reloaded.manifest.seal_sha256, new_manifest.seal_sha256);
        assert_eq!(
            reloaded.manifest.canonical_root_sha256,
            new_manifest.canonical_root_sha256
        );
        let _ = fs::remove_dir_all(root);
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
        assert_eq!(loaded.generation.revisions, vec![Revision::empty()]);
        assert!(loaded.transactions.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}

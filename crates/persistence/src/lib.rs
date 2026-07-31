use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threeterm_domain::ProjectGeneration;

pub mod bundle {
    pub use super::{BundleError, LoadedBundle, Manifest, load, schema_version, write_fresh};
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
        || manifest.transaction_sha256 != hash(transactions.as_bytes())
        || manifest.transaction_count != 0
        || !transactions.is_empty()
    {
        return Err(BundleError::Invalid(
            "canonical transaction log integrity mismatch".into(),
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
    use threeterm_domain::Revision;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.persistence/1");
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

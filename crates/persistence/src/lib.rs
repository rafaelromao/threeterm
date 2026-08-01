use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threeterm_domain::{CommandTransaction, ProjectGeneration, Revision};

/// Classification of a `.threeterm/` bundle's manifest schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStatus {
    Current,
    Prior,
    Unknown,
}

/// Sealed prior-epoch (epoch 0) project manifest.
///
/// This is the migration source: it carries generation/revision identity and
/// a single transaction-log hash but no manifest seal. The `deny_unknown_fields`
/// attribute closes the manifest-level expression of the migration policy's
/// "fail closed on unknown fields" rule (closed issue #45). When future
/// slices add feature-payload, command, or worker-fingerprint fields to the
/// manifest they must follow the same `deny_unknown_fields` plus
/// `ManifestFieldUnknown` pattern.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct V0Manifest {
    pub schema_version: String,
    pub generation_id: String,
    pub revision_id: String,
    pub revision_count: usize,
    pub transaction_count: usize,
    pub transaction_bytes: usize,
    pub transaction_sha256: String,
}

/// In-memory authenticated v0 bundle: a parsed manifest plus its canonical
/// transaction-log contents and the domain generation the manifest identifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V0Bundle {
    pub manifest: V0Manifest,
    pub transactions: String,
    pub generation: ProjectGeneration,
}

pub mod bundle {
    pub use super::{
        BundleError, LoadedBundle, Manifest, PRE_MIGRATION_BACKUP_SUFFIX, SchemaStatus, V0Bundle,
        V0Manifest, append_transaction, detect_schema, load, migrate_v0_to_v1, prior_schema_epoch,
        read_v0, schema_epoch, write_fresh, write_v0_fixture,
    };
}

pub const PRE_MIGRATION_BACKUP_SUFFIX: &str = ".pre-migration-backup";

pub fn schema_epoch() -> &'static str {
    "threeterm.persistence/1"
}

pub fn prior_schema_epoch() -> &'static str {
    "threeterm.persistence/0"
}

const MANIFEST_FILE: &str = "manifest.json";
const TRANSACTIONS_FILE: &str = "canonical/transactions.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
    SchemaUnknown {
        found: String,
        expected_current: &'static str,
        expected_prior: &'static str,
    },
    SchemaTooOld {
        found: String,
        expected: &'static str,
    },
    SchemaTooNew {
        found: String,
        expected: &'static str,
    },
    ManifestFieldUnknown {
        kind: &'static str,
        field: String,
    },
    Backup {
        path: PathBuf,
        source: std::io::Error,
    },
    SourceUnreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    Migration {
        source: Box<BundleError>,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "filesystem error: {error}"),
            Self::Json(error) => write!(formatter, "invalid JSON: {error}"),
            Self::Invalid(detail) => formatter.write_str(detail),
            Self::SchemaUnknown {
                found,
                expected_current,
                expected_prior,
            } => write!(
                formatter,
                "persistence.schema-unsupported: unknown schema {found:?}; expected current {expected_current:?} or prior {expected_prior:?}"
            ),
            Self::SchemaTooOld { found, expected } => write!(
                formatter,
                "persistence.schema-too-old: found {found:?}, expected {expected:?}; open this project in a compatible release first"
            ),
            Self::SchemaTooNew { found, expected } => write!(
                formatter,
                "persistence.schema-too-new: found {found:?}, expected {expected:?}; upgrade ThreeTerm to open this project"
            ),
            Self::ManifestFieldUnknown { kind, field } => write!(
                formatter,
                "persistence.manifest-field-unknown: kind={kind} field={field:?}"
            ),
            Self::Backup { path, source } => write!(
                formatter,
                "persistence.backup-failed: path={} source={source}",
                path.display()
            ),
            Self::SourceUnreadable { path, source } => write!(
                formatter,
                "persistence.source-unreadable: path={} source={source}",
                path.display()
            ),
            Self::Migration { source } => {
                write!(formatter, "persistence.migration-failed: {source}")
            }
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

    generation
        .revisions
        .first()
        .filter(|revision| generation.revisions.len() == 1 && revision.features.is_empty())
        .ok_or_else(|| {
            BundleError::Invalid("fresh generation must contain one empty revision".into())
        })?;
    let transactions = String::new();
    let manifest = manifest_for(&generation, &transactions);
    let staging = staging_path(path);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    write_v1_into(&staging, &manifest, transactions.as_bytes())?;
    fs::rename(&staging, path)?;
    Ok(manifest)
}

pub fn append_transaction(
    path: &Path,
    transaction: &CommandTransaction,
) -> Result<LoadedBundle, BundleError> {
    let loaded = load(path)?;
    let mut generation = loaded.generation.clone();
    generation
        .replay(transaction)
        .map_err(|error| BundleError::Invalid(error.to_string()))?;
    let mut transactions = loaded.transactions;
    transactions.push_str(&transaction.canonical_line());
    let manifest = manifest_for(&generation, &transactions);
    let staging = staging_path(path);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    write_v1_into(&staging, &manifest, transactions.as_bytes())?;
    let validated = match load_v1(&staging) {
        Ok(loaded) => loaded,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let previous = previous_path(path);
    if previous.exists() {
        fs::remove_dir_all(&previous)?;
    }
    fs::rename(path, &previous)?;
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::rename(&previous, path);
        return Err(BundleError::Io(error));
    }
    Ok(validated)
}

fn manifest_for(generation: &ProjectGeneration, transactions: &str) -> Manifest {
    let revision = generation.current_revision();
    let transaction_bytes = transactions.len();
    let transaction_sha256 = hash(transactions.as_bytes());
    let mut manifest = Manifest {
        schema_version: schema_epoch().to_string(),
        generation_id: generation.id.clone(),
        revision_id: revision.id.clone(),
        revision_count: generation.revisions.len(),
        transaction_count: count_lines(transactions),
        transaction_bytes,
        transaction_sha256,
        canonical_root_sha256: String::new(),
        seal_sha256: String::new(),
    };
    manifest.canonical_root_sha256 = hash(&canonical_manifest_bytes(&manifest));
    manifest.seal_sha256 = hash(&sealed_manifest_bytes(&manifest));
    manifest
}

fn count_lines(transactions: &str) -> usize {
    if transactions.is_empty() {
        0
    } else {
        transactions.lines().count()
    }
}

pub fn load(path: &Path) -> Result<LoadedBundle, BundleError> {
    let status = detect_schema(path)?;
    match status {
        SchemaStatus::Current => load_v1(path),
        SchemaStatus::Prior => load_v0_with_migration(path),
        SchemaStatus::Unknown => Err(BundleError::SchemaUnknown {
            found: read_schema_version_raw(path).unwrap_or_default(),
            expected_current: schema_epoch(),
            expected_prior: prior_schema_epoch(),
        }),
    }
}

fn load_v1(path: &Path) -> Result<LoadedBundle, BundleError> {
    let raw =
        fs::read(path.join(MANIFEST_FILE)).map_err(|error| BundleError::SourceUnreadable {
            path: path.join(MANIFEST_FILE),
            source: error,
        })?;
    let manifest: Manifest = match serde_json::from_slice(&raw) {
        Ok(manifest) => manifest,
        Err(error) => {
            if let Some(field) = unknown_field_in(&error.to_string()) {
                return Err(BundleError::ManifestFieldUnknown { kind: "v1", field });
            }
            return Err(BundleError::Json(error));
        }
    };
    if manifest.schema_version != schema_epoch() {
        return Err(BundleError::Invalid(format!(
            "unsupported persistence schema {:?}",
            manifest.schema_version
        )));
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
        || manifest.transaction_count != transactions.lines().count()
    {
        return Err(BundleError::Invalid(
            "canonical transaction log integrity mismatch".into(),
        ));
    }
    let mut generation = ProjectGeneration::with_id(manifest.generation_id.clone());
    for line in transactions.lines() {
        let transaction: CommandTransaction = serde_json::from_str(line)?;
        generation
            .replay(&transaction)
            .map_err(|error| BundleError::Invalid(error.to_string()))?;
    }
    if generation.current_revision().id != manifest.revision_id
        || generation.revisions.len() != manifest.revision_count
    {
        return Err(BundleError::Invalid("revision identity mismatch".into()));
    }
    Ok(LoadedBundle {
        manifest,
        generation,
        transactions,
    })
}

fn read_schema_version_raw(path: &Path) -> Option<String> {
    let raw = fs::read(path.join(MANIFEST_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value.get("schema_version")?.as_str().map(str::to_string)
}

/// Orchestrated prior-epoch load: sealed backup → deterministic migration →
/// validated staging → atomic publish. On any failure the source directory
/// stays byte-for-byte unchanged and no partial target is published.
fn load_v0_with_migration(path: &Path) -> Result<LoadedBundle, BundleError> {
    let v0 = read_v0(path).map_err(|error| BundleError::Migration {
        source: Box::new(error),
    })?;
    let (manifest, _generation) = migrate_v0_to_v1(&v0);
    let transactions = v0.transactions.clone();

    let backup_path = backup_path_for(path);
    if let Err(source) = publish_sealed_backup(path, &backup_path) {
        return Err(BundleError::Backup {
            path: backup_path,
            source,
        });
    }

    let staging = staging_path_for_migration(path);
    if let Err(error) = write_v1_into(&staging, &manifest, transactions.as_bytes()) {
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&backup_path);
        return Err(BundleError::Migration {
            source: Box::new(error),
        });
    }

    let validate = load_v1(&staging);
    let validated = match validate {
        Ok(loaded) => loaded,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            let _ = fs::remove_dir_all(&backup_path);
            return Err(BundleError::Migration {
                source: Box::new(error),
            });
        }
    };

    if let Err(error) = publish_staged(&staging, path) {
        let _ = fs::remove_dir_all(&staging);
        return Err(BundleError::Io(error));
    }

    Ok(validated)
}

fn publish_staged(staging: &Path, destination: &Path) -> std::io::Result<()> {
    // The destination is the prior-epoch source directory; the
    // pre-migration backup already holds a byte-faithful copy of it, so we
    // can drop the source contents and rename the validated v1 staging
    // directory on top. If we are interrupted between the remove and the
    // rename, the backup is the durable recovery path (migration policy:
    // "On any failure the source and backup remain untouched").
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
    fs::rename(staging, destination)
}

fn backup_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(PRE_MIGRATION_BACKUP_SUFFIX);
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

fn staging_path_for_migration(path: &Path) -> PathBuf {
    let mut staging = path.to_path_buf();
    let suffix = format!(".migrate-tmp-{}", std::process::id());
    staging.set_file_name(format!(
        "{}{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        suffix
    ));
    staging
}

fn publish_sealed_backup(source: &Path, backup: &Path) -> std::io::Result<()> {
    if backup.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("backup path already exists: {}", backup.display()),
        ));
    }
    copy_dir_recursive(source, backup)
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to copy symlink: {}", entry.path().display()),
            ));
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn write_v1_into(
    staging: &Path,
    manifest: &Manifest,
    transactions: &[u8],
) -> Result<(), BundleError> {
    fs::create_dir_all(staging.join("canonical"))?;
    fs::write(staging.join(TRANSACTIONS_FILE), transactions)?;
    fs::write(
        staging.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(manifest)?,
    )?;
    Ok(())
}

/// Classify a bundle directory's manifest schema without mutating it.
///
/// Reads `manifest.json`, parses the `schema_version` field, and returns
/// `Current` for the v1 epoch, `Prior` for the v0 epoch, or `Unknown` for any
/// other string. A path whose name ends with `PRE_MIGRATION_BACKUP_SUFFIX` —
/// i.e. a pre-migration backup sibling — always returns `Unknown`, so a
/// future v2 reader that opens the backup expecting a canonical layout
/// fails closed. Unknown manifest fields produce `ManifestFieldUnknown`.
pub fn detect_schema(path: &Path) -> Result<SchemaStatus, BundleError> {
    if is_pre_migration_backup_path(path) {
        return Ok(SchemaStatus::Unknown);
    }
    let raw =
        fs::read(path.join(MANIFEST_FILE)).map_err(|error| BundleError::SourceUnreadable {
            path: path.join(MANIFEST_FILE),
            source: error,
        })?;
    let value: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(error) => {
            if let Some(field) = unknown_field_in(&error.to_string()) {
                return Err(BundleError::ManifestFieldUnknown {
                    kind: "manifest",
                    field,
                });
            }
            return Err(BundleError::Json(error));
        }
    };
    let found = match value.get("schema_version").and_then(|v| v.as_str()) {
        Some(found) => found.to_string(),
        None => {
            return Err(BundleError::Invalid(
                "manifest is missing schema_version field".into(),
            ));
        }
    };
    let kind: &'static str = if found == prior_schema_epoch() {
        "v0"
    } else if found == schema_epoch() {
        "v1"
    } else {
        return Ok(SchemaStatus::Unknown);
    };
    let parse_result: Result<(), String> = if kind == "v0" {
        serde_json::from_value::<V0Manifest>(value)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        serde_json::from_value::<Manifest>(value)
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    if let Err(message) = parse_result {
        if let Some(field) = unknown_field_in(&message) {
            return Err(BundleError::ManifestFieldUnknown { kind, field });
        }
        return Err(BundleError::Invalid(message));
    }
    Ok(if kind == "v0" {
        SchemaStatus::Prior
    } else {
        SchemaStatus::Current
    })
}

fn unknown_field_in(message: &str) -> Option<String> {
    for needle in ["unknown field `", "unknown field '"] {
        if let Some(after) = message.find(needle) {
            let rest = &message[after + needle.len()..];
            let closer = needle.chars().last().unwrap_or('`');
            if let Some(end) = rest.find(closer) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn is_pre_migration_backup_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(PRE_MIGRATION_BACKUP_SUFFIX))
}

/// Authenticate and read a v0 bundle on disk. Mirrors the v1 reader's
/// invariants: manifest parses with `deny_unknown_fields`, transaction log
/// bytes match the recorded hash, and the manifest's generation/revision
/// identity lines up with the canonical log.
pub fn read_v0(path: &Path) -> Result<V0Bundle, BundleError> {
    let raw =
        fs::read(path.join(MANIFEST_FILE)).map_err(|error| BundleError::SourceUnreadable {
            path: path.join(MANIFEST_FILE),
            source: error,
        })?;
    let manifest: V0Manifest = match serde_json::from_slice(&raw) {
        Ok(manifest) => manifest,
        Err(error) => {
            if let Some(field) = unknown_field_in(&error.to_string()) {
                return Err(BundleError::ManifestFieldUnknown { kind: "v0", field });
            }
            return Err(BundleError::Json(error));
        }
    };
    if manifest.schema_version != prior_schema_epoch() {
        return Err(BundleError::Invalid(format!(
            "v0 reader expected schema {:?}, found {:?}",
            prior_schema_epoch(),
            manifest.schema_version
        )));
    }
    if manifest.revision_count != 1 {
        return Err(BundleError::Invalid(format!(
            "v0 reader requires exactly one revision per bundle, found {}",
            manifest.revision_count
        )));
    }
    let transactions = String::from_utf8(fs::read(path.join(TRANSACTIONS_FILE))?)
        .map_err(|_| BundleError::Invalid("v0 transactions are not UTF-8".into()))?;
    if manifest.transaction_bytes != transactions.len()
        || manifest.transaction_sha256 != hash(transactions.as_bytes())
    {
        return Err(BundleError::Invalid(
            "v0 canonical transaction log integrity mismatch".into(),
        ));
    }
    let generation = ProjectGeneration::with_id(manifest.generation_id.clone());
    if generation.revisions[0].id != manifest.revision_id {
        return Err(BundleError::Invalid("v0 revision identity mismatch".into()));
    }
    Ok(V0Bundle {
        manifest,
        transactions,
        generation,
    })
}

/// Write a v0 fixture to `path`. Used by tests to construct a prior-epoch
/// bundle that the v1 reader must migrate. Refuses to overwrite an existing
/// directory.
pub fn write_v0_fixture(
    path: &Path,
    generation: ProjectGeneration,
) -> Result<V0Manifest, BundleError> {
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
            BundleError::Invalid("v0 fixture generation must contain one empty revision".into())
        })?;
    let transactions = String::new();
    let manifest = V0Manifest {
        schema_version: prior_schema_epoch().to_string(),
        generation_id: generation.id.clone(),
        revision_id: revision.id.clone(),
        revision_count: 1,
        transaction_count: 0,
        transaction_bytes: transactions.len(),
        transaction_sha256: hash(transactions.as_bytes()),
    };
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

/// Pure deterministic host-only v0 → v1 transformation.
///
/// Reads no clock, RNG, environment, worker, kernel, locale, or network; only
/// the supplied authenticated v0 inputs. The "feature-graph hash" preserved
/// by the migration is the v1 `Manifest::canonical_root_sha256`, since the
/// feature graph in this slice is empty (one feature) and the v1 manifest's
/// canonical-root hash is the only stable project-graph identifier in the
/// current epoch.
pub fn migrate_v0_to_v1(source: &V0Bundle) -> (Manifest, ProjectGeneration) {
    debug_assert!(source.generation.revisions.len() == 1);
    debug_assert!(source.transactions.is_empty());
    let generation = ProjectGeneration {
        id: source.manifest.generation_id.clone(),
        revisions: vec![Revision {
            id: source.manifest.revision_id.clone(),
            features: source.generation.revisions[0].features.clone(),
            component_graph: source.generation.revisions[0].component_graph.clone(),
        }],
    };
    let mut manifest = Manifest {
        schema_version: schema_epoch().to_string(),
        generation_id: source.manifest.generation_id.clone(),
        revision_id: source.manifest.revision_id.clone(),
        revision_count: source.manifest.revision_count,
        transaction_count: source.manifest.transaction_count,
        transaction_bytes: source.manifest.transaction_bytes,
        transaction_sha256: source.manifest.transaction_sha256.clone(),
        canonical_root_sha256: String::new(),
        seal_sha256: String::new(),
    };
    manifest.canonical_root_sha256 = hash(&canonical_manifest_bytes(&manifest));
    manifest.seal_sha256 = hash(&sealed_manifest_bytes(&manifest));
    (manifest, generation)
}

fn previous_path(path: &Path) -> PathBuf {
    let mut previous = path.to_path_buf();
    previous.set_file_name(format!(
        "{}.previous",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    previous
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
        assert_eq!(schema_epoch(), "threeterm.persistence/1");
        assert_eq!(prior_schema_epoch(), "threeterm.persistence/0");
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

    #[test]
    fn detect_schema_classifies_v0_and_v1() {
        let root_v0 =
            std::env::temp_dir().join(format!("threeterm-detect-v0-{}", std::process::id()));
        let root_v1 =
            std::env::temp_dir().join(format!("threeterm-detect-v1-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root_v0);
        let _ = fs::remove_dir_all(&root_v1);

        write_v0_fixture(&root_v0, ProjectGeneration::with_id("generation-v0")).expect("v0 writes");
        write_fresh(&root_v1, ProjectGeneration::with_id("generation-v1")).expect("v1 writes");

        assert_eq!(
            detect_schema(&root_v0).expect("v0 detected"),
            SchemaStatus::Prior
        );
        assert_eq!(
            detect_schema(&root_v1).expect("v1 detected"),
            SchemaStatus::Current
        );

        let _ = fs::remove_dir_all(root_v0);
        let _ = fs::remove_dir_all(root_v1);
    }

    #[test]
    fn migrate_v0_to_v1_is_deterministic() {
        let root = std::env::temp_dir().join(format!("threeterm-migrate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_v0_fixture(&root, ProjectGeneration::with_id("generation-mig")).expect("v0 writes");
        let v0 = read_v0(&root).expect("v0 reads");
        let (first_manifest, first_generation) = migrate_v0_to_v1(&v0);
        let (second_manifest, second_generation) = migrate_v0_to_v1(&v0);

        assert_eq!(
            first_manifest.canonical_root_sha256,
            second_manifest.canonical_root_sha256
        );
        assert_eq!(first_manifest.seal_sha256, second_manifest.seal_sha256);
        assert_eq!(first_manifest, second_manifest);
        assert_eq!(first_generation, second_generation);
        assert_eq!(first_generation.id, v0.manifest.generation_id);
        assert_eq!(first_generation.revisions[0].id, v0.manifest.revision_id);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn v0_manifest_rejects_unknown_fields() {
        let root =
            std::env::temp_dir().join(format!("threeterm-unknown-field-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bad = r#"{
            "schema_version": "threeterm.persistence/0",
            "generation_id": "g",
            "revision_id": "r",
            "revision_count": 1,
            "transaction_count": 0,
            "transaction_bytes": 0,
            "transaction_sha256": "00",
            "future_field": true
        }"#;
        fs::create_dir_all(&root).expect("dir");
        fs::write(root.join(MANIFEST_FILE), bad).expect("manifest");
        fs::create_dir_all(root.join("canonical")).expect("canonical dir");
        fs::write(root.join(TRANSACTIONS_FILE), b"").expect("log");
        let err = read_v0(&root).expect_err("v0 unknown field rejected");
        match err {
            BundleError::ManifestFieldUnknown { kind, field } => {
                assert_eq!(kind, "v0");
                assert_eq!(field, "future_field");
            }
            other => panic!("expected ManifestFieldUnknown, got {other:?}"),
        }
        let _ = fs::remove_dir_all(root);
    }
}

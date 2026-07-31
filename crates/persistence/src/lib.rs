use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threeterm_domain::{Feature, FeatureGraph, ProjectGeneration, Revision};

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
        Bundle, BundleError, EMPTY_LOG_DIGEST_HEX, LoadedBundle, LogEntry, MANIFEST_FILENAME,
        MANIFEST_SCHEMA_GENERATION, Manifest, PRE_MIGRATION_BACKUP_SUFFIX, SchemaStatus,
        TRANSACTIONS_LOG_FILENAME, TransactionLog, V0Bundle, V0Manifest, detect_schema, load,
        migrate_v0_to_v1, prior_schema_epoch, read_v0, schema_epoch, write_fresh, write_v0_fixture,
    };
}

pub const PRE_MIGRATION_BACKUP_SUFFIX: &str = ".pre-migration-backup";

pub fn schema_epoch() -> &'static str {
    "threeterm.persistence/1"
}

pub fn prior_schema_epoch() -> &'static str {
    "threeterm.persistence/0"
}

pub const MANIFEST_FILENAME: &str = "manifest.json";
pub const TRANSACTIONS_LOG_FILENAME: &str = "transactions.log";
pub const MANIFEST_SCHEMA_GENERATION: u32 = 1;
pub const EMPTY_LOG_DIGEST_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: String,
    pub schema_generation: u32,
    pub generation_id: String,
    pub revision_id: String,
    pub revision_count: usize,
    pub transaction_count: usize,
    pub transaction_bytes: usize,
    pub transaction_sha256: String,
    pub terminal_log_digest: String,
    pub feature_graph_hash: String,
    pub revision_hash: String,
    pub canonical_root_sha256: String,
    pub seal_sha256: String,
}

impl Manifest {
    fn seal(
        generation_id: &str,
        revision_id: &str,
        log: &TransactionLog,
        graph: &FeatureGraph,
    ) -> Self {
        let transactions = log.encode();
        let terminal_log_digest = log.terminal_digest_hex().to_string();
        let feature_graph_hash = graph.graph_hash_hex();
        let revision_hash = graph.revision_hash_hex(&terminal_log_digest);
        let mut manifest = Self {
            schema_version: schema_epoch().to_string(),
            schema_generation: MANIFEST_SCHEMA_GENERATION,
            generation_id: generation_id.to_string(),
            revision_id: revision_id.to_string(),
            revision_count: 1,
            transaction_count: log.len(),
            transaction_bytes: transactions.len(),
            transaction_sha256: hash(&transactions),
            terminal_log_digest,
            feature_graph_hash,
            revision_hash,
            canonical_root_sha256: String::new(),
            seal_sha256: String::new(),
        };
        manifest.canonical_root_sha256 = hash(&canonical_manifest_bytes(&manifest));
        manifest.seal_sha256 = hash(&sealed_manifest_bytes(&manifest));
        manifest
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogEntry {
    pub log_index: usize,
    pub previous_digest: String,
    pub feature_id: String,
    pub kind: String,
    pub terminal_digest: String,
}

impl LogEntry {
    fn new(log_index: usize, previous_digest: &str, feature_id: &str, kind: &str) -> Self {
        let mut entry = Self {
            log_index,
            previous_digest: previous_digest.to_string(),
            feature_id: feature_id.to_string(),
            kind: kind.to_string(),
            terminal_digest: String::new(),
        };
        entry.terminal_digest = entry.recomputed_digest();
        entry
    }

    fn recomputed_digest(&self) -> String {
        let mut copy = self.clone();
        copy.terminal_digest.clear();
        hash(&serde_json::to_vec(&copy).expect("log entry serializes"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransactionLog {
    entries: Vec<LogEntry>,
}

impl TransactionLog {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    fn append_feature(&mut self, feature_id: &str, kind: &str) {
        let previous = self.terminal_digest_hex().to_string();
        self.entries.push(LogEntry::new(
            self.entries.len(),
            &previous,
            feature_id,
            kind,
        ));
    }

    pub fn terminal_digest_hex(&self) -> &str {
        self.entries
            .last()
            .map_or(EMPTY_LOG_DIGEST_HEX, |entry| entry.terminal_digest.as_str())
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in &self.entries {
            bytes.extend_from_slice(&serde_json::to_vec(entry).expect("log entry serializes"));
            bytes.push(b'\n');
        }
        bytes
    }

    fn decode_and_verify(bytes: &[u8]) -> Result<Self, BundleError> {
        let mut entries = Vec::new();
        for (line_index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            let entry: LogEntry =
                serde_json::from_slice(line).map_err(|error| BundleError::LogBrokenLink {
                    log_index: line_index,
                    detail: error.to_string(),
                })?;
            let expected_previous = entries
                .last()
                .map_or(EMPTY_LOG_DIGEST_HEX, |previous: &LogEntry| {
                    previous.terminal_digest.as_str()
                });
            if entry.log_index != entries.len()
                || entry.previous_digest != expected_previous
                || entry.terminal_digest != entry.recomputed_digest()
            {
                return Err(BundleError::LogBrokenLink {
                    log_index: entry.log_index,
                    detail: "digest chain verification failed".to_string(),
                });
            }
            entries.push(entry);
        }
        Ok(Self { entries })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedBundle {
    pub manifest: Manifest,
    pub generation: ProjectGeneration,
    pub transactions: String,
    pub log: TransactionLog,
    pub graph: FeatureGraph,
}

impl LoadedBundle {
    pub fn feature_graph_hash_hex(&self) -> &str {
        &self.manifest.feature_graph_hash
    }

    pub fn revision_hash_hex(&self) -> &str {
        &self.manifest.revision_hash
    }
}

#[derive(Debug)]
pub enum BundleError {
    ManifestMissing,
    LogMissing,
    SchemaGenerationUnsupported {
        found: u32,
    },
    LogDigestMismatch,
    LogBrokenLink {
        log_index: usize,
        detail: String,
    },
    Io(String),
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

impl BundleError {
    pub fn diagnostic_detail(&self) -> &'static str {
        match self {
            Self::ManifestMissing => "manifest_missing",
            Self::LogMissing => "log_missing",
            Self::SchemaGenerationUnsupported { .. } => "schema_generation_unsupported",
            Self::LogDigestMismatch => "log_digest_mismatch",
            Self::LogBrokenLink { .. } => "log_broken_link",
            Self::Io(_) => "bundle_io_failure",
            Self::Invalid(_) => "bundle_invalid",
            Self::SchemaUnknown { .. } => "schema_unknown",
            Self::SchemaTooOld { .. } => "schema_too_old",
            Self::SchemaTooNew { .. } => "schema_too_new",
            Self::ManifestFieldUnknown { .. } => "manifest_field_unknown",
            Self::Backup { .. } => "backup_failed",
            Self::SourceUnreadable { .. } => "source_unreadable",
            Self::Migration { .. } => "migration_failed",
        }
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifestMissing => formatter.write_str("manifest missing"),
            Self::LogMissing => formatter.write_str("transaction log missing"),
            Self::SchemaGenerationUnsupported { found } => {
                write!(formatter, "unsupported schema generation: {found}")
            }
            Self::LogDigestMismatch => formatter.write_str("log digest mismatch"),
            Self::LogBrokenLink { log_index, detail } => {
                write!(formatter, "log broken link at entry {log_index}: {detail}")
            }
            Self::Io(detail) | Self::Invalid(detail) => formatter.write_str(detail),
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
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for BundleError {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct Bundle {
    root: PathBuf,
}

impl Bundle {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn create(root: impl Into<PathBuf>) -> Result<Self, BundleError> {
        let mut random = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        Self::create_for_test(root, &hex(&random))
    }

    pub fn create_for_test(
        root: impl Into<PathBuf>,
        generation_id: &str,
    ) -> Result<Self, BundleError> {
        Self::create_with_revision(root, generation_id, "revision-0")
    }

    fn create_with_revision(
        root: impl Into<PathBuf>,
        generation_id: &str,
        revision_id: &str,
    ) -> Result<Self, BundleError> {
        let bundle = Self::at(root);
        if bundle.root.exists() {
            return Err(BundleError::Invalid(format!(
                "destination already exists: {}",
                bundle.root.display()
            )));
        }
        fs::create_dir_all(&bundle.root)?;
        let log = TransactionLog::empty();
        let graph = FeatureGraph::empty();
        atomic_write(&bundle.transactions_path(), &log.encode())?;
        let manifest = Manifest::seal(generation_id, revision_id, &log, &graph);
        atomic_write(
            &bundle.manifest_path(),
            &serde_json::to_vec_pretty(&manifest)
                .map_err(|error| BundleError::Invalid(error.to_string()))?,
        )?;
        Ok(bundle)
    }

    pub fn open(&self) -> Result<LoadedBundle, BundleError> {
        let manifest_bytes = read_required(&self.manifest_path(), BundleError::ManifestMissing)?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| BundleError::Invalid(error.to_string()))?;
        if manifest.schema_generation != MANIFEST_SCHEMA_GENERATION {
            return Err(BundleError::SchemaGenerationUnsupported {
                found: manifest.schema_generation,
            });
        }
        if manifest.schema_version != schema_epoch() {
            return Err(BundleError::SchemaGenerationUnsupported {
                found: manifest.schema_generation,
            });
        }

        let transaction_bytes = read_required(&self.transactions_path(), BundleError::LogMissing)?;
        let log = TransactionLog::decode_and_verify(&transaction_bytes)?;
        if log.terminal_digest_hex() != manifest.terminal_log_digest {
            return Err(BundleError::LogDigestMismatch);
        }

        let mut graph = FeatureGraph::empty();
        let mut feature_ids = Vec::new();
        for entry in log.entries() {
            let feature = Feature::new(&entry.feature_id, &entry.kind).map_err(|error| {
                BundleError::LogBrokenLink {
                    log_index: entry.log_index,
                    detail: error.to_string(),
                }
            })?;
            feature_ids.push(feature.id.clone());
            graph.add_feature(feature);
        }
        if manifest.transaction_count != log.len()
            || manifest.transaction_bytes != transaction_bytes.len()
            || manifest.transaction_sha256 != hash(&transaction_bytes)
            || manifest.feature_graph_hash != graph.graph_hash_hex()
            || manifest.revision_hash != graph.revision_hash_hex(log.terminal_digest_hex())
            || manifest.canonical_root_sha256 != hash(&canonical_manifest_bytes(&manifest))
            || manifest.seal_sha256 != hash(&sealed_manifest_bytes(&manifest))
        {
            return Err(BundleError::LogDigestMismatch);
        }

        let generation = ProjectGeneration {
            id: manifest.generation_id.clone(),
            revisions: vec![Revision {
                id: manifest.revision_id.clone(),
                features: feature_ids,
            }],
        };
        let transactions = String::from_utf8(transaction_bytes)
            .map_err(|error| BundleError::Invalid(error.to_string()))?;
        Ok(LoadedBundle {
            manifest,
            generation,
            transactions,
            log,
            graph,
        })
    }

    pub fn append_feature(
        &self,
        feature_id: &str,
        kind: &str,
    ) -> Result<LoadedBundle, BundleError> {
        let mut loaded = self.open()?;
        let feature = Feature::new(feature_id, kind)
            .map_err(|error| BundleError::Invalid(error.to_string()))?;
        if !loaded.graph.add_feature(feature) {
            return Ok(loaded);
        }

        loaded.log.append_feature(feature_id, kind);
        let last = loaded
            .log
            .entries()
            .last()
            .expect("appended log has an entry");
        let mut line =
            serde_json::to_vec(last).map_err(|error| BundleError::Invalid(error.to_string()))?;
        line.push(b'\n');
        append_and_sync(&self.transactions_path(), &line)?;

        loaded.manifest = Manifest::seal(
            &loaded.manifest.generation_id,
            &loaded.manifest.revision_id,
            &loaded.log,
            &loaded.graph,
        );
        atomic_write(
            &self.manifest_path(),
            &serde_json::to_vec_pretty(&loaded.manifest)
                .map_err(|error| BundleError::Invalid(error.to_string()))?,
        )?;
        self.open()
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILENAME)
    }

    fn transactions_path(&self) -> PathBuf {
        self.root.join(TRANSACTIONS_LOG_FILENAME)
    }
}

pub fn write_fresh(path: &Path, generation: ProjectGeneration) -> Result<Manifest, BundleError> {
    let revision = generation
        .revisions
        .first()
        .filter(|revision| generation.revisions.len() == 1 && revision.features.is_empty())
        .ok_or_else(|| {
            BundleError::Invalid("fresh generation must contain one empty revision".to_string())
        })?;
    let bundle = Bundle::create_with_revision(path, &generation.id, &revision.id)?;
    Ok(bundle.open()?.manifest)
}

pub fn load(path: &Path) -> Result<LoadedBundle, BundleError> {
    let root = path;
    if !root.exists() {
        return Err(BundleError::Invalid(format!(
            "bundle path missing: {}",
            root.display()
        )));
    }
    if !root.is_dir() {
        return Err(BundleError::Invalid(format!(
            "bundle path is not a directory: {}",
            root.display()
        )));
    }
    let status = detect_schema(root)?;
    match status {
        SchemaStatus::Current => load_v1(root),
        SchemaStatus::Prior => load_v0_with_migration(root),
        SchemaStatus::Unknown => Err(BundleError::SchemaUnknown {
            found: read_schema_version_raw(root).unwrap_or_default(),
            expected_current: schema_epoch(),
            expected_prior: prior_schema_epoch(),
        }),
    }
}

fn load_v1(path: &Path) -> Result<LoadedBundle, BundleError> {
    Bundle::at(path).open()
}

fn read_schema_version_raw(path: &Path) -> Option<String> {
    let raw = fs::read(path.join(MANIFEST_FILENAME)).ok()?;
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
    let (manifest, generation) = migrate_v0_to_v1(&v0);
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

    let validate = Bundle::at(&staging).open();
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
        return Err(BundleError::Io(error.to_string()));
    }

    Ok(loaded_with(validated, manifest, generation, transactions))
}

fn loaded_with(
    stale: LoadedBundle,
    manifest: Manifest,
    generation: ProjectGeneration,
    transactions: String,
) -> LoadedBundle {
    LoadedBundle {
        manifest,
        generation,
        transactions,
        log: stale.log,
        graph: stale.graph,
    }
}

fn publish_staged(staging: &Path, destination: &Path) -> std::io::Result<()> {
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
    fs::rename(staging, destination)
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

fn write_v1_into(
    staging: &Path,
    manifest: &Manifest,
    transactions: &[u8],
) -> Result<(), BundleError> {
    fs::create_dir_all(staging)?;
    fs::write(staging.join(TRANSACTIONS_LOG_FILENAME), transactions)?;
    fs::write(
        staging.join(MANIFEST_FILENAME),
        serde_json::to_vec_pretty(manifest)?,
    )?;
    Ok(())
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
    let raw = read_required(&path.join(MANIFEST_FILENAME), BundleError::ManifestMissing)?;
    let value: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(error) => {
            if let Some(field) = unknown_field_in(&error.to_string()) {
                return Err(BundleError::ManifestFieldUnknown {
                    kind: "manifest",
                    field,
                });
            }
            return Err(BundleError::Invalid(error.to_string()));
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
    let raw = read_required(&path.join(MANIFEST_FILENAME), BundleError::ManifestMissing)?;
    let manifest: V0Manifest = match serde_json::from_slice(&raw) {
        Ok(manifest) => manifest,
        Err(error) => {
            if let Some(field) = unknown_field_in(&error.to_string()) {
                return Err(BundleError::ManifestFieldUnknown { kind: "v0", field });
            }
            return Err(BundleError::Invalid(error.to_string()));
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
    let transactions = String::from_utf8(fs::read(path.join("canonical/transactions.ndjson"))?)
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
    fs::write(
        staging.join("canonical/transactions.ndjson"),
        transactions.as_bytes(),
    )?;
    fs::write(
        staging.join(MANIFEST_FILENAME),
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
        }],
    };
    let mut manifest = Manifest {
        schema_version: schema_epoch().to_string(),
        schema_generation: MANIFEST_SCHEMA_GENERATION,
        generation_id: source.manifest.generation_id.clone(),
        revision_id: source.manifest.revision_id.clone(),
        revision_count: source.manifest.revision_count,
        transaction_count: source.manifest.transaction_count,
        transaction_bytes: source.manifest.transaction_bytes,
        transaction_sha256: source.manifest.transaction_sha256.clone(),
        terminal_log_digest: EMPTY_LOG_DIGEST_HEX.to_string(),
        feature_graph_hash: String::new(),
        revision_hash: String::new(),
        canonical_root_sha256: String::new(),
        seal_sha256: String::new(),
    };
    let empty_graph = FeatureGraph::empty();
    manifest.feature_graph_hash = empty_graph.graph_hash_hex();
    manifest.revision_hash = empty_graph.revision_hash_hex(&manifest.terminal_log_digest);
    manifest.canonical_root_sha256 = hash(&canonical_manifest_bytes(&manifest));
    manifest.seal_sha256 = hash(&sealed_manifest_bytes(&manifest));
    (manifest, generation)
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

fn read_required(path: &Path, missing: BundleError) -> Result<Vec<u8>, BundleError> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(missing),
        Err(error) => Err(error.into()),
    }
}

fn append_and_sync(path: &Path, bytes: &[u8]) -> Result<(), BundleError> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), BundleError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| BundleError::Invalid("target has no file name".to_string()))?;
    let temporary = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));
    let mut file = File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
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
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use threeterm_domain::Revision;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "threeterm-persistence-{}-{}-{}",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_epoch(), "threeterm.persistence/1");
        assert_eq!(prior_schema_epoch(), "threeterm.persistence/0");
    }

    #[test]
    fn append_then_verified_reopen_preserves_graph_revision_and_entry_count() {
        let root = temp_root("append");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        let saved = bundle
            .append_feature("box-1", "box")
            .expect("feature appends");
        assert_eq!(saved.log.len(), 1);

        let duplicate = bundle
            .append_feature("box-1", "box")
            .expect("duplicate saves");
        assert_eq!(duplicate.log.len(), 1);

        let loaded = bundle.open().expect("bundle reopens");
        assert_eq!(loaded.log.len(), 1);
        assert_eq!(
            loaded.feature_graph_hash_hex(),
            saved.feature_graph_hash_hex()
        );
        assert_eq!(loaded.revision_hash_hex(), saved.revision_hash_hex());
        assert!(root.join(MANIFEST_FILENAME).is_file());
        assert!(root.join(TRANSACTIONS_LOG_FILENAME).is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_canonical_files_have_stable_failure_kinds() {
        let missing_manifest = temp_root("missing-manifest");
        fs::create_dir_all(&missing_manifest).expect("root creates");
        fs::write(missing_manifest.join(TRANSACTIONS_LOG_FILENAME), b"").expect("log creates");
        assert!(matches!(
            Bundle::at(&missing_manifest).open(),
            Err(BundleError::ManifestMissing)
        ));

        let missing_log = temp_root("missing-log");
        let bundle = Bundle::create_for_test(&missing_log, "00".repeat(16).as_str())
            .expect("bundle creates");
        fs::remove_file(missing_log.join(TRANSACTIONS_LOG_FILENAME)).expect("log removes");
        assert!(matches!(bundle.open(), Err(BundleError::LogMissing)));

        let _ = fs::remove_dir_all(missing_manifest);
        let _ = fs::remove_dir_all(missing_log);
    }

    #[test]
    fn unsupported_schema_generations_fail_closed() {
        for generation in [0, 2] {
            let root = temp_root(&format!("schema-{generation}"));
            let bundle =
                Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
            let path = root.join(MANIFEST_FILENAME);
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).expect("manifest reads"))
                    .expect("manifest parses");
            manifest["schema_generation"] = generation.into();
            fs::write(
                &path,
                serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
            )
            .expect("manifest writes");
            assert!(matches!(
                bundle.open(),
                Err(BundleError::SchemaGenerationUnsupported { found: g }) if g == generation
            ));
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn altered_log_entry_is_reported_as_broken_link() {
        let root = temp_root("broken-link");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        bundle
            .append_feature("box-1", "box")
            .expect("feature appends");
        let path = root.join(TRANSACTIONS_LOG_FILENAME);
        let mut entry: serde_json::Value =
            serde_json::from_str(fs::read_to_string(&path).expect("log reads").trim())
                .expect("entry parses");
        entry["kind"] = "sphere".into();
        fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string(&entry).expect("entry serializes")
            ),
        )
        .expect("entry writes");
        assert!(matches!(
            bundle.open(),
            Err(BundleError::LogBrokenLink { log_index: 0, .. })
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn altered_previous_digest_is_reported_as_broken_link() {
        let root = temp_root("previous-link");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        bundle
            .append_feature("box-1", "box")
            .expect("first feature appends");
        bundle
            .append_feature("box-2", "box")
            .expect("second feature appends");
        let path = root.join(TRANSACTIONS_LOG_FILENAME);
        let contents = fs::read_to_string(&path).expect("log reads");
        let mut entries: Vec<serde_json::Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).expect("entry parses"))
            .collect();
        entries[1]["previous_digest"] = "f".repeat(64).into();
        let rewritten = entries
            .iter()
            .map(|entry| serde_json::to_string(entry).expect("entry serializes"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&path, rewritten).expect("log writes");
        assert!(matches!(
            bundle.open(),
            Err(BundleError::LogBrokenLink { log_index: 1, .. })
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn altered_manifest_terminal_is_reported_as_log_digest_mismatch() {
        let root = temp_root("digest-mismatch");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        bundle
            .append_feature("box-1", "box")
            .expect("feature appends");
        let path = root.join(MANIFEST_FILENAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("manifest reads"))
                .expect("manifest parses");
        manifest["terminal_log_digest"] = "f".repeat(64).into();
        fs::write(
            &path,
            serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
        )
        .expect("manifest writes");
        assert!(matches!(bundle.open(), Err(BundleError::LogDigestMismatch)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fresh_bundle_round_trips_empty_generation() {
        let root = temp_root("empty");
        let generation = ProjectGeneration::with_id("generation-test");
        write_fresh(&root, generation).expect("bundle writes");
        let loaded = load(&root).expect("bundle loads");
        assert_eq!(loaded.generation.revisions, vec![Revision::empty()]);
        assert!(loaded.transactions.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tampered_transaction_log_is_rejected() {
        let root = temp_root("tamper");
        let _ = fs::remove_dir_all(&root);
        write_fresh(&root, ProjectGeneration::with_id("generation-tamper")).expect("bundle writes");
        fs::write(root.join(TRANSACTIONS_LOG_FILENAME), b"tampered\n").expect("log changes");
        assert!(load(&root).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn detect_schema_classifies_v0_and_v1() {
        let root_v0 = temp_root("detect-v0");
        let root_v1 = temp_root("detect-v1");
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
        let root = temp_root("migrate");
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
        let root = temp_root("unknown-field");
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
        fs::write(root.join(MANIFEST_FILENAME), bad).expect("manifest");
        fs::write(root.join(TRANSACTIONS_LOG_FILENAME), b"").expect("log");
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

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threeterm_domain::{Feature, FeatureGraph, ProjectGeneration, Revision};

pub mod bundle {
    pub use super::{
        Bundle, BundleError, LoadedBundle, Manifest, load, schema_version, write_fresh,
    };
}

pub fn schema_version() -> &'static str {
    "threeterm.persistence/1"
}

pub const MANIFEST_FILENAME: &str = "manifest.json";
pub const TRANSACTIONS_LOG_FILENAME: &str = "transactions.log";
pub const MANIFEST_SCHEMA_GENERATION: u32 = 1;
pub const EMPTY_LOG_DIGEST_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
            schema_version: schema_version().to_string(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    ManifestMissing,
    LogMissing,
    SchemaGenerationUnsupported { found: u32 },
    LogDigestMismatch,
    LogBrokenLink { log_index: usize, detail: String },
    Io(String),
    Invalid(String),
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
        }
    }
}

impl std::error::Error for BundleError {}

impl From<std::io::Error> for BundleError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
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
        if manifest.schema_version != schema_version() {
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
    Bundle::at(path).open()
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
        assert_eq!(schema_version(), "threeterm.persistence/1");
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
        assert_eq!(
            Bundle::at(&missing_manifest).open(),
            Err(BundleError::ManifestMissing)
        );

        let missing_log = temp_root("missing-log");
        let bundle = Bundle::create_for_test(&missing_log, "00".repeat(16).as_str())
            .expect("bundle creates");
        fs::remove_file(missing_log.join(TRANSACTIONS_LOG_FILENAME)).expect("log removes");
        assert_eq!(bundle.open(), Err(BundleError::LogMissing));

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
            assert_eq!(
                bundle.open(),
                Err(BundleError::SchemaGenerationUnsupported { found: generation })
            );
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
        assert_eq!(bundle.open(), Err(BundleError::LogDigestMismatch));
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
}

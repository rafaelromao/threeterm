use std::cell::RefCell;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threeterm_domain::{
    ComponentCommand, ComponentGraph, Feature, FeatureGraph, FitDimension, ProjectGeneration,
    Revision,
    history::{
        HistoryEvent, HistoryOperation, HistoryState, HistoryTimeline, project_feature_timeline,
    },
};

const COMPONENT_COMMAND_KIND_PREFIX: &str = "component-command:";
const SKETCH_COMMAND_KIND_PREFIX: &str = "sketch-command:";
const FIT_DIMENSION_KIND_PREFIX: &str = "fit-dimension/1:";
pub const HISTORY_EVENT_KIND_PREFIX: &str = "history-event:";

/// Classification of a `.threeterm/` bundle's manifest schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStatus {
    Current,
    Prior,
    Unknown,
}

/// Controls whether a prior schema epoch may be migrated during load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPolicy {
    MigrateV0,
    RejectV0RequiresBackup,
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
        Bundle, BundleError, EMPTY_LOG_DIGEST_HEX, HISTORY_EVENT_KIND_PREFIX, LoadPolicy,
        LoadedBundle, LogEntry, MANIFEST_FILENAME, MANIFEST_SCHEMA_GENERATION, Manifest,
        PRE_MIGRATION_BACKUP_SUFFIX, PUBLICATION_KILL_POINT_ENV, PublicationFailurePoint,
        PublicationKillPoint, SchemaStatus, TRANSACTIONS_LOG_FILENAME, TransactionLog, V0Bundle,
        V0Manifest, detect_schema, fail_next_publication_at, load, load_with_policy,
        migrate_v0_to_v1, prior_schema_epoch, read_v0, schema_epoch, write_fresh, write_v0_fixture,
    };
}

pub const PRE_MIGRATION_BACKUP_SUFFIX: &str = ".pre-migration-backup";
pub const PREVIOUS_GENERATION_SUFFIX: &str = ".previous-generation";

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
/// Harness-only environment variable; it is not part of the end-user CLI contract.
#[doc(hidden)]
pub const PUBLICATION_KILL_POINT_ENV: &str = "THREETERM_PUBLICATION_KILL_POINT";

/// Named publication boundaries used by the child-process recovery harness.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationKillPoint {
    StagedFiles,
    LogSeal,
    ManifestSeal,
    StagingSync,
    StagingValidation,
    RetirePrevious,
    ReplaceCurrent,
    PromoteStaging,
    ParentSync,
    RetiredCleanup,
}

impl PublicationKillPoint {
    pub const ALL: [Self; 10] = [
        Self::StagedFiles,
        Self::LogSeal,
        Self::ManifestSeal,
        Self::StagingSync,
        Self::StagingValidation,
        Self::RetirePrevious,
        Self::ReplaceCurrent,
        Self::PromoteStaging,
        Self::ParentSync,
        Self::RetiredCleanup,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StagedFiles => "staged-files",
            Self::LogSeal => "log-seal",
            Self::ManifestSeal => "manifest-seal",
            Self::StagingSync => "staging-sync",
            Self::StagingValidation => "staging-validation",
            Self::RetirePrevious => "retire-previous",
            Self::ReplaceCurrent => "replace-current",
            Self::PromoteStaging => "promote-staging",
            Self::ParentSync => "parent-sync",
            Self::RetiredCleanup => "retired-cleanup",
        }
    }
}

fn terminate_at_requested_publication_point(point: PublicationKillPoint) {
    #[cfg(feature = "publication-kill-points")]
    {
        // This seam is inert unless the recovery harness explicitly opts in on a
        // child process; normal callers never terminate from the persistence API.
        if std::env::var(PUBLICATION_KILL_POINT_ENV).ok().as_deref() == Some(point.as_str()) {
            // Exit without unwinding so the child leaves the filesystem at the
            // exact publication boundary selected by the recovery harness.
            std::process::exit(137);
        }
    }

    #[cfg(not(feature = "publication-kill-points"))]
    let _ = point;
}

/// A deterministic filesystem-operation boundary for generation-publication
/// tests. The hook makes the selected operation return an I/O error, then
/// clears itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationFailurePoint {
    StagedFiles,
    LogSync,
    ManifestSync,
    BrepCopy,
    BrepWrite,
    BrepSync,
    BrepRename,
    BrepDirectorySync,
    StagingSync,
    RetirePrevious,
    ReplaceCurrent,
    PromoteStaging,
    ParentSync,
    RetiredCleanup,
}

impl PublicationFailurePoint {
    fn kill_point(self) -> PublicationKillPoint {
        match self {
            Self::StagedFiles => PublicationKillPoint::StagedFiles,
            Self::LogSync => PublicationKillPoint::LogSeal,
            Self::ManifestSync => PublicationKillPoint::ManifestSeal,
            Self::BrepCopy
            | Self::BrepWrite
            | Self::BrepSync
            | Self::BrepRename
            | Self::BrepDirectorySync => PublicationKillPoint::StagingSync,
            Self::StagingSync => PublicationKillPoint::StagingSync,
            Self::RetirePrevious => PublicationKillPoint::RetirePrevious,
            Self::ReplaceCurrent => PublicationKillPoint::ReplaceCurrent,
            Self::PromoteStaging => PublicationKillPoint::PromoteStaging,
            Self::ParentSync => PublicationKillPoint::ParentSync,
            Self::RetiredCleanup => PublicationKillPoint::RetiredCleanup,
        }
    }
}

thread_local! {
    static NEXT_PUBLICATION_FAILURE: RefCell<Option<PublicationFailurePoint>> = const { RefCell::new(None) };
}

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn fail_next_publication_at(point: PublicationFailurePoint) {
    NEXT_PUBLICATION_FAILURE.with(|next| *next.borrow_mut() = Some(point));
}

fn fail_if_injected(point: PublicationFailurePoint) -> std::io::Result<()> {
    NEXT_PUBLICATION_FAILURE.with(|next| {
        let mut next = next.borrow_mut();
        if *next == Some(point) {
            *next = None;
            Err(std::io::Error::other("injected publication failure"))
        } else {
            Ok(())
        }
    })
}

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
    #[allow(clippy::too_many_arguments)]
    fn seal(
        _generation_id: &str,
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
            // The Project Generation identity is the canonical log
            // digest: for new bundles and append operations the
            // chain head is the identity. The v0 → v1 migration
            // path constructs the manifest directly (without going
            // through this seal) so the prior identity is preserved.
            // The `generation_id` parameter is retained for API
            // shape but is not consulted here.
            generation_id: terminal_log_digest.clone(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brep_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brep_byte_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brep_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_payload: Option<String>,
    pub terminal_digest: String,
}

impl LogEntry {
    fn new(log_index: usize, previous_digest: &str, feature_id: &str, kind: &str) -> Self {
        let mut entry = Self {
            log_index,
            previous_digest: previous_digest.to_string(),
            feature_id: feature_id.to_string(),
            kind: kind.to_string(),
            brep_path: None,
            brep_byte_count: None,
            brep_sha256: None,
            idempotency_key: None,
            idempotency_payload: None,
            terminal_digest: String::new(),
        };
        entry.terminal_digest = entry.recomputed_digest();
        entry
    }

    fn with_idempotency_key(mut self, key: &str, payload: Option<&str>) -> Self {
        self.idempotency_key = Some(key.to_string());
        self.idempotency_payload = payload.map(str::to_string);
        self.terminal_digest = self.recomputed_digest();
        self
    }

    fn with_brep(mut self, bytes: &[u8]) -> Self {
        self.brep_path = Some(format!("brep/{}.brep", self.feature_id));
        self.brep_byte_count = Some(bytes.len() as u64);
        self.brep_sha256 = Some(hash(bytes));
        self.terminal_digest = self.recomputed_digest();
        self
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

    fn append_feature_with_idempotency(
        &mut self,
        feature_id: &str,
        kind: &str,
        idempotency_key: &str,
        idempotency_payload: Option<&str>,
    ) {
        let previous = self.terminal_digest_hex().to_string();
        self.entries.push(
            LogEntry::new(self.entries.len(), &previous, feature_id, kind)
                .with_idempotency_key(idempotency_key, idempotency_payload),
        );
    }

    fn append_feature_with_brep(
        &mut self,
        feature_id: &str,
        kind: &str,
        brep_bytes: &[u8],
        idempotency_key: Option<&str>,
        idempotency_payload: Option<&str>,
    ) {
        let previous = self.terminal_digest_hex().to_string();
        let entry =
            LogEntry::new(self.entries.len(), &previous, feature_id, kind).with_brep(brep_bytes);
        let entry = match idempotency_key {
            Some(key) => entry.with_idempotency_key(key, idempotency_payload),
            None => entry,
        };
        self.entries.push(entry);
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

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedBundle {
    /// The exact sealed generation selected by `Bundle::open`, including a
    /// previous-generation recovery path when the canonical root is absent.
    pub canonical_root: PathBuf,
    pub manifest: Manifest,
    pub generation: ProjectGeneration,
    pub transactions: String,
    pub log: TransactionLog,
    pub graph: FeatureGraph,
    pub components: ComponentGraph,
    pub history: HistoryState,
    pub history_events: Vec<HistoryEvent>,
    /// `true` when the canonical path was unavailable and the immediately
    /// preceding sealed Project Generation was opened instead.
    pub recovered_from_previous: bool,
}

impl LoadedBundle {
    pub fn feature_graph_hash_hex(&self) -> &str {
        &self.manifest.feature_graph_hash
    }

    pub fn revision_hash_hex(&self) -> &str {
        &self.manifest.revision_hash
    }

    pub fn feature_timeline(&self, feature_id: &str) -> Result<HistoryTimeline, BundleError> {
        project_feature_timeline(&self.history_events, feature_id)
            .map_err(|error| BundleError::Invalid(error.to_string()))
    }
}

#[derive(Debug)]
pub enum BundleError {
    ManifestMissing,
    LogMissing,
    BundlePathMissing {
        path: PathBuf,
    },
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
    PublicationUnknown(String),
    SourceUnreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    Migration {
        source: Box<BundleError>,
    },
    SchemaEpochV0RequiresBackup,
}

impl BundleError {
    pub fn diagnostic_detail(&self) -> &'static str {
        match self {
            Self::ManifestMissing => "manifest_missing",
            Self::LogMissing => "log_missing",
            Self::BundlePathMissing { .. } => "bundle_path_missing",
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
            Self::PublicationUnknown(_) => "publication_unknown",
            Self::SourceUnreadable { .. } => "source_unreadable",
            Self::Migration { .. } => "migration_failed",
            Self::SchemaEpochV0RequiresBackup => "SCHEMA_EPOCH_V0_REQUIRES_BACKUP",
        }
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifestMissing => formatter.write_str("manifest missing"),
            Self::LogMissing => formatter.write_str("transaction log missing"),
            Self::BundlePathMissing { path } => {
                write!(formatter, "bundle path missing: {}", path.display())
            }
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
            Self::PublicationUnknown(detail) => formatter.write_str(detail),
            Self::SourceUnreadable { path, source } => write!(
                formatter,
                "persistence.source-unreadable: path={} source={source}",
                path.display()
            ),
            Self::Migration { source } => {
                write!(formatter, "persistence.migration-failed: {source}")
            }
            Self::SchemaEpochV0RequiresBackup => {
                formatter.write_str("schema epoch v0 requires a backup before loading")
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
        Self {
            root: canonical_bundle_root(&root.into()),
        }
    }

    /// The canonical root directory this bundle operates on.
    ///
    /// Symlink aliases of one bundle resolve to the same canonical root, so
    /// writers addressed through an alias and writers addressed through the
    /// target share one lock and one set of recovery siblings.
    pub fn canonical_root(&self) -> &Path {
        &self.root
    }

    pub fn create(root: impl Into<PathBuf>) -> Result<Self, BundleError> {
        // The Project Generation identity is the canonical log digest;
        // a fresh bundle's identity is the empty log identity. After the
        // first accepted command transaction, the identity advances to
        // the chain head.
        Self::create_for_test(root, EMPTY_LOG_DIGEST_HEX)
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
        let root = root.into();
        with_bundle_write_lock(&root, || {
            Self::create_staged(&root, generation_id, revision_id)
        })
    }

    /// Create the bundle directory structure without acquiring the write
    /// lock. Callers must already hold the per-root lock; `create_staged`
    /// writes into a staging directory and `append_features_locked` folds a
    /// concurrent first save into a plain append.
    fn create_inner(
        root: &Path,
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
        atomic_write(
            &bundle.transactions_path(),
            &log.encode(),
            Some(PublicationFailurePoint::LogSync),
        )?;
        let manifest = Manifest::seal(generation_id, revision_id, &log, &graph);
        atomic_write(
            &bundle.manifest_path(),
            &serde_json::to_vec_pretty(&manifest)
                .map_err(|error| BundleError::Invalid(error.to_string()))?,
            Some(PublicationFailurePoint::ManifestSync),
        )?;
        // A fresh bundle is a new directory entry: it is only durable once
        // the containing directory is synced.
        if let Some(parent) = bundle
            .root
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            File::open(parent)?.sync_all()?;
        }
        Ok(bundle)
    }

    /// Stage, validate, and atomically promote a fresh bundle.
    ///
    /// Callers must already hold the per-root write lock. The empty baseline
    /// is written into a staging directory, validated as a sealed
    /// generation, and only then promoted into the canonical root, so a
    /// crash or I/O failure mid-creation never leaves a partial canonical
    /// root that a later save would treat as the current source.
    fn create_staged(
        root: &Path,
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
        let staging = fresh_staging_path_for_publish(&bundle.root);
        let prepared = (|| {
            Self::create_inner(&staging, generation_id, revision_id)?;
            fail_if_injected(PublicationFailurePoint::StagedFiles)?;
            sync_directory(&staging, PublicationFailurePoint::StagingSync)?;
            Bundle::at(&staging).open_sealed(false)?;
            Ok::<(), BundleError>(())
        })();
        if let Err(error) = prepared {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        publish_staged(&staging, &bundle.root).map_err(|error| match error {
            PublicationError::Io(error) => BundleError::Io(error.to_string()),
            PublicationError::Unknown(error) => BundleError::PublicationUnknown(error.to_string()),
        })?;
        Ok(bundle)
    }

    /// Open the selected canonical generation, reconciling any interrupted
    /// rotation first.
    ///
    /// Reconciliation mutates the rotation slots (it can rename the retired
    /// generation back into the previous slot), so it runs under the same
    /// per-root write lock a publisher holds for the whole read-modify-
    /// publish cycle. A lock-free reconcile could interleave with a writer
    /// between `previous → retired` and `destination → previous`, restoring
    /// the previous slot and failing the writer's replacement. Serializing
    /// opens with publishes closes that race: a reader never observes a
    /// half-rotated state, and a writer never sees a foreign `previous`.
    pub fn open(&self) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || self.open_locked())
    }

    /// The locked body of `open`. Callers must already hold the per-root
    /// write lock; `append_features_locked`, `load_unlocked`, and the
    /// migration staging validation call it from inside the lock.
    fn open_locked(&self) -> Result<LoadedBundle, BundleError> {
        reconcile_interrupted_rotation(&self.root)?;
        match self.open_sealed(false) {
            Ok(loaded) => Ok(loaded),
            Err(BundleError::ManifestMissing) if !self.root.exists() => {
                Self::at(previous_generation_path(&self.root)).open_sealed(true)
            }
            Err(error) => Err(error),
        }
    }

    fn open_sealed(&self, recovered_from_previous: bool) -> Result<LoadedBundle, BundleError> {
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
        let mut components = ComponentGraph::default();
        let mut history = HistoryState::default();
        let mut history_events = Vec::new();
        let mut feature_ids = Vec::new();
        for entry in log.entries() {
            if let Some(payload) = entry.kind.strip_prefix(HISTORY_EVENT_KIND_PREFIX) {
                let event: HistoryEvent =
                    serde_json::from_str(payload).map_err(|error| BundleError::LogBrokenLink {
                        log_index: entry.log_index,
                        detail: format!("invalid history event: {error}"),
                    })?;
                history
                    .apply_event(&event)
                    .map_err(|error| BundleError::LogBrokenLink {
                        log_index: entry.log_index,
                        detail: error.to_string(),
                    })?;
                history_events.push(event);
                continue;
            }
            if let Some(payload) = entry.kind.strip_prefix(FIT_DIMENSION_KIND_PREFIX) {
                let fit: FitDimension =
                    serde_json::from_str(payload).map_err(|error| BundleError::LogBrokenLink {
                        log_index: entry.log_index,
                        detail: format!("invalid fit dimension: {error}"),
                    })?;
                graph
                    .add_fit_dimension(fit)
                    .map_err(|detail| BundleError::LogBrokenLink {
                        log_index: entry.log_index,
                        detail,
                    })?;
                continue;
            }
            let sketch_payload = entry
                .kind
                .strip_prefix(SKETCH_COMMAND_KIND_PREFIX)
                .map(|payload| {
                    serde_json::from_str::<threeterm_domain::SketchPayload>(payload).map_err(
                        |error| BundleError::LogBrokenLink {
                            log_index: entry.log_index,
                            detail: format!("invalid sketch payload: {error}"),
                        },
                    )
                })
                .transpose()?;
            let feature_kind = if sketch_payload.is_some() {
                "sketch"
            } else {
                &entry.kind
            };
            let feature = Feature::new(&entry.feature_id, feature_kind).map_err(|error| {
                BundleError::LogBrokenLink {
                    log_index: entry.log_index,
                    detail: error.to_string(),
                }
            })?;
            feature_ids.push(feature.id.clone());
            if let Some(payload) = sketch_payload {
                graph.add_sketch(feature, payload).map_err(|detail| {
                    BundleError::LogBrokenLink {
                        log_index: entry.log_index,
                        detail,
                    }
                })?;
            } else {
                graph.add_feature(feature);
            }
            if let Some(command) = entry.kind.strip_prefix(COMPONENT_COMMAND_KIND_PREFIX) {
                let command: ComponentCommand =
                    serde_json::from_str(command).map_err(|error| BundleError::LogBrokenLink {
                        log_index: entry.log_index,
                        detail: format!("invalid component command: {error}"),
                    })?;
                components
                    .apply(&command)
                    .map_err(|detail| BundleError::LogBrokenLink {
                        log_index: entry.log_index,
                        detail,
                    })?;
            }
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
        verify_brep_provenance(&self.root, &log)?;

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
            canonical_root: self.root.clone(),
            manifest,
            generation,
            transactions,
            log,
            graph,
            components,
            history,
            history_events,
            recovered_from_previous,
        })
    }

    pub fn append_feature(
        &self,
        feature_id: &str,
        kind: &str,
    ) -> Result<LoadedBundle, BundleError> {
        self.append_features(&[(feature_id, kind)])
    }

    /// Append a revision-bound sketch result as canonical transaction data.
    /// The payload is encoded inside the authenticated log entry so old
    /// bundles remain readable while replay reconstructs the sketch graph.
    pub fn append_sketch_if_revision(
        &self,
        payload: &threeterm_domain::SketchPayload,
        expected_revision: &str,
    ) -> Result<LoadedBundle, BundleError> {
        payload.validate().map_err(BundleError::Invalid)?;
        let encoded = format!(
            "{SKETCH_COMMAND_KIND_PREFIX}{}",
            serde_json::to_string(payload)
                .map_err(|error| BundleError::Invalid(error.to_string()))?
        );
        self.append_feature_if_revision(&payload.feature_id, &encoded, expected_revision)
    }

    /// Append one versioned component command as a canonical transaction.
    /// The command payload is replayed on every open; no component snapshot is
    /// persisted as an authority.
    pub fn append_component_command(
        &self,
        command: &ComponentCommand,
    ) -> Result<LoadedBundle, BundleError> {
        self.append_component_command_with_revision(command, None)
    }

    /// Append one component command only if the bundle still has the revision
    /// whose history authorized the command.
    pub fn append_component_command_if_revision(
        &self,
        command: &ComponentCommand,
        expected_revision: &str,
    ) -> Result<LoadedBundle, BundleError> {
        self.append_component_command_with_revision(command, Some(expected_revision))
    }

    /// Append one relation-only fit transaction if the bundle still has the
    /// revision that supplied its sketch dimensions. Fit relations are
    /// canonical graph metadata, not synthetic geometry features.
    pub fn append_fit_dimension_if_revision(
        &self,
        fit: &FitDimension,
        expected_revision: &str,
    ) -> Result<LoadedBundle, BundleError> {
        fit.validate().map_err(BundleError::Invalid)?;
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked_with_fit(
                &[],
                Some(expected_revision),
                None,
                None,
                None,
                None,
                None,
                Some(fit),
                false,
            )
        })
    }

    fn append_component_command_with_revision(
        &self,
        command: &ComponentCommand,
        expected_revision: Option<&str>,
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            let loaded = if self.root.exists() || previous_generation_path(&self.root).exists() {
                self.open_locked()?
            } else {
                Self::create_staged(&self.root, EMPTY_LOG_DIGEST_HEX, "revision-0")?
                    .open_locked()?
            };
            let mut components = loaded.components.clone();
            components.apply(command).map_err(BundleError::Invalid)?;
            let payload = serde_json::to_string(command)
                .map_err(|error| BundleError::Invalid(error.to_string()))?;
            let feature_id = format!("component-transaction-{}", loaded.log.len());
            let kind = format!("{COMPONENT_COMMAND_KIND_PREFIX}{payload}");
            self.append_features_locked(
                &[(&feature_id, &kind)],
                expected_revision,
                None,
                None,
                None,
                None,
                None,
            )
        })
    }

    /// Append ordinary feature entries and one accepted history event in the
    /// same sealed publication. The event is part of the canonical log but is
    /// filtered from the legacy feature graph during replay.
    pub fn append_features_with_history(
        &self,
        entries: &[(&str, &str)],
        event: &HistoryEvent,
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(entries, None, None, None, None, None, Some(event))
        })
    }

    /// Replay the authenticated history event stream twice without touching
    /// the bundle. The two states are independent materializations used by
    /// the host's determinism verification boundary.
    pub fn replay_history_states(&self) -> Result<(HistoryState, HistoryState), BundleError> {
        let loaded = self.open()?;
        let first = replay_history_events(&loaded.log)?;
        let second = replay_history_events(&loaded.log)?;
        Ok((first, second))
    }

    pub fn feature_timeline(&self, feature_id: &str) -> Result<HistoryTimeline, BundleError> {
        self.open()?.feature_timeline(feature_id)
    }

    /// Append one feature only if the sealed bundle is still at the
    /// Revision Snapshot that authorized the operation. The check runs
    /// inside the bundle write lock, so a worker completion cannot publish
    /// stale geometry after another writer has advanced the revision.
    pub fn append_feature_if_revision(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(
                &[(feature_id, kind)],
                Some(expected_revision),
                None,
                None,
                None,
                None,
                None,
            )
        })
    }

    /// Publish one verified BREP together with its revision-guarded log entry
    /// in the same sealed generation. The source comparison, BREP staging, log
    /// rewrite, and generation rotation all occur under the bundle lock, so a
    /// stale worker can never overwrite a newer writer's canonical artifact.
    pub fn append_feature_with_brep_if_revision(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        brep_bytes: &[u8],
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(
                &[(feature_id, kind)],
                Some(expected_revision),
                Some((feature_id, brep_bytes)),
                None,
                None,
                None,
                None,
            )
        })
    }

    /// Publish one verified BREP and its command provenance as one canonical
    /// transaction. The provenance is metadata only: worker paths and raw
    /// artifact bytes remain outside the Canonical Transaction Log.
    pub fn append_feature_with_brep_if_revision_and_provenance(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        request_id: &str,
        provenance: &str,
        brep_bytes: &[u8],
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(
                &[(feature_id, kind)],
                Some(expected_revision),
                Some((feature_id, brep_bytes)),
                None,
                Some(request_id),
                Some(provenance),
                None,
            )
        })
    }

    /// Publish a verified BREP and command provenance for a new feature ID.
    /// The duplicate check and complete generation publication run under the
    /// same bundle write lock.
    pub fn append_new_feature_with_brep_if_revision_and_provenance(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        request_id: &str,
        provenance: &str,
        brep_bytes: &[u8],
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked_with_fit(
                &[(feature_id, kind)],
                Some(expected_revision),
                Some((feature_id, brep_bytes)),
                None,
                Some(request_id),
                Some(provenance),
                None,
                None,
                true,
            )
        })
    }

    /// Publish a BREP for a new feature ID only. The duplicate check and the
    /// complete generation publication run under the bundle write lock.
    pub fn append_new_feature_with_brep_if_revision(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        brep_bytes: &[u8],
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked_with_fit(
                &[(feature_id, kind)],
                Some(expected_revision),
                Some((feature_id, brep_bytes)),
                None,
                None,
                None,
                None,
                None,
                true,
            )
        })
    }

    /// Publish a verified BREP and its initial history event in one sealed
    /// generation.
    pub fn append_feature_with_brep_if_revision_and_history(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        brep_bytes: &[u8],
        history_event: &HistoryEvent,
    ) -> Result<LoadedBundle, BundleError> {
        self.append_features_with_brep_if_revision_and_history(
            &[(feature_id, kind)],
            feature_id,
            expected_revision,
            brep_bytes,
            history_event,
        )
    }

    /// Publish several feature entries, one verified BREP, and its initial
    /// history event in one sealed generation.
    pub fn append_features_with_brep_if_revision_and_history(
        &self,
        entries: &[(&str, &str)],
        brep_feature_id: &str,
        expected_revision: &str,
        brep_bytes: &[u8],
        history_event: &HistoryEvent,
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(
                entries,
                Some(expected_revision),
                Some((brep_feature_id, brep_bytes)),
                None,
                None,
                None,
                Some(history_event),
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_features_with_brep_if_revision_and_history_and_provenance(
        &self,
        entries: &[(&str, &str)],
        brep_feature_id: &str,
        expected_revision: &str,
        brep_bytes: &[u8],
        history_event: &HistoryEvent,
        request_id: &str,
        provenance: &str,
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(
                entries,
                Some(expected_revision),
                Some((brep_feature_id, brep_bytes)),
                None,
                Some(request_id),
                Some(provenance),
                Some(history_event),
            )
        })
    }

    /// Variant of BREP promotion that also authenticates the source BREP while
    /// holding the bundle write lock, immediately before staging the new
    /// generation.
    pub fn append_feature_with_brep_if_revision_and_source(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        expected_source_sha256: &str,
        brep_bytes: &[u8],
    ) -> Result<LoadedBundle, BundleError> {
        self.append_feature_with_brep_if_revision_and_source_and_idempotency(
            feature_id,
            kind,
            expected_revision,
            expected_source_sha256,
            None,
            brep_bytes,
        )
    }

    pub fn append_feature_with_brep_if_revision_and_source_and_idempotency(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        expected_source_sha256: &str,
        idempotency_key: Option<&str>,
        brep_bytes: &[u8],
    ) -> Result<LoadedBundle, BundleError> {
        self.append_feature_with_brep_if_revision_and_source_and_idempotency_payload(
            feature_id,
            kind,
            expected_revision,
            expected_source_sha256,
            idempotency_key,
            None,
            brep_bytes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_feature_with_brep_if_revision_and_source_and_idempotency_payload(
        &self,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        expected_source_sha256: &str,
        idempotency_key: Option<&str>,
        idempotency_payload: Option<&str>,
        brep_bytes: &[u8],
    ) -> Result<LoadedBundle, BundleError> {
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(
                &[(feature_id, kind)],
                Some(expected_revision),
                Some((feature_id, brep_bytes)),
                Some((feature_id, expected_source_sha256)),
                idempotency_key,
                idempotency_payload,
                None,
            )
        })
    }

    /// Look up a previously published transaction by its durable idempotency
    /// key while holding the same bundle lock used by promotion.
    pub fn find_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<LoadedBundle>, BundleError> {
        with_bundle_write_lock(&self.root, || {
            let loaded = self.open_locked()?;
            Ok(
                if loaded
                    .log
                    .entries()
                    .iter()
                    .any(|entry| entry.idempotency_key.as_deref() == Some(idempotency_key))
                {
                    Some(loaded)
                } else {
                    None
                },
            )
        })
    }

    /// Atomically append one or more `(feature_id, kind)` pairs to the
    /// bundle's Canonical Transaction Log and revision graph. The bundle
    /// is opened once, every entry is applied to the in-memory graph and
    /// log, the transactions log is rewritten once, the manifest is
    /// re-sealed once, and the bundle is reopened to return the
    /// post-write `LoadedBundle`. Either every entry is accepted or none
    /// is, so a crash between two writes cannot leave a half-bracket on
    /// disk.
    ///
    /// The rewritten log and re-sealed manifest are written into a fresh
    /// staging directory and published atomically, so a failed publication
    /// leaves the live bundle (and any preceding generation) byte-for-byte
    /// untouched on disk.
    pub fn append_features(&self, entries: &[(&str, &str)]) -> Result<LoadedBundle, BundleError> {
        if entries.is_empty() {
            return self.open();
        }
        // The read-modify-publish cycle is serialized per bundle root so
        // concurrent writers cannot diverge the Canonical Transaction Log:
        // every entry is staged from the same sealed base, so log positions
        // stay unique, predecessor digests chain, and no writer observes a
        // half-published rotation. The OS releases the lock if the holder
        // crashes, so an interrupted writer never leaves a stale lock.
        with_bundle_write_lock(&self.root, || {
            self.append_features_locked(entries, None, None, None, None, None, None)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn append_features_locked(
        &self,
        entries: &[(&str, &str)],
        expected_revision: Option<&str>,
        brep: Option<(&str, &[u8])>,
        source_brep: Option<(&str, &str)>,
        idempotency_key: Option<&str>,
        idempotency_payload: Option<&str>,
        history_event: Option<&HistoryEvent>,
    ) -> Result<LoadedBundle, BundleError> {
        self.append_features_locked_with_fit(
            entries,
            expected_revision,
            brep,
            source_brep,
            idempotency_key,
            idempotency_payload,
            history_event,
            None,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn append_features_locked_with_fit(
        &self,
        entries: &[(&str, &str)],
        expected_revision: Option<&str>,
        brep: Option<(&str, &[u8])>,
        source_brep: Option<(&str, &str)>,
        idempotency_key: Option<&str>,
        idempotency_payload: Option<&str>,
        history_event: Option<&HistoryEvent>,
        fit_dimension: Option<&FitDimension>,
        reject_existing_brep: bool,
    ) -> Result<LoadedBundle, BundleError> {
        // A save against a brand-new bundle path creates the sealed empty
        // generation first, so concurrent first saves serialize into one
        // bundle instead of racing a create against an append. The baseline
        // is staged and atomically promoted, so an interrupted first save
        // never leaves a partial canonical root.
        let mut loaded = if self.root.exists() || previous_generation_path(&self.root).exists() {
            self.open_locked()?
        } else {
            Self::create_staged(&self.root, EMPTY_LOG_DIGEST_HEX, "revision-0")?.open_locked()?
        };
        if let Some(idempotency_key) = idempotency_key
            && let Some(existing) = loaded
                .log
                .entries()
                .iter()
                .find(|entry| entry.idempotency_key.as_deref() == Some(idempotency_key))
        {
            let same_transaction = entries.len() == 1
                && existing.feature_id == entries[0].0
                && existing.kind == entries[0].1
                && existing.idempotency_payload.as_deref() == idempotency_payload;
            if same_transaction {
                return Ok(loaded);
            }
            return Err(BundleError::Invalid(format!(
                "idempotency key {idempotency_key:?} is already bound to another transaction"
            )));
        }
        if let Some(expected_revision) = expected_revision
            && loaded.revision_hash_hex() != expected_revision
        {
            return Err(BundleError::Invalid(format!(
                "worker result belongs to revision {expected_revision:?}, but current revision is {:?}",
                loaded.revision_hash_hex()
            )));
        }
        let allow_existing = (idempotency_key.is_some()
            && source_brep.is_some()
            && entries.len() == 1
            && entries[0].1.starts_with("bracket:"))
            || entries
                .iter()
                .all(|(_, kind)| kind.starts_with(SKETCH_COMMAND_KIND_PREFIX));
        validate_feature_entries(&loaded.graph, entries, allow_existing)?;
        if let Some((brep_feature_id, _)) = brep
            && !entries
                .iter()
                .any(|(feature_id, _)| *feature_id == brep_feature_id)
        {
            return Err(BundleError::Invalid(
                "BREP feature ID is not part of the transaction".to_string(),
            ));
        }
        if reject_existing_brep
            && let Some((feature_id, _)) = brep
            && loaded
                .graph
                .features()
                .any(|feature| feature.id.as_str() == feature_id)
        {
            return Err(BundleError::Invalid(format!(
                "feature ID {feature_id:?} already exists"
            )));
        }
        if let Some((feature_id, expected_sha256)) = source_brep {
            let source = self.root.join("brep").join(format!("{feature_id}.brep"));
            let actual = fs::read(&source)
                .map(|bytes| hash(&bytes))
                .map_err(|error| {
                    BundleError::Invalid(format!("source BREP could not be read: {error}"))
                })?;
            if actual != expected_sha256 {
                return Err(BundleError::Invalid(format!(
                    "source BREP digest mismatch for {feature_id}: expected {expected_sha256}, actual {actual}"
                )));
            }
        }
        for (feature_id, kind) in entries {
            let sketch_payload = kind
                .strip_prefix(SKETCH_COMMAND_KIND_PREFIX)
                .map(|payload| {
                    serde_json::from_str::<threeterm_domain::SketchPayload>(payload).map_err(
                        |error| BundleError::Invalid(format!("invalid sketch payload: {error}")),
                    )
                })
                .transpose()?;
            let feature_kind = if sketch_payload.is_some() {
                "sketch"
            } else {
                *kind
            };
            let feature = Feature::new(*feature_id, feature_kind)
                .map_err(|error| BundleError::Invalid(error.to_string()))?;
            let graph_changed = if let Some(payload) = sketch_payload {
                loaded
                    .graph
                    .add_sketch(feature, payload)
                    .map_err(BundleError::Invalid)?
            } else {
                loaded.graph.add_feature(feature)
            };
            if graph_changed {
                if let Some(idempotency_key) = idempotency_key {
                    if let Some((brep_feature_id, brep_bytes)) = brep
                        && brep_feature_id == *feature_id
                    {
                        loaded.log.append_feature_with_brep(
                            feature_id,
                            kind,
                            brep_bytes,
                            Some(idempotency_key),
                            idempotency_payload,
                        );
                    } else {
                        loaded.log.append_feature_with_idempotency(
                            feature_id,
                            kind,
                            idempotency_key,
                            idempotency_payload,
                        );
                    }
                } else if let Some((brep_feature_id, brep_bytes)) = brep
                    && brep_feature_id == *feature_id
                {
                    loaded
                        .log
                        .append_feature_with_brep(feature_id, kind, brep_bytes, None, None);
                } else {
                    loaded.log.append_feature(feature_id, kind);
                }
            }
        }

        if let Some(fit) = fit_dimension
            && loaded
                .graph
                .add_fit_dimension(fit.clone())
                .map_err(BundleError::Invalid)?
        {
            let payload = serde_json::to_string(fit)
                .map_err(|error| BundleError::Invalid(error.to_string()))?;
            loaded.log.append_feature(
                &format!("fit-dimension-{}", loaded.log.len()),
                &format!("{FIT_DIMENSION_KIND_PREFIX}{payload}"),
            );
        }

        if let Some(event) = history_event {
            let rebased_event = if event.ordinal == loaded.history.event_ordinal() + 1 {
                event.clone()
            } else if let HistoryOperation::InitializeLBracket {
                bracket_id,
                length,
                width,
                height,
                thickness,
            } = &event.operation
            {
                loaded
                    .history
                    .initialize_l_bracket(bracket_id, *length, *width, *height, *thickness)
                    .map_err(|error| BundleError::Invalid(error.to_string()))?
            } else {
                return Err(BundleError::Invalid(format!(
                    "stale history event ordinal {}, current ordinal {}",
                    event.ordinal,
                    loaded.history.event_ordinal()
                )));
            };
            loaded
                .history
                .apply_event(&rebased_event)
                .map_err(|error| BundleError::Invalid(error.to_string()))?;
            let payload = serde_json::to_string(&rebased_event)
                .map_err(|error| BundleError::Invalid(error.to_string()))?;
            let feature_id = format!("history-event-{}", loaded.log.len());
            loaded.log.append_feature(
                &feature_id,
                &format!("{HISTORY_EVENT_KIND_PREFIX}{payload}"),
            );
        }

        let mut encoded = Vec::new();
        for entry in loaded.log.entries() {
            let mut line = serde_json::to_vec(entry)
                .map_err(|error| BundleError::Invalid(error.to_string()))?;
            line.push(b'\n');
            encoded.extend_from_slice(&line);
        }

        loaded.manifest = Manifest::seal(
            &loaded.manifest.generation_id,
            &loaded.manifest.revision_id,
            &loaded.log,
            &loaded.graph,
        );
        let staging = fresh_staging_path_for_publish(&self.root);
        let source = if self.root.exists() {
            &self.root
        } else {
            &previous_generation_path(&self.root)
        };
        let prepared = (|| {
            copy_dir_recursive(source, &staging)?;
            fail_if_injected(PublicationFailurePoint::StagedFiles)?;
            terminate_at_requested_publication_point(PublicationKillPoint::StagedFiles);
            atomic_write(
                &staging.join(TRANSACTIONS_LOG_FILENAME),
                &encoded,
                Some(PublicationFailurePoint::LogSync),
            )?;
            terminate_at_requested_publication_point(PublicationKillPoint::LogSeal);
            atomic_write(
                &staging.join(MANIFEST_FILENAME),
                &serde_json::to_vec_pretty(&loaded.manifest)
                    .map_err(|error| BundleError::Invalid(error.to_string()))?,
                Some(PublicationFailurePoint::ManifestSync),
            )?;
            if let Some((feature_id, brep_bytes)) = brep {
                let brep_dir = staging.join("brep");
                fs::create_dir_all(&brep_dir)?;
                atomic_write_with_points(
                    &brep_dir.join(format!("{feature_id}.brep")),
                    brep_bytes,
                    None,
                    Some(PublicationFailurePoint::BrepCopy),
                    Some(PublicationFailurePoint::BrepWrite),
                    Some(PublicationFailurePoint::BrepSync),
                    Some(PublicationFailurePoint::BrepRename),
                    Some(PublicationFailurePoint::BrepDirectorySync),
                )?;
            }
            terminate_at_requested_publication_point(PublicationKillPoint::ManifestSeal);
            sync_directory(&staging, PublicationFailurePoint::StagingSync)?;
            Bundle::at(&staging).open_sealed(false)?;
            Ok::<(), BundleError>(())
        })();
        if let Err(error) = prepared {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        terminate_at_requested_publication_point(PublicationKillPoint::StagingValidation);
        publish_staged(&staging, &self.root).map_err(|error| match error {
            PublicationError::Io(error) => BundleError::Io(error.to_string()),
            PublicationError::Unknown(error) => BundleError::PublicationUnknown(error.to_string()),
        })?;
        self.open_locked()
            .map_err(|error| BundleError::PublicationUnknown(error.to_string()))
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
    // The Project Generation identity is the canonical log digest; for
    // a fresh bundle the chain is empty, so the seal writes the empty
    // log identity. The `generation.id` parameter is ignored for the
    // identity invariant.
    let bundle = Bundle::create_with_revision(path, EMPTY_LOG_DIGEST_HEX, &revision.id)?;
    Ok(bundle.open()?.manifest)
}

pub fn load(path: &Path) -> Result<LoadedBundle, BundleError> {
    load_with_policy(path, LoadPolicy::MigrateV0)
}

/// Load a bundle with an explicit migration policy.
pub fn load_with_policy(path: &Path, policy: LoadPolicy) -> Result<LoadedBundle, BundleError> {
    // The whole existence and schema classification runs under the per-root
    // write lock. A publisher renames the canonical root into the previous
    // slot mid-publication and promotes staging afterwards, so an unlocked
    // preflight could observe a missing manifest exactly between those two
    // renames and fail a load that simply waited for the writer. Serializing
    // classification with publications makes every load linearize against
    // them. The root is canonicalized first so a symlink alias and the
    // target share one lock and one set of recovery siblings.
    let root = canonical_bundle_root(path);
    with_bundle_write_lock(&root, || load_unlocked(&root, policy))
}

/// The lock-free body of `load`. Callers must hold the per-root write lock
/// whenever the bundle may need mutation (migration or recovery); the
/// classification is re-made here so a lock holder always acts on the
/// current on-disk state.
fn load_unlocked(path: &Path, policy: LoadPolicy) -> Result<LoadedBundle, BundleError> {
    let root = path;
    if !root.exists() {
        let previous = previous_generation_path(root);
        if previous.exists() {
            return match detect_schema(&previous)? {
                SchemaStatus::Current => Bundle::at(&previous).open_sealed(true),
                SchemaStatus::Prior => {
                    if policy == LoadPolicy::RejectV0RequiresBackup {
                        return Err(BundleError::SchemaEpochV0RequiresBackup);
                    }
                    fs::rename(&previous, root)?;
                    load_v0_with_migration(root, true)
                }
                SchemaStatus::Unknown => Err(BundleError::SchemaUnknown {
                    found: read_schema_version_raw(&previous).unwrap_or_default(),
                    expected_current: schema_epoch(),
                    expected_prior: prior_schema_epoch(),
                }),
            };
        }
        return Err(BundleError::BundlePathMissing {
            path: root.to_path_buf(),
        });
    }
    if !root.is_dir() {
        return Err(BundleError::Invalid(format!(
            "bundle path is not a directory: {}",
            root.display()
        )));
    }
    let status = detect_schema(root)?;
    match status {
        SchemaStatus::Current => Bundle::at(root).open_locked(),
        SchemaStatus::Prior => {
            if policy == LoadPolicy::RejectV0RequiresBackup {
                Err(BundleError::SchemaEpochV0RequiresBackup)
            } else {
                load_v0_with_migration(root, false)
            }
        }
        SchemaStatus::Unknown => Err(BundleError::SchemaUnknown {
            found: read_schema_version_raw(root).unwrap_or_default(),
            expected_current: schema_epoch(),
            expected_prior: prior_schema_epoch(),
        }),
    }
}

fn read_schema_version_raw(path: &Path) -> Option<String> {
    let raw = fs::read(path.join(MANIFEST_FILENAME)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value.get("schema_version")?.as_str().map(str::to_string)
}

fn validate_feature_entries(
    graph: &FeatureGraph,
    entries: &[(&str, &str)],
    allow_existing: bool,
) -> Result<(), BundleError> {
    let mut ids = std::collections::BTreeSet::new();
    for (feature_id, kind) in entries {
        Feature::new(*feature_id, *kind)
            .map_err(|error| BundleError::Invalid(error.to_string()))?;
        if !ids.insert(*feature_id) {
            return Err(BundleError::Invalid(format!(
                "feature ID {feature_id:?} is duplicated in one transaction"
            )));
        }
        if let Some(existing) = graph
            .features()
            .find(|feature| feature.id.as_str() == *feature_id)
        {
            let effective_kind = if kind.starts_with(SKETCH_COMMAND_KIND_PREFIX) {
                "sketch"
            } else {
                *kind
            };
            if !allow_existing
                || (!kind.starts_with(SKETCH_COMMAND_KIND_PREFIX)
                    && existing.kind == effective_kind)
            {
                return Err(BundleError::Invalid(format!(
                    "feature ID {feature_id:?} already exists"
                )));
            }
        }
    }
    Ok(())
}

fn verify_brep_provenance(root: &Path, log: &TransactionLog) -> Result<(), BundleError> {
    let mut latest = std::collections::BTreeMap::new();
    for entry in log.entries() {
        if entry.brep_path.is_some()
            || entry.brep_byte_count.is_some()
            || entry.brep_sha256.is_some()
        {
            latest.insert(entry.feature_id.as_str(), entry);
        }
    }
    for entry in latest.into_values() {
        let Some(path) = entry.brep_path.as_deref() else {
            return Err(BundleError::LogBrokenLink {
                log_index: entry.log_index,
                detail: "BREP transaction is missing its path".to_string(),
            });
        };
        let Some(expected_bytes) = entry.brep_byte_count else {
            return Err(BundleError::LogBrokenLink {
                log_index: entry.log_index,
                detail: "BREP path is missing its byte count".to_string(),
            });
        };
        let Some(expected_sha256) = entry.brep_sha256.as_deref() else {
            return Err(BundleError::LogBrokenLink {
                log_index: entry.log_index,
                detail: "BREP path is missing its digest".to_string(),
            });
        };
        let expected_path = format!("brep/{}.brep", entry.feature_id);
        if path != expected_path {
            return Err(BundleError::LogBrokenLink {
                log_index: entry.log_index,
                detail: "BREP path does not match its feature ID".to_string(),
            });
        }
        let file_name = Path::new(path)
            .file_name()
            .expect("validated BREP path has a file name");
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        const O_DIRECTORY: i32 = 0o200000;
        const O_NOFOLLOW: i32 = 0o400000;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(O_DIRECTORY | O_NOFOLLOW)
            .open(root.join("brep"))
            .map_err(|error| {
                BundleError::Invalid(format!(
                    "promoted BREP directory could not be read: {error}"
                ))
            })?;
        // Read through the opened directory handle so replacing `brep/` after
        // this point cannot redirect verification outside the bundle.
        let path =
            PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(file_name);
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(&path)
            .map_err(|error| {
                BundleError::Invalid(format!(
                    "promoted BREP for {} could not be read: {error}",
                    entry.feature_id
                ))
            })?;
        let metadata = file.metadata().map_err(|error| {
            BundleError::Invalid(format!(
                "promoted BREP for {} could not be read: {error}",
                entry.feature_id
            ))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(BundleError::Invalid(format!(
                "promoted BREP for {} is not a regular file",
                entry.feature_id
            )));
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|error| {
            BundleError::Invalid(format!(
                "promoted BREP for {} could not be read: {error}",
                entry.feature_id
            ))
        })?;
        let actual_sha256 = hash(&bytes);
        if bytes.len() as u64 != expected_bytes || actual_sha256 != expected_sha256 {
            return Err(BundleError::Invalid(format!(
                "promoted BREP for {} failed integrity verification",
                entry.feature_id
            )));
        }
    }
    Ok(())
}

/// Orchestrated prior-epoch load: sealed backup → deterministic migration →
/// validated staging → atomic publish. On any failure the source directory
/// stays byte-for-byte unchanged and no partial target is published.
fn load_v0_with_migration(
    path: &Path,
    recovered_from_previous: bool,
) -> Result<LoadedBundle, BundleError> {
    let v0 = read_v0(path).map_err(|error| BundleError::Migration {
        source: Box::new(error),
    })?;
    let (manifest, generation) = migrate_v0_to_v1(&v0);
    let transactions = v0.transactions.clone();

    let backup_path = backup_path_for(path);
    let backup_created =
        publish_sealed_backup(path, &backup_path).map_err(|source| BundleError::Backup {
            path: backup_path.clone(),
            source,
        })?;

    let staging = fresh_staging_path_for_migration(path);
    if let Err(error) = write_v1_into(&staging, &manifest, transactions.as_bytes()) {
        let _ = fs::remove_dir_all(&staging);
        // Only artifacts this attempt created are discarded. An
        // authenticated backup retained from an earlier attempt is a valid
        // recovery copy and must survive a retry that fails later.
        if backup_created {
            let _ = fs::remove_dir_all(&backup_path);
        }
        return Err(BundleError::Migration {
            source: Box::new(error),
        });
    }

    let validate = Bundle::at(&staging).open_locked();
    let validated = match validate {
        Ok(loaded) => loaded,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            if backup_created {
                let _ = fs::remove_dir_all(&backup_path);
            }
            return Err(BundleError::Migration {
                source: Box::new(error),
            });
        }
    };

    if let Err(error) = publish_staged(&staging, path) {
        // A validated staging directory is a sealed recovery candidate. Keep
        // it for diagnostics and a later retry rather than discarding data.
        return Err(BundleError::Io(match error {
            PublicationError::Io(error) | PublicationError::Unknown(error) => error.to_string(),
        }));
    }

    Ok(loaded_with(
        validated,
        path.to_path_buf(),
        manifest,
        generation,
        transactions,
        recovered_from_previous,
    ))
}

fn loaded_with(
    stale: LoadedBundle,
    canonical_root: PathBuf,
    manifest: Manifest,
    generation: ProjectGeneration,
    transactions: String,
    recovered_from_previous: bool,
) -> LoadedBundle {
    LoadedBundle {
        canonical_root,
        manifest,
        generation,
        transactions,
        log: stale.log,
        graph: stale.graph,
        components: stale.components,
        history: stale.history,
        history_events: stale.history_events,
        recovered_from_previous: stale.recovered_from_previous || recovered_from_previous,
    }
}

fn replay_history_events(log: &TransactionLog) -> Result<HistoryState, BundleError> {
    let mut state = HistoryState::default();
    for entry in log.entries() {
        let Some(payload) = entry.kind.strip_prefix(HISTORY_EVENT_KIND_PREFIX) else {
            continue;
        };
        let event: HistoryEvent =
            serde_json::from_str(payload).map_err(|error| BundleError::LogBrokenLink {
                log_index: entry.log_index,
                detail: format!("invalid history event: {error}"),
            })?;
        state
            .apply_event(&event)
            .map_err(|error| BundleError::LogBrokenLink {
                log_index: entry.log_index,
                detail: error.to_string(),
            })?;
    }
    Ok(state)
}

enum PublicationError {
    Io(std::io::Error),
    Unknown(std::io::Error),
}

impl From<std::io::Error> for PublicationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn publish_staged(staging: &Path, destination: &Path) -> Result<(), PublicationError> {
    let previous = previous_generation_path(destination);
    if !destination.exists() {
        rename_generation(
            staging,
            destination,
            PublicationFailurePoint::PromoteStaging,
        )?;
        sync_parent_directory(destination, PublicationFailurePoint::ParentSync)
            .map_err(PublicationError::Unknown)?;
        return Ok(());
    }
    let retired = retired_generation_path(&previous);
    // An interrupted cleanup leaves a generation that is older than the two
    // recovery generations. Reconcile it before rotating again so a retry is
    // not blocked by the deterministic retired path.
    remove_retired_generation(&retired)?;
    if previous.exists() {
        rename_generation(&previous, &retired, PublicationFailurePoint::RetirePrevious)?;
    }
    if let Err(error) = rename_generation(
        destination,
        &previous,
        PublicationFailurePoint::ReplaceCurrent,
    ) {
        if retired.exists() {
            let _ = rename_generation(&retired, &previous, PublicationFailurePoint::ReplaceCurrent);
        }
        return Err(PublicationError::Io(error));
    }
    // The previous generation is deliberately left in place. `Bundle::open`
    // recognizes an interrupted replacement and opens it explicitly.
    if let Err(error) = rename_generation(
        staging,
        destination,
        PublicationFailurePoint::PromoteStaging,
    ) {
        let _ = rename_generation(
            &previous,
            destination,
            PublicationFailurePoint::PromoteStaging,
        );
        if retired.exists() {
            let _ = rename_generation(&retired, &previous, PublicationFailurePoint::RetirePrevious);
        }
        return Err(PublicationError::Io(error));
    }
    // This is the generation older than the retained predecessor. It is no
    // longer part of recovery, so its cleanup must not block a later publish
    // if the post-promotion durability sync reports an error.
    // Cleanup is outside the two-generation publication boundary. A failed
    // cleanup is retained for the next publication to reconcile.
    let _ = remove_retired_generation(&retired);
    sync_parent_directory(destination, PublicationFailurePoint::ParentSync)
        .map_err(PublicationError::Unknown)?;
    Ok(())
}

/// Sync the directory containing `path` after a generation rename.
///
/// A bare relative destination such as `"project"` has an empty `parent()`
/// component, which `File::open` cannot open; the containing directory is
/// the current working directory in that case.
fn sync_parent_directory(path: &Path, point: PublicationFailurePoint) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => sync_directory(Path::new("."), point),
        Some(parent) => sync_directory(parent, point),
        None => Ok(()),
    }
}

fn rename_generation(
    source: &Path,
    destination: &Path,
    point: PublicationFailurePoint,
) -> std::io::Result<()> {
    PublicationFilesystem::rename(source, destination, point)
}

fn sync_directory(path: &Path, point: PublicationFailurePoint) -> std::io::Result<()> {
    PublicationFilesystem::sync_directory(path, point)
}

/// The one filesystem-operation seam used by durable generation publication.
/// Tests inject errors here, at the operation being modeled, rather than in
/// the publisher's control flow.
struct PublicationFilesystem;

impl PublicationFilesystem {
    fn rename(
        source: &Path,
        destination: &Path,
        point: PublicationFailurePoint,
    ) -> std::io::Result<()> {
        fail_if_injected(point)?;
        fs::rename(source, destination)?;
        terminate_at_requested_publication_point(point.kill_point());
        Ok(())
    }

    fn sync_directory(path: &Path, point: PublicationFailurePoint) -> std::io::Result<()> {
        fail_if_injected(point)?;
        File::open(path)?.sync_all()?;
        terminate_at_requested_publication_point(point.kill_point());
        Ok(())
    }
}

fn remove_retired_generation(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        fail_if_injected(PublicationFailurePoint::RetiredCleanup)?;
        fs::remove_dir_all(path)?;
        terminate_at_requested_publication_point(PublicationKillPoint::RetiredCleanup);
    }
    Ok(())
}

/// Copy `source` into a sealed backup at `backup`, staging the copy so an
/// interrupted copy can never leave a partial backup that a later migration
/// would accept as complete.
///
/// A pre-existing backup is retained only when it is a real directory that
/// authenticates as a v0 bundle belonging to this source; a symlink, a
/// non-directory, a partial copy, or a foreign generation is replaced from
/// a fresh staged copy. The staged copy is fully synced (files, directory,
/// and containing directory) and validated before it is renamed into place.
/// Returns whether this call created or replaced the backup, so a failed
/// migration retry can discard only the artifacts it produced and never an
/// authenticated recovery copy from an earlier attempt.
fn publish_sealed_backup(source: &Path, backup: &Path) -> std::io::Result<bool> {
    if let Ok(metadata) = fs::symlink_metadata(backup) {
        // `symlink_metadata` does not follow the entry: a symlinked backup
        // slot must never be treated as the recovery copy, and it must not
        // be written through.
        let authentic_source_copy = !metadata.file_type().is_symlink()
            && metadata.is_dir()
            && match (read_v0(backup), read_v0(source)) {
                (Ok(backup_v0), Ok(source_v0)) => {
                    backup_v0.manifest.generation_id == source_v0.manifest.generation_id
                        && backup_v0.manifest.revision_id == source_v0.manifest.revision_id
                }
                _ => false,
            };
        if authentic_source_copy {
            return Ok(false);
        }
        // `remove_dir_all` removes a symlink entry itself; it never writes
        // through it.
        fs::remove_dir_all(backup)?;
    }
    let staging = fresh_backup_staging_path(backup);
    let _ = fs::remove_dir_all(&staging);
    copy_dir_recursive(source, &staging)?;
    if read_v0(&staging).is_err() {
        let _ = fs::remove_dir_all(&staging);
        return Err(std::io::Error::other(
            "staged migration backup does not authenticate",
        ));
    }
    File::open(&staging)?.sync_all()?;
    if let Some(parent) = backup
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        File::open(parent)?.sync_all()?;
    }
    fs::rename(&staging, backup)?;
    if let Some(parent) = backup
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        File::open(parent)?.sync_all()?;
    }
    Ok(true)
}
fn fresh_backup_staging_path(backup: &Path) -> PathBuf {
    sibling_path_with_suffix(backup, &format!(".backup-tmp-{}", std::process::id()))
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
            // A subdivided generation directory (e.g. `brep/`) carries durable
            // file contents once its files are synced, but its directory entry
            // is only durable once the directory itself is synced. Sync it so a
            // sealed staging generation never loses nested artifact entries.
            File::open(&target)?.sync_all()?;
        } else if file_type.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to copy symlink: {}", entry.path().display()),
            ));
        } else {
            fs::copy(entry.path(), &target)?;
            // A generation is sealed only after every copied artifact has
            // reached the staging filesystem, not merely its directory.
            File::open(&target)?.sync_all()?;
        }
    }
    Ok(())
}

fn staging_path_for_migration(path: &Path) -> PathBuf {
    sibling_path_with_suffix(path, &format!(".migrate-tmp-{}", std::process::id()))
}

/// Select an absent migration staging candidate.
///
/// A pre-existing entry at the deterministic candidate — a stale directory
/// from an interrupted migration or a planted symlink — is skipped rather
/// than reused: `write_v1_into` would otherwise follow a symlink and the
/// final publish would rename the symlink itself into the canonical root.
fn fresh_staging_path_for_migration(path: &Path) -> PathBuf {
    let base = staging_path_for_migration(path);
    if !path_entry_exists(&base) {
        return base;
    }
    loop {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = sibling_path_with_suffix(
            path,
            &format!(".migrate-tmp-{}-{sequence}", std::process::id()),
        );
        if !path_entry_exists(&candidate) {
            return candidate;
        }
    }
}

/// Whether any filesystem entry (including a dangling symlink) exists at
/// `path`.
fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn staging_path_for_publish(path: &Path) -> PathBuf {
    sibling_path_with_suffix(path, &format!(".publish-tmp-{}", std::process::id()))
}

fn fresh_staging_path_for_publish(path: &Path) -> PathBuf {
    let staging = staging_path_for_publish(path);
    if !path_entry_exists(&staging) {
        return staging;
    }
    // A process restart can leave both the PID-based candidate and earlier
    // sequence candidates on disk, and a dangling symlink at a candidate
    // occupies its entry even though it points nowhere. Keep advancing
    // until an actually absent entry is found, so an interrupted save can
    // never block the next one on a stale path.
    loop {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let fresh = sibling_path_with_suffix(
            path,
            &format!(".publish-tmp-{}-{sequence}", std::process::id()),
        );
        if !path_entry_exists(&fresh) {
            return fresh;
        }
    }
}

/// The sibling path that retains the immediately preceding valid generation
/// of the bundle at `path`.
///
/// Built from the raw `OsStr` bytes so a non-UTF-8 bundle name still gets a
/// real, distinct sibling slot.
pub fn previous_generation_path(path: &Path) -> PathBuf {
    sibling_path_with_suffix(path, PREVIOUS_GENERATION_SUFFIX)
}

/// Resolve `path` to one canonical identity for locking and recovery.
///
/// An existing root is canonicalized so symlink aliases of the same bundle
/// share one lock and one set of sibling slots. A missing root cannot be
/// canonicalized; its parent is canonicalized instead and the file name is
/// re-joined, which converges with the full canonicalization once the root
/// exists (unless the root name itself is a dangling symlink).
fn canonical_bundle_root(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        && let Some(file_name) = path.file_name()
        && let Ok(canonical_parent) = fs::canonicalize(parent)
    {
        return canonical_parent.join(file_name);
    }
    path.to_path_buf()
}

/// Append `suffix` to `path`'s file name, preserving the raw `OsStr` bytes.
///
/// Deriving sibling names through `to_string_lossy()` would map a non-UTF-8
/// bundle name and a UTF-8 name containing U+FFFD onto the same sibling
/// path, so two distinct roots could share a staging, previous, retired, or
/// backup slot and race each other.
fn sibling_path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut sibling = path.to_path_buf();
    if let Some(file_name) = path.file_name() {
        let mut name = file_name.to_os_string();
        name.push(suffix);
        sibling.set_file_name(name);
    }
    sibling
}

/// Serialize a mutation of `root` against every other writer of the same
/// bundle, in this process and across processes.
///
/// The lock identity is the bundle's containing directory, opened and
/// flocked as a directory descriptor. Generation rotation renames only the
/// bundle root and its siblings, never the containing directory, so the
/// locked identity cannot be replaced while it is held: there is no
/// pathname window in which a second writer could lock a different inode.
/// Aliases of one bundle resolve to the same canonical directory through
/// `canonical_bundle_root`, and writers of different bundles in the same
/// directory serialize on the same lock. If the holder dies, the OS
/// releases the lock, so a crashed writer cannot strand subsequent saves.
///
/// Pure reads (`open_sealed`) stay lock-free by design. `Bundle::open`
/// takes the same lock because its interrupted-rotation reconciliation
/// mutates the rotation slots and must never interleave with a publisher's
/// `previous → retired → destination → previous → staging → destination`
/// sequence.
fn with_bundle_write_lock<T>(
    root: &Path,
    operation: impl FnOnce() -> Result<T, BundleError>,
) -> Result<T, BundleError> {
    let lock_directory = lock_directory_for(root)?;
    let lock_descriptor = File::open(&lock_directory)?;
    lock_descriptor.lock().map_err(|error| {
        BundleError::Io(format!(
            "failed to lock {}: {error}",
            lock_directory.display()
        ))
    })?;
    let result = operation();
    let _ = lock_descriptor.unlock();
    result
}

/// The canonical containing directory whose descriptor is the per-bundle
/// write lock.
///
/// An empty parent (a bare relative root) resolves to the current working
/// directory, so every writer of that bundle locks the same inode.
fn lock_directory_for(root: &Path) -> Result<PathBuf, BundleError> {
    match root.parent() {
        Some(parent) if parent.as_os_str().is_empty() => {
            Ok(fs::canonicalize(".").map_err(BundleError::from)?)
        }
        Some(parent) => {
            fs::create_dir_all(parent)?;
            Ok(fs::canonicalize(parent).map_err(BundleError::from)?)
        }
        None => Ok(fs::canonicalize(".").map_err(BundleError::from)?),
    }
}

fn retired_generation_path(path: &Path) -> PathBuf {
    sibling_path_with_suffix(path, ".retired-generation")
}

fn reconcile_interrupted_rotation(destination: &Path) -> Result<(), BundleError> {
    let previous = previous_generation_path(destination);
    let retired = retired_generation_path(&previous);
    if destination.exists() && !previous.exists() && retired.exists() {
        // A crash after retiring the predecessor has not changed the selected
        // canonical generation. Restore the recognized previous slot before
        // allowing either reads or another publication to proceed.
        Bundle::at(&retired).open_sealed(false)?;
        match fs::rename(&retired, previous) {
            Ok(()) => Ok(()),
            // A concurrent reconciler restored the slot first, or a
            // concurrent publisher drained the retired generation; the
            // selected generation is unchanged, so the restore is
            // idempotent.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    } else {
        Ok(())
    }
}

fn write_v1_into(
    staging: &Path,
    manifest: &Manifest,
    transactions: &[u8],
) -> Result<(), BundleError> {
    fs::create_dir_all(staging)?;
    atomic_write(
        &staging.join(TRANSACTIONS_LOG_FILENAME),
        transactions,
        Some(PublicationFailurePoint::LogSync),
    )?;
    atomic_write(
        &staging.join(MANIFEST_FILENAME),
        &serde_json::to_vec_pretty(manifest)?,
        Some(PublicationFailurePoint::ManifestSync),
    )?;
    sync_directory(staging, PublicationFailurePoint::StagingSync)?;
    Ok(())
}

fn backup_path_for(path: &Path) -> PathBuf {
    let name = path.file_name().map(|name| {
        let mut name = name.to_os_string();
        name.push(PRE_MIGRATION_BACKUP_SUFFIX);
        name
    });
    match (path.parent(), name) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => parent.join(name),
        (_, Some(name)) => PathBuf::from(name),
        (_, None) => PathBuf::from(PRE_MIGRATION_BACKUP_SUFFIX),
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
    let Some(file_name) = path.file_name() else {
        return false;
    };
    // Compare the suffix losslessly so a non-UTF-8 backup sibling is still
    // recognized and never treated as a migratable v0 source.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        file_name
            .as_bytes()
            .ends_with(PRE_MIGRATION_BACKUP_SUFFIX.as_bytes())
    }
    #[cfg(not(unix))]
    {
        file_name
            .to_string_lossy()
            .ends_with(PRE_MIGRATION_BACKUP_SUFFIX)
    }
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
    sibling_path_with_suffix(path, &format!(".tmp-{}", std::process::id()))
}

fn read_required(path: &Path, missing: BundleError) -> Result<Vec<u8>, BundleError> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(missing),
        Err(error) => Err(error.into()),
    }
}

fn atomic_write(
    path: &Path,
    bytes: &[u8],
    sync_point: Option<PublicationFailurePoint>,
) -> Result<(), BundleError> {
    atomic_write_with_points(path, bytes, sync_point, None, None, None, None, None)
}

#[allow(clippy::too_many_arguments)]
fn atomic_write_with_points(
    path: &Path,
    bytes: &[u8],
    sync_point: Option<PublicationFailurePoint>,
    copy_point: Option<PublicationFailurePoint>,
    write_point: Option<PublicationFailurePoint>,
    file_sync_point: Option<PublicationFailurePoint>,
    rename_point: Option<PublicationFailurePoint>,
    directory_sync_point: Option<PublicationFailurePoint>,
) -> Result<(), BundleError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| BundleError::Invalid("target has no file name".to_string()))?;
    let temporary = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));
    let result = (|| {
        if let Some(point) = copy_point {
            fail_if_injected(point)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Some(point) = write_point {
            fail_if_injected(point)?;
        }
        file.write_all(bytes)?;
        if let Some(point) = sync_point.or(file_sync_point) {
            fail_if_injected(point)?;
        }
        file.sync_all()?;
        if let Some(point) = rename_point {
            fail_if_injected(point)?;
        }
        fs::rename(&temporary, path)?;
        if let Some(point) = directory_sync_point {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            sync_directory(parent, point)?;
        }
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(BundleError::from)
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
    fn duplicate_append_is_rejected_without_advancing_the_revision() {
        let root = temp_root("append");
        let bundle =
            Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        let saved = bundle
            .append_feature("box-1", "box")
            .expect("feature appends");
        assert_eq!(saved.log.len(), 1);

        let duplicate = bundle
            .append_feature("box-1", "box")
            .expect_err("duplicate feature IDs are rejected");
        assert!(matches!(duplicate, BundleError::Invalid(_)));

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
    fn revision_fenced_append_rejects_stale_workers_without_mutating_the_bundle() {
        let root = temp_root("revision-fence");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
        let current = bundle
            .append_feature("box-1", "box")
            .expect("first feature appends");
        let manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
        let log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");

        let error = bundle
            .append_feature_if_revision("box-2", "box", "stale-revision")
            .expect_err("stale worker must be rejected");
        assert!(
            error.to_string().contains("current revision"),
            "error must identify the revision mismatch: {error}"
        );
        assert_eq!(fs::read(root.join(MANIFEST_FILENAME)).unwrap(), manifest);
        assert_eq!(fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(), log);
        assert_eq!(
            bundle.open().unwrap().revision_hash_hex(),
            current.revision_hash_hex()
        );

        let updated = bundle
            .append_feature_if_revision("box-2", "box", current.revision_hash_hex())
            .expect("current worker revision appends");
        assert_eq!(updated.log.len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_generation_replacement_restores_prior_and_retains_candidate() {
        let root = temp_root("interrupted-publication");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
        bundle
            .append_feature("box-1", "box")
            .expect("first publish");
        let prior_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("prior manifest");
        let prior_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("prior log");

        fail_next_publication_at(PublicationFailurePoint::PromoteStaging);
        assert!(bundle.append_feature("box-2", "box").is_err());

        let recovered = bundle.open().expect("prior generation remains current");
        assert!(!recovered.recovered_from_previous);
        assert_eq!(recovered.log.len(), 1);
        let previous = previous_generation_path(&root);
        assert_eq!(
            fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
            prior_manifest
        );
        assert_eq!(
            fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
            prior_log
        );
        let candidate = staging_path_for_publish(&root);
        assert!(Bundle::at(&candidate).open_sealed(false).is_ok());

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(previous);
        let _ = fs::remove_dir_all(candidate);
    }

    #[test]
    fn publication_filesystem_failures_preserve_a_loadable_generation() {
        for point in [
            PublicationFailurePoint::StagingSync,
            PublicationFailurePoint::ReplaceCurrent,
            PublicationFailurePoint::PromoteStaging,
            PublicationFailurePoint::ParentSync,
        ] {
            let root = temp_root("publication-failure");
            let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
            bundle
                .append_feature("box-1", "box")
                .expect("first publish");
            let manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest");
            let log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log");

            fail_next_publication_at(point);
            assert!(bundle.append_feature("box-2", "box").is_err());
            if point == PublicationFailurePoint::ParentSync {
                // Parent sync happens after promotion. The new generation is
                // selected, while the predecessor remains byte-preserved.
                assert_eq!(bundle.open().unwrap().log.len(), 2);
                let previous = previous_generation_path(&root);
                assert_eq!(
                    fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
                    manifest
                );
                assert_eq!(
                    fs::read(previous.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
                    log
                );
            } else {
                assert_eq!(fs::read(root.join(MANIFEST_FILENAME)).unwrap(), manifest);
                assert_eq!(fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(), log);
                assert_eq!(bundle.open().unwrap().log.len(), 1);
            }

            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_dir_all(previous_generation_path(&root));
            let _ = fs::remove_dir_all(staging_path_for_publish(&root));
        }
    }

    #[test]
    fn failed_promotion_restores_the_current_and_preceding_generations() {
        let root = temp_root("promotion-rollback-generations");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
        bundle
            .append_feature("box-1", "box")
            .expect("first publish");
        let preceding_manifest =
            fs::read(root.join(MANIFEST_FILENAME)).expect("preceding manifest");
        let preceding_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("preceding log");
        bundle
            .append_feature("box-2", "box")
            .expect("second publish");
        let current_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("current manifest");
        let current_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("current log");

        fail_next_publication_at(PublicationFailurePoint::PromoteStaging);
        assert!(bundle.append_feature("box-3", "box").is_err());

        let previous = previous_generation_path(&root);
        assert_eq!(
            fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
            current_manifest
        );
        assert_eq!(
            fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
            current_log
        );
        assert_eq!(
            fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
            preceding_manifest
        );
        assert_eq!(
            fs::read(previous.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
            preceding_log
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(previous);
        let _ = fs::remove_dir_all(staging_path_for_publish(&root));
    }

    #[test]
    fn retire_previous_failure_preserves_the_current_and_preceding_generations() {
        let root = temp_root("retire-previous-failure");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
        bundle
            .append_feature("box-1", "box")
            .expect("first publish");
        let preceding_manifest =
            fs::read(root.join(MANIFEST_FILENAME)).expect("preceding manifest");
        let preceding_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("preceding log");
        bundle
            .append_feature("box-2", "box")
            .expect("second publish");
        let current_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("current manifest");
        let current_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("current log");

        fail_next_publication_at(PublicationFailurePoint::RetirePrevious);
        assert!(bundle.append_feature("box-3", "box").is_err());

        let previous = previous_generation_path(&root);
        assert!(
            !retired_generation_path(&previous).exists(),
            "retirement is not reached"
        );
        assert_eq!(
            fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
            current_manifest
        );
        assert_eq!(
            fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
            current_log
        );
        assert_eq!(
            fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
            preceding_manifest
        );
        assert_eq!(
            fs::read(previous.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
            preceding_log
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(previous);
        let _ = fs::remove_dir_all(staging_path_for_publish(&root));
    }

    #[test]
    fn interrupted_rotation_restores_the_recognized_previous_generation() {
        let root = temp_root("interrupted-rotation");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
        bundle
            .append_feature("box-1", "box")
            .expect("first publish");
        bundle
            .append_feature("box-2", "box")
            .expect("second publish");
        let previous = previous_generation_path(&root);
        let retired = retired_generation_path(&previous);
        let preceding_manifest = fs::read(previous.join(MANIFEST_FILENAME)).expect("manifest");
        fs::rename(&previous, &retired).expect("simulates interrupted rotation");

        let loaded = bundle.open().expect("canonical generation opens");
        assert_eq!(loaded.log.len(), 2);
        assert_eq!(
            fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
            preceding_manifest
        );
        assert!(!retired.exists());

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(previous);
    }

    #[test]
    fn parent_sync_failure_does_not_block_the_next_publication() {
        let root = temp_root("parent-sync-retry");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
        bundle
            .append_feature("box-1", "box")
            .expect("first publish");
        bundle
            .append_feature("box-2", "box")
            .expect("second publish");

        fail_next_publication_at(PublicationFailurePoint::ParentSync);
        assert!(bundle.append_feature("box-3", "box").is_err());

        bundle
            .append_feature("box-4", "box")
            .expect("next publication recovers");
        assert_eq!(bundle.open().unwrap().log.len(), 4);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(previous_generation_path(&root));
    }

    #[test]
    fn interrupted_retired_cleanup_is_reconciled_before_the_next_publication() {
        let root = temp_root("retired-cleanup-retry");
        let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("creates");
        bundle
            .append_feature("box-1", "box")
            .expect("first publish");
        bundle
            .append_feature("box-2", "box")
            .expect("second publish");

        fail_next_publication_at(PublicationFailurePoint::RetiredCleanup);
        bundle
            .append_feature("box-3", "box")
            .expect("cleanup failure does not invalidate publication");

        bundle
            .append_feature("box-4", "box")
            .expect("next publication reconciles stale retired generation");
        assert_eq!(bundle.open().unwrap().log.len(), 4);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(previous_generation_path(&root));
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

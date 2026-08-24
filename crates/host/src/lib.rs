use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use threeterm_domain::FeatureGraph;
use threeterm_occt_worker::{
    BooleanFuseRequest, BooleanFuseResult, BracketRequest, BracketResult, ChamferRequest,
    ChamferResult, CircularPatternRequest, CircularPatternResult, DraftRequest, DraftResult,
    ExtrudeRequest, ExtrudeResult, FilletRequest, FilletResult, HoleRequest, HoleResult,
    LinearPatternRequest, LinearPatternResult, LoftRequest, LoftResult, MirrorRequest,
    MirrorResult, OcctDiagnostic, OcctWorker, RevolveRequest, RevolveResult, ShellRequest,
    ShellResult, WorkerError,
};
use threeterm_persistence::{Bundle, BundleError, LoadedBundle, load, previous_generation_path};
use threeterm_protocol::artifact::{
    ArtifactError, Layer1ArtifactRequest, Layer1CacheKey, Stage, WorkerFingerprint,
};
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::supervisor::SupervisorOutcome;

pub const BREP_SUBDIR: &str = "brep";

/// Returns true if a Layer 1 artifact request must never be cached per the
/// exclusion policy. Mirrors `threeterm_viewport::ViewportDisplayCache::is_excluded`
/// but on the Host's Layer 1 Derived Result boundary.
/// Exclusions: Command Drafts, hover/pointer/candidate, stale last-valid
/// geometry, preview-only beyond session, worker internals (tmp/ / stderr).
pub fn is_layer1_excluded(request: &Layer1ArtifactRequest) -> bool {
    if request.source_revision_id.is_empty() {
        return true;
    }
    let fields = [
        &request.artifact_kind,
        &request.staging_name,
        &request.semantic_input_sha256,
        &request.deterministic_settings_sha256,
    ];
    for field in fields {
        let lower = field.to_ascii_lowercase();
        if lower.contains("draft")
            || lower.contains("hover")
            || lower.contains("candidate")
            || lower.contains("pointer")
            || lower.contains("stale")
            || lower.contains("preview-only")
            || lower.contains("worker-internal")
            || lower.contains("tmp/")
            || lower.contains("stderr")
        {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotView {
    pub feature_graph_hash: String,
    pub revision_hash: String,
    pub recovered_from_previous: bool,
}

impl From<&LoadedBundle> for SnapshotView {
    fn from(bundle: &LoadedBundle) -> Self {
        Self {
            feature_graph_hash: bundle.feature_graph_hash_hex().to_string(),
            revision_hash: bundle.revision_hash_hex().to_string(),
            recovered_from_previous: bundle.recovered_from_previous,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer1DerivedResult {
    pub request_id: String,
    pub source_revision_id: String,
    pub cache_key: Layer1CacheKey,
    pub worker_fingerprint: WorkerFingerprint,
    pub artifact_kind: String,
    pub artifact_name: String,
    pub byte_count: u64,
    pub sha256: String,
    pub path: PathBuf,
}

/// Immutable host-owned input for a disposable presentation.
///
/// The snapshot keeps the canonical revision and graph together so a
/// presentation adapter cannot accidentally pair data from two host reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationSnapshot {
    pub snapshot: SnapshotView,
    pub graph: FeatureGraph,
    pub layer1_results: Vec<Layer1DerivedResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrudeCommitView {
    pub snapshot: SnapshotView,
    pub result: ExtrudeResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BooleanFuseCommitView {
    pub snapshot: SnapshotView,
    pub result: BooleanFuseResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilletCommitView {
    pub snapshot: SnapshotView,
    pub result: FilletResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChamferCommitView {
    pub snapshot: SnapshotView,
    pub result: ChamferResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoleCommitView {
    pub snapshot: SnapshotView,
    pub result: HoleResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RevolveCommitView {
    pub snapshot: SnapshotView,
    pub result: RevolveResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirrorCommitView {
    pub snapshot: SnapshotView,
    pub result: MirrorResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinearPatternCommitView {
    pub snapshot: SnapshotView,
    pub result: LinearPatternResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CircularPatternCommitView {
    pub snapshot: SnapshotView,
    pub result: CircularPatternResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShellCommitView {
    pub snapshot: SnapshotView,
    pub result: ShellResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DraftCommitView {
    pub snapshot: SnapshotView,
    pub result: DraftResult,
}

/// A transient semantic command input bound to one canonical Revision Snapshot.
/// Drafts are host-session state: they are never written to the project bundle
/// and their worker output is always disposable until commit promotion.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandDraft {
    pub draft_id: String,
    pub bundle_root: PathBuf,
    pub source_feature_id: String,
    pub source_revision: String,
    pub source_brep_sha256: String,
    pub request: DraftRequest,
    preview_path: Option<PathBuf>,
    created_at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DraftPreviewView {
    pub draft_id: String,
    pub source_revision: String,
    pub preview_revision: String,
    pub input_fingerprint: String,
    pub result: DraftResult,
    pub brep_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BracketParameterDraft {
    pub draft_id: String,
    pub bundle_root: PathBuf,
    pub bracket_id: String,
    pub source_revision: String,
    pub source_brep_sha256: String,
    pub request: BracketRequest,
    pub sequence: u64,
    preview_path: Option<PathBuf>,
    created_at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BracketPreviewView {
    pub draft_id: String,
    pub source_revision: String,
    pub preview_revision: String,
    pub input_fingerprint: String,
    pub result: BracketResult,
    pub brep_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BracketCommitView {
    pub snapshot: SnapshotView,
    pub input_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoftCommitView {
    pub snapshot: SnapshotView,
    pub result: LoftResult,
}

#[derive(Debug)]
pub enum HostError {
    BundlePathMissing {
        path: PathBuf,
    },
    BundlePathNotDirectory {
        path: PathBuf,
    },
    Validation {
        detail: String,
    },
    Persistence(BundleError),
    WorkerFailure {
        request_id: Option<String>,
        detail: String,
    },
    WorkerUnavailable {
        detail: String,
    },
    UnsupportedGeometry {
        request_id: Option<String>,
        detail: String,
    },
    BrepInvalid {
        request_id: Option<String>,
        detail: String,
    },
    BrepFileMissing {
        path: PathBuf,
    },
    BrepIo {
        detail: String,
    },
    /// A supervised worker lifecycle ended without a typed result. The
    /// structured termination record is preserved so the diagnostic
    /// surface keeps the request id, stage, elapsed time, exit
    /// signal/code, last progress, artifact error, and stderr tail.
    WorkerTerminated {
        record: Box<threeterm_protocol::supervisor::TerminationRecord>,
    },
    DraftAlreadyExists {
        draft_id: String,
    },
    DraftNotFound {
        draft_id: String,
    },
    DraftStale {
        draft_id: String,
        source_revision: String,
        current_revision: String,
        recovery: &'static str,
    },
    DraftSourceChanged {
        draft_id: String,
        source_feature_id: String,
        recovery: &'static str,
    },
    DraftInvalid {
        draft_id: String,
        detail: String,
    },
    DraftSequenceConflict {
        draft_id: String,
        expected: u64,
        current: u64,
    },
    DraftUnknownOutcome {
        draft_id: String,
        source_revision: String,
        recovery: &'static str,
    },
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
            Self::WorkerFailure { detail, .. } => {
                write!(formatter, "occt worker failure: {detail}")
            }
            Self::WorkerUnavailable { detail } => {
                write!(formatter, "occt worker unavailable: {detail}")
            }
            Self::UnsupportedGeometry { detail, .. } => {
                write!(formatter, "occt unsupported geometry: {detail}")
            }
            Self::BrepInvalid { detail, .. } => {
                write!(formatter, "occt brep invalid: {detail}")
            }
            Self::WorkerTerminated { record } => {
                write!(
                    formatter,
                    "occt worker terminated: stage={} elapsed={:?} exit_signal={:?} exit_code={:?} request_id={}",
                    record.stage,
                    record.elapsed,
                    record.exit_signal,
                    record.exit_code,
                    record.request_id
                )
            }
            Self::BrepFileMissing { path } => {
                write!(formatter, "occt brep file missing: {}", path.display())
            }
            Self::BrepIo { detail } => {
                write!(formatter, "occt brep io error: {detail}")
            }
            Self::DraftAlreadyExists { draft_id } => {
                write!(formatter, "command draft already exists: {draft_id}")
            }
            Self::DraftNotFound { draft_id } => {
                write!(formatter, "command draft not found: {draft_id}")
            }
            Self::DraftStale {
                draft_id,
                source_revision,
                current_revision,
                recovery,
            } => write!(
                formatter,
                "command draft {draft_id} is stale: source_revision={source_revision} current_revision={current_revision} recovery={recovery}"
            ),
            Self::DraftSourceChanged {
                draft_id,
                source_feature_id,
                recovery,
            } => write!(
                formatter,
                "command draft {draft_id} source {source_feature_id} changed: recovery={recovery}"
            ),
            Self::DraftInvalid { draft_id, detail } => {
                write!(formatter, "command draft {draft_id} is invalid: {detail}")
            }
            Self::DraftSequenceConflict {
                draft_id,
                expected,
                current,
            } => write!(
                formatter,
                "command draft {draft_id} update conflicts: expected_sequence={expected} current_sequence={current}"
            ),
            Self::DraftUnknownOutcome {
                draft_id,
                source_revision,
                recovery,
            } => write!(
                formatter,
                "command draft {draft_id} has unknown publication outcome: source_revision={source_revision} recovery={recovery}"
            ),
        }
    }
}

impl std::error::Error for HostError {}

impl From<BundleError> for HostError {
    fn from(error: BundleError) -> Self {
        Self::Persistence(error)
    }
}

fn host_error_from_diagnostic(diagnostic: OcctDiagnostic, request_id: Option<String>) -> HostError {
    if diagnostic.code == "brep_invalid" {
        HostError::BrepInvalid {
            request_id,
            detail: format!("{} {}", diagnostic.code, diagnostic.arg),
        }
    } else if diagnostic.code == "unsupported_geometry" {
        HostError::UnsupportedGeometry {
            request_id,
            detail: diagnostic.arg,
        }
    } else {
        HostError::WorkerFailure {
            request_id,
            detail: format!("{} {}", diagnostic.code, diagnostic.arg),
        }
    }
}

impl From<WorkerError> for HostError {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::Diagnostic(diagnostic) => host_error_from_diagnostic(diagnostic, None),
            WorkerError::DiagnosticWithContext {
                request_id,
                diagnostic,
            } => host_error_from_diagnostic(diagnostic, Some(request_id)),
            WorkerError::NonZeroExitWithContext {
                request_id,
                code,
                stderr,
            } => Self::WorkerFailure {
                request_id: Some(request_id),
                detail: format!("worker exited with code {code:?}: {stderr}"),
            },
            WorkerError::SignalledWithContext {
                request_id,
                signal,
                stderr,
            } => Self::WorkerFailure {
                request_id: Some(request_id),
                detail: format!("worker signalled with {signal}: {stderr}"),
            },
            WorkerError::MalformedWithContext { request_id, detail } => Self::WorkerFailure {
                request_id: Some(request_id),
                detail,
            },
            WorkerError::Spawn {
                request_id, detail, ..
            } => Self::WorkerFailure { request_id, detail },
            WorkerError::Cancelled {
                request_id,
                last_progress,
                elapsed,
                stderr_tail,
                exit_signal,
                exit_code,
            } => Self::WorkerTerminated {
                record: Box::new(threeterm_protocol::supervisor::TerminationRecord {
                    request_id: request_id.clone(),
                    stage: "cancelled".to_string(),
                    elapsed,
                    last_progress,
                    last_artifact_error: None,
                    exit_signal,
                    exit_code,
                    stderr_tail,
                    failed_code: None,
                    failed_detail: None,
                    exit_kind: threeterm_protocol::supervisor::ExitKind::Cooperative,
                }),
            },
            WorkerError::Supervised { record } => Self::WorkerTerminated { record },
            other => Self::WorkerFailure {
                request_id: None,
                detail: other.to_string(),
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct Host {
    current: RefCell<Option<LoadedBundle>>,
    layer1_results: RefCell<HashMap<Layer1CacheKey, Layer1DerivedResult>>,
    drafts: RefCell<HashMap<String, CommandDraft>>,
    bracket_drafts: RefCell<HashMap<(PathBuf, String), BracketParameterDraft>>,
}

fn draft_map_key(root: &Path, draft_id: &str) -> (PathBuf, String) {
    (root.to_path_buf(), draft_id.to_string())
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
        let bundle = self.bundle_for_save(root)?;
        let loaded = match bundle.append_feature(feature_id, kind) {
            Ok(loaded) => loaded,
            Err(error) => {
                // Publication can promote before its final parent sync
                // reports an error. Re-open so in-memory state never lags
                // the selected generation on disk.
                if let Ok(loaded) = bundle.open() {
                    self.current.replace(Some(loaded));
                }
                return Err(error.into());
            }
        };
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
        let bundle = self.bundle_for_save(root)?;
        let vertical_id = format!("{bracket_id}-plate-vertical");
        let horizontal_id = format!("{bracket_id}-plate-horizontal");
        let entries = [
            (vertical_id.as_str(), "plate-vertical"),
            (horizontal_id.as_str(), "plate-horizontal"),
        ];
        let loaded = match bundle.append_features(&entries) {
            Ok(loaded) => loaded,
            Err(error) => {
                // Publication can promote before its final parent sync
                // reports an error. Re-open so in-memory state never lags
                // the selected generation on disk.
                if let Ok(loaded) = bundle.open() {
                    self.current.replace(Some(loaded));
                }
                return Err(error.into());
            }
        };
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    pub fn load(&self, root: impl AsRef<Path>) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        if root.exists() && !root.is_dir() {
            return Err(HostError::BundlePathNotDirectory {
                path: root.to_path_buf(),
            });
        }
        let loaded = match load(root) {
            Ok(loaded) => loaded,
            Err(BundleError::BundlePathMissing { .. }) => {
                // The missing-root classification is made under the
                // persistence lock, so a concurrent first save either
                // completes before the classification (and the load
                // succeeds) or is still in flight.
                return Err(HostError::BundlePathMissing {
                    path: root.to_path_buf(),
                });
            }
            Err(error) => {
                // Migration can promote before its final parent sync reports
                // an error. Re-open the selected generation before returning
                // so an existing Host never retains a stale snapshot.
                if let Ok(loaded) = Bundle::at(root).open() {
                    self.current.replace(Some(loaded));
                }
                return Err(error.into());
            }
        };
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    /// Runs `load` as a schema-migration preflight and reconciles `Host` state
    /// when migration promotes a generation before reporting a late parent-sync
    /// error. `save` reuses this so it never retains a stale snapshot.
    fn load_preflight(&self, root: &Path) -> Result<(), HostError> {
        if let Err(error) = load(root) {
            // Migration can promote before its final parent sync reports an
            // error. Re-open so in-memory state never lags the selected
            // generation on disk.
            if let Ok(loaded) = Bundle::at(root).open() {
                self.current.replace(Some(loaded));
            }
            return Err(error.into());
        }
        Ok(())
    }

    /// Resolve the bundle a save should append to.
    ///
    /// Every save path is migration-aware: when the bundle or its previous
    /// sibling exists, `load` runs first so a v0 source or a v0 previous
    /// generation is migrated before the append. The previous-sibling check
    /// uses the persistence-derived, lossless sibling path so a non-UTF-8
    /// bundle name is never missed. When nothing exists, the locked append
    /// path stages and atomically promotes the empty baseline itself, so a
    /// concurrent first save on the same missing root serializes instead of
    /// failing with "destination already exists".
    fn bundle_for_save(&self, root: &Path) -> Result<Bundle, HostError> {
        let bundle = Bundle::at(root);
        let canonical = bundle.canonical_root();
        if canonical.exists() {
            if !canonical.is_dir() {
                return Err(HostError::BundlePathNotDirectory {
                    path: root.to_path_buf(),
                });
            }
            self.load_preflight(root)?;
        } else if previous_generation_path(canonical).exists() {
            // A missing root can retain a v0 source after an interrupted
            // migration. Recover/migrate it before attempting an append.
            self.load_preflight(root)?;
        }
        Ok(bundle)
    }

    pub fn current(&self) -> Option<SnapshotView> {
        self.current.borrow().as_ref().map(SnapshotView::from)
    }

    /// Return a read-only copy of the canonical feature graph for presentation
    /// adapters. Transient UI navigation must never borrow mutable host state.
    pub fn current_graph(&self) -> Option<FeatureGraph> {
        self.current
            .borrow()
            .as_ref()
            .map(|loaded| loaded.graph.clone())
    }

    /// Capture one immutable presentation projection from the current
    /// canonical bundle. Derived results remain disposable metadata.
    pub fn presentation_snapshot(&self) -> Option<PresentationSnapshot> {
        let current = self.current.borrow();
        let loaded = current.as_ref()?;
        let mut layer1_results: Vec<_> = self
            .layer1_results
            .borrow()
            .values()
            .filter(|result| result.source_revision_id == loaded.manifest.revision_hash)
            .cloned()
            .collect();
        layer1_results.sort_by(|left, right| left.request_id.cmp(&right.request_id));
        Some(PresentationSnapshot {
            snapshot: SnapshotView::from(loaded),
            graph: loaded.graph.clone(),
            layer1_results,
        })
    }

    /// Open a transient command draft against the current canonical source.
    /// The caller supplies only the source feature identity; the canonical
    /// BREP path and source digest are derived by the host.
    pub fn open_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: impl Into<String>,
        source_feature_id: impl Into<String>,
        request: DraftRequest,
    ) -> Result<CommandDraft, HostError> {
        let draft_id = draft_id.into();
        if draft_id.is_empty() {
            return Err(HostError::DraftInvalid {
                draft_id,
                detail: "draft_id must not be empty".to_string(),
            });
        }
        if self.has_draft(&draft_id) {
            return Err(HostError::DraftAlreadyExists { draft_id });
        }
        let source_feature_id = source_feature_id.into();
        if !valid_feature_path_component(&source_feature_id) {
            return Err(HostError::DraftInvalid {
                draft_id,
                detail: "source_feature_id must be a plain feature id".to_string(),
            });
        }
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let loaded = Bundle::at(&root).open()?;
        let source_path = committed_brep_path(&root, &source_feature_id);
        let source_brep_sha256 =
            sha256_path(&source_path).map_err(|error| HostError::DraftInvalid {
                draft_id: draft_id.clone(),
                detail: format!("source BREP could not be read: {error}"),
            })?;
        let mut request = request;
        request.base_path = source_path;
        request = request.with_output_path(&root, "unused.brep");
        request
            .validate()
            .map_err(|detail| HostError::DraftInvalid {
                draft_id: draft_id.clone(),
                detail,
            })?;
        self.current.replace(Some(loaded.clone()));
        let draft = CommandDraft {
            draft_id: draft_id.clone(),
            bundle_root: root,
            source_feature_id,
            source_revision: loaded.revision_hash_hex().to_string(),
            source_brep_sha256,
            request,
            preview_path: None,
            created_at: Instant::now(),
        };
        self.drafts.borrow_mut().insert(draft_id, draft.clone());
        Ok(draft)
    }

    /// Replace the semantic values of a draft without changing its source
    /// binding. Any prior preview is invalidated before the new values land.
    pub fn update_draft(
        &self,
        draft_id: &str,
        request: DraftRequest,
    ) -> Result<CommandDraft, HostError> {
        let mut drafts = self.drafts.borrow_mut();
        let draft = drafts
            .get_mut(draft_id)
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?;
        let mut request = request;
        request.base_path = committed_brep_path(&draft.bundle_root, &draft.source_feature_id);
        let request = request.with_output_path(&draft.bundle_root, "unused.brep");
        request
            .validate()
            .map_err(|detail| HostError::DraftInvalid {
                draft_id: draft_id.to_string(),
                detail,
            })?;
        if let Some(path) = draft.preview_path.take() {
            remove_preview_stage(&path);
        }
        draft.request = request;
        Ok(draft.clone())
    }

    /// Evaluate a draft through the production OCCT worker without promoting
    /// its staged BREP or changing any canonical bundle bytes.
    pub fn preview_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        worker: &OcctWorker,
    ) -> Result<DraftPreviewView, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft = self.drafts.borrow().get(draft_id).cloned().ok_or_else(|| {
            HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            }
        })?;
        if draft.bundle_root != root {
            return Err(HostError::DraftInvalid {
                draft_id: draft_id.to_string(),
                detail: "draft belongs to a different bundle".to_string(),
            });
        }
        let loaded = Bundle::at(&root).open()?;
        self.clear_draft_preview(draft_id);
        self.validate_draft_source(&draft, &loaded)?;
        if let Some(path) = &draft.preview_path {
            remove_preview_stage(path);
        }
        let stage = preview_stage_path(&root, draft_id);
        fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
            detail: format!("create preview stage failed: {error}"),
        })?;
        let request = draft
            .request
            .clone()
            .with_output_path(&stage, "preview.brep");
        let result = match worker
            .clone()
            .with_revision_id(draft.source_revision.clone())
            .draft(&request)
        {
            Ok(result) if result.is_success() => result,
            Ok(result) => {
                remove_preview_stage(&stage);
                return Err(HostError::BrepInvalid {
                    request_id: Some(request.request_id),
                    detail: format!("draft preview returned status {}", result.status),
                });
            }
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(HostError::from(error));
            }
        };
        let input_fingerprint = draft_input_fingerprint(&draft, &result.brep_sha256);
        let preview_revision = draft_preview_revision(&draft.source_revision, &input_fingerprint);
        let preview_path = result.brep_path.clone();
        self.drafts
            .borrow_mut()
            .get_mut(draft_id)
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?
            .preview_path = Some(stage);
        Ok(DraftPreviewView {
            draft_id: draft_id.to_string(),
            source_revision: draft.source_revision,
            preview_revision,
            input_fingerprint,
            result,
            brep_path: preview_path,
        })
    }

    /// Re-evaluate and atomically promote a draft. The preview BREP is never
    /// trusted as commit input; a fresh worker result is required.
    pub fn commit_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        worker: &OcctWorker,
    ) -> Result<DraftCommitView, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft = self.drafts.borrow().get(draft_id).cloned().ok_or_else(|| {
            HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            }
        })?;
        if draft.bundle_root != root {
            return Err(HostError::DraftInvalid {
                draft_id: draft_id.to_string(),
                detail: "draft belongs to a different bundle".to_string(),
            });
        }
        let loaded = Bundle::at(&root).open()?;
        self.current.replace(Some(loaded.clone()));
        self.clear_draft_preview(draft_id);
        self.validate_draft_source(&draft, &loaded)?;
        if let Some(path) = draft.preview_path {
            remove_preview_stage(&path);
        }
        let stage = preview_stage_path(&root, &format!("{draft_id}-commit"));
        fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
            detail: format!("create commit stage failed: {error}"),
        })?;
        let request = draft
            .request
            .clone()
            .with_output_path(&stage, "commit.brep");
        let result = match worker
            .clone()
            .with_revision_id(draft.source_revision.clone())
            .draft(&request)
        {
            Ok(result) if result.is_success() => result,
            Ok(result) => {
                remove_preview_stage(&stage);
                return Err(HostError::BrepInvalid {
                    request_id: Some(request.request_id),
                    detail: format!("draft commit returned status {}", result.status),
                });
            }
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(HostError::from(error));
            }
        };
        let brep_bytes = fs::read(&result.brep_path).map_err(|error| HostError::BrepIo {
            detail: format!("read draft commit BREP failed: {error}"),
        })?;
        if brep_bytes.len() != result.brep_bytes
            || format!("{:x}", Sha256::digest(&brep_bytes)) != result.brep_sha256
        {
            remove_preview_stage(&stage);
            return Err(HostError::BrepIo {
                detail: "draft commit BREP changed after worker verification".to_string(),
            });
        }
        let bundle = Bundle::at(&root);
        let kind = format!("brep:{}", request.feature_id);
        let updated = match bundle.append_feature_with_brep_if_revision(
            &request.feature_id,
            &kind,
            &draft.source_revision,
            &brep_bytes,
        ) {
            Ok(updated) => updated,
            Err(error) => {
                self.current.replace(Some(loaded));
                remove_preview_stage(&stage);
                return Err(error.into());
            }
        };
        let snapshot = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        remove_preview_stage(&stage);
        self.drafts.borrow_mut().remove(draft_id);
        Ok(DraftCommitView { snapshot, result })
    }

    /// Refuse a draft and remove every transient preview artifact.
    pub fn discard_draft(&self, draft_id: &str) -> Result<(), HostError> {
        let draft =
            self.drafts
                .borrow_mut()
                .remove(draft_id)
                .ok_or_else(|| HostError::DraftNotFound {
                    draft_id: draft_id.to_string(),
                })?;
        if let Some(path) = draft.preview_path {
            remove_preview_stage(&path);
        }
        Ok(())
    }

    pub fn has_draft(&self, draft_id: &str) -> bool {
        self.drafts.borrow().contains_key(draft_id)
    }

    fn clear_draft_preview(&self, draft_id: &str) {
        if let Some(draft) = self.drafts.borrow_mut().get_mut(draft_id)
            && let Some(path) = draft.preview_path.take()
        {
            remove_preview_stage(&path);
        }
    }

    fn clear_bracket_draft_preview(&self, draft_key: &(PathBuf, String)) {
        if let Some(draft) = self.bracket_drafts.borrow_mut().get_mut(draft_key)
            && let Some(path) = draft.preview_path.take()
        {
            remove_preview_stage(&path);
        }
    }

    /// Create the initial parameterized L-bracket through the OCCT worker.
    pub fn create_bracket(
        &self,
        root: impl AsRef<Path>,
        request: BracketRequest,
        worker: &OcctWorker,
    ) -> Result<SnapshotView, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let mut request = request;
        let loaded = Bundle::at(&root).open()?;
        let stage = preview_stage_path(&root, &format!("create-{}", request.feature_id));
        request = request.with_output_path(&stage, "bracket.brep");
        request
            .validate()
            .map_err(|detail| HostError::Validation { detail })?;
        fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
            detail: format!("create bracket stage failed: {error}"),
        })?;
        let result = match worker
            .clone()
            .with_revision_id(loaded.revision_hash_hex())
            .bracket(&request)
        {
            Ok(result) if result.is_success() => result,
            Ok(result) => {
                remove_preview_stage(&stage);
                return Err(HostError::BrepInvalid {
                    request_id: Some(request.request_id),
                    detail: format!("bracket returned status {}", result.status),
                });
            }
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(error.into());
            }
        };
        let bytes = match read_verified_worker_brep(&result) {
            Ok(bytes) => bytes,
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(error);
            }
        };
        let kind = bracket_kind(&request);
        let snapshot = match self.promote_brep_bytes(
            &root,
            &request.feature_id,
            &kind,
            loaded.revision_hash_hex(),
            &bytes,
            None,
            None,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(error);
            }
        };
        remove_preview_stage(&stage);
        let _ = result;
        Ok(snapshot)
    }

    pub fn open_bracket_parameter_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: impl Into<String>,
        bracket_id: impl Into<String>,
        request: BracketRequest,
    ) -> Result<BracketParameterDraft, HostError> {
        let draft_id = draft_id.into();
        let bracket_id = bracket_id.into();
        if draft_id.is_empty() || !valid_feature_path_component(&bracket_id) {
            return Err(HostError::DraftInvalid {
                draft_id,
                detail: "draft and bracket ids must be non-empty plain identifiers".to_string(),
            });
        }
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft_key = draft_map_key(&root, &draft_id);
        if self.bracket_drafts.borrow().contains_key(&draft_key) {
            return Err(HostError::DraftAlreadyExists { draft_id });
        }
        let loaded = Bundle::at(&root).open()?;
        let source_path = committed_brep_path(&root, &bracket_id);
        let source_brep_sha256 =
            sha256_path(&source_path).map_err(|error| HostError::DraftInvalid {
                draft_id: draft_id.clone(),
                detail: format!("bracket source BREP could not be read: {error}"),
            })?;
        let request = request
            .with_feature_id(&bracket_id)
            .with_output_path(&root, "unused.brep");
        request
            .validate()
            .map_err(|detail| HostError::DraftInvalid {
                draft_id: draft_id.clone(),
                detail,
            })?;
        let draft = BracketParameterDraft {
            draft_id: draft_id.clone(),
            bundle_root: root,
            bracket_id,
            source_revision: loaded.revision_hash_hex().to_string(),
            source_brep_sha256,
            request,
            sequence: 0,
            preview_path: None,
            created_at: Instant::now(),
        };
        self.bracket_drafts
            .borrow_mut()
            .insert(draft_key, draft.clone());
        self.current.replace(Some(loaded));
        Ok(draft)
    }

    pub fn preview_bracket_parameter_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        worker: &OcctWorker,
    ) -> Result<BracketPreviewView, HostError> {
        let cancel = AtomicBool::new(false);
        self.preview_bracket_parameter_draft_with_cancel(root, draft_id, worker, &cancel)
    }

    pub fn preview_bracket_parameter_draft_with_cancel(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        worker: &OcctWorker,
        cancel: &AtomicBool,
    ) -> Result<BracketPreviewView, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft_key = draft_map_key(&root, draft_id);
        let draft = self
            .bracket_drafts
            .borrow()
            .get(&draft_key)
            .cloned()
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?;
        if draft.bundle_root != root {
            return Err(HostError::DraftInvalid {
                draft_id: draft_id.to_string(),
                detail: "draft belongs to a different bundle".to_string(),
            });
        }
        let loaded = Bundle::at(&root).open()?;
        self.clear_bracket_draft_preview(&draft_key);
        self.validate_bracket_source(&draft, &loaded)?;
        if let Some(path) = &draft.preview_path {
            remove_preview_stage(path);
        }
        let stage = preview_stage_path(&root, draft_id);
        fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
            detail: format!("create bracket preview stage failed: {error}"),
        })?;
        let request = draft
            .request
            .clone()
            .with_output_path(&stage, "preview.brep");
        let result = match worker
            .clone()
            .with_revision_id(draft.source_revision.clone())
            .bracket_with_cancel(&request, cancel)
        {
            Ok(result) if result.is_success() => result,
            Ok(result) => {
                remove_preview_stage(&stage);
                return Err(HostError::BrepInvalid {
                    request_id: Some(request.request_id),
                    detail: format!("bracket preview returned status {}", result.status),
                });
            }
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(error.into());
            }
        };
        if let Err(error) = read_verified_worker_brep(&result) {
            remove_preview_stage(&stage);
            return Err(error);
        }
        let input_fingerprint = bracket_input_fingerprint(&draft, &result.brep_sha256);
        let preview_revision = draft_preview_revision(&draft.source_revision, &input_fingerprint);
        let preview_path = result.brep_path.clone();
        self.bracket_drafts
            .borrow_mut()
            .get_mut(&draft_key)
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?
            .preview_path = Some(stage);
        Ok(BracketPreviewView {
            draft_id: draft_id.to_string(),
            source_revision: draft.source_revision,
            preview_revision,
            input_fingerprint,
            result,
            brep_path: preview_path,
        })
    }

    pub fn update_bracket_parameter_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        expected_sequence: u64,
        request: BracketRequest,
    ) -> Result<BracketParameterDraft, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft_key = draft_map_key(&root, draft_id);
        let mut drafts = self.bracket_drafts.borrow_mut();
        let draft = drafts
            .get_mut(&draft_key)
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?;
        if draft.sequence != expected_sequence {
            return Err(HostError::DraftSequenceConflict {
                draft_id: draft_id.to_string(),
                expected: expected_sequence,
                current: draft.sequence,
            });
        }
        let request = request
            .with_feature_id(&draft.bracket_id)
            .with_output_path(&draft.bundle_root, "unused.brep");
        request
            .validate()
            .map_err(|detail| HostError::DraftInvalid {
                draft_id: draft_id.to_string(),
                detail,
            })?;
        if let Some(path) = &draft.preview_path {
            remove_preview_stage(path);
        }
        draft.request = request;
        draft.preview_path = None;
        draft.sequence += 1;
        Ok(draft.clone())
    }

    pub fn commit_bracket_parameter_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        worker: &OcctWorker,
    ) -> Result<BracketCommitView, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft_key = draft_map_key(&root, draft_id);
        let draft = self
            .bracket_drafts
            .borrow()
            .get(&draft_key)
            .cloned()
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?;
        if draft.bundle_root != root {
            return Err(HostError::DraftInvalid {
                draft_id: draft_id.to_string(),
                detail: "draft belongs to a different bundle".to_string(),
            });
        }
        let loaded = Bundle::at(&root).open()?;
        self.current.replace(Some(loaded.clone()));
        self.clear_bracket_draft_preview(&draft_key);
        self.validate_bracket_source(&draft, &loaded)?;
        if let Some(path) = &draft.preview_path {
            remove_preview_stage(path);
        }
        let stage = preview_stage_path(&root, &format!("{draft_id}-commit"));
        fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
            detail: format!("create bracket commit stage failed: {error}"),
        })?;
        let request = draft
            .request
            .clone()
            .with_output_path(&stage, "commit.brep");
        let result = match worker
            .clone()
            .with_revision_id(draft.source_revision.clone())
            .bracket(&request)
        {
            Ok(result) if result.is_success() => result,
            Ok(result) => {
                remove_preview_stage(&stage);
                return Err(HostError::BrepInvalid {
                    request_id: Some(request.request_id),
                    detail: format!("bracket commit returned status {}", result.status),
                });
            }
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(error.into());
            }
        };
        let bytes = match read_verified_worker_brep(&result) {
            Ok(bytes) => bytes,
            Err(error) => {
                remove_preview_stage(&stage);
                return Err(error);
            }
        };
        let input_fingerprint = bracket_input_fingerprint(&draft, &result.brep_sha256);
        if let Some(committed) = Bundle::at(&root).find_idempotency_key(draft_id)?
            && sha256_path(&committed_brep_path(&root, &draft.bracket_id)).ok()
                == Some(result.brep_sha256.clone())
        {
            let snapshot = SnapshotView::from(&committed);
            self.current.replace(Some(committed));
            self.bracket_drafts.borrow_mut().remove(&draft_key);
            remove_preview_stage(&stage);
            return Ok(BracketCommitView {
                snapshot,
                input_fingerprint,
            });
        }
        let kind = bracket_kind_with_draft(&request, Some(draft_id));
        let snapshot = match self.promote_brep_bytes(
            &root,
            &draft.bracket_id,
            &kind,
            &draft.source_revision,
            &bytes,
            Some(&draft.source_brep_sha256),
            Some(draft_id),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                // Parent-sync failures can report after publication. Resolve
                // that durable outcome by looking up the same semantic key
                // before retrying or surfacing a failure.
                if let Ok(committed) = Bundle::at(&root).open()
                    && committed
                        .log
                        .entries()
                        .iter()
                        .any(|entry| entry.idempotency_key.as_deref() == Some(draft_id))
                    && sha256_path(&committed_brep_path(&root, &draft.bracket_id)).ok()
                        == Some(result.brep_sha256.clone())
                {
                    let snapshot = SnapshotView::from(&committed);
                    self.current.replace(Some(committed));
                    self.bracket_drafts.borrow_mut().remove(&draft_key);
                    remove_preview_stage(&stage);
                    return Ok(BracketCommitView {
                        snapshot,
                        input_fingerprint,
                    });
                }
                remove_preview_stage(&stage);
                if matches!(
                    &error,
                    HostError::Persistence(BundleError::Io(_))
                        | HostError::Persistence(BundleError::Backup { .. })
                ) {
                    return Err(HostError::DraftUnknownOutcome {
                        draft_id: draft_id.to_string(),
                        source_revision: draft.source_revision.clone(),
                        recovery: "retry_same_idempotency_key",
                    });
                }
                return Err(error);
            }
        };
        remove_preview_stage(&stage);
        self.bracket_drafts.borrow_mut().remove(&draft_key);
        Ok(BracketCommitView {
            snapshot,
            input_fingerprint,
        })
    }

    pub fn discard_bracket_parameter_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
    ) -> Result<String, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft_key = draft_map_key(&root, draft_id);
        let draft = self
            .bracket_drafts
            .borrow_mut()
            .remove(&draft_key)
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?;
        if let Some(path) = draft.preview_path {
            remove_preview_stage(&path);
        }
        Ok(draft.source_revision)
    }

    pub fn has_bracket_parameter_draft(&self, root: impl AsRef<Path>, draft_id: &str) -> bool {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        self.bracket_drafts
            .borrow()
            .contains_key(&draft_map_key(&root, draft_id))
    }

    pub fn bracket_draft_source_revision(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
    ) -> Option<String> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let key = draft_map_key(&root, draft_id);
        self.bracket_drafts
            .borrow()
            .get(&key)
            .map(|draft| draft.source_revision.clone())
    }

    pub fn bracket_draft_sequence(&self, root: impl AsRef<Path>, draft_id: &str) -> Option<u64> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        self.bracket_drafts
            .borrow()
            .get(&draft_map_key(&root, draft_id))
            .map(|draft| draft.sequence)
    }

    /// Remove abandoned transient drafts and their staged worker output.
    /// Session adapters call this at input boundaries; `Host::drop` remains
    /// the final cleanup guard for sessions that terminate without another
    /// input.
    pub fn prune_expired_drafts(&self, max_age: Duration) -> usize {
        let now = Instant::now();
        let generic_ids: Vec<_> = self
            .drafts
            .borrow()
            .iter()
            .filter(|(_, draft)| now.duration_since(draft.created_at) > max_age)
            .map(|(id, _)| id.clone())
            .collect();
        let bracket_ids: Vec<_> = self
            .bracket_drafts
            .borrow()
            .iter()
            .filter(|(_, draft)| now.duration_since(draft.created_at) > max_age)
            .map(|(id, _)| id.clone())
            .collect();
        let mut removed = 0;
        for id in generic_ids {
            if let Some(draft) = self.drafts.borrow_mut().remove(&id) {
                if let Some(path) = draft.preview_path {
                    remove_preview_stage(&path);
                }
                removed += 1;
            }
        }
        for id in bracket_ids {
            if let Some(draft) = self.bracket_drafts.borrow_mut().remove(&id) {
                if let Some(path) = draft.preview_path {
                    remove_preview_stage(&path);
                }
                removed += 1;
            }
        }
        removed
    }

    fn validate_bracket_source(
        &self,
        draft: &BracketParameterDraft,
        loaded: &LoadedBundle,
    ) -> Result<(), HostError> {
        if loaded.revision_hash_hex() != draft.source_revision {
            return Err(HostError::DraftStale {
                draft_id: draft.draft_id.clone(),
                source_revision: draft.source_revision.clone(),
                current_revision: loaded.revision_hash_hex().to_string(),
                recovery: "discard_and_reopen",
            });
        }
        let source_path = committed_brep_path(&draft.bundle_root, &draft.bracket_id);
        let source_sha = sha256_path(&source_path).map_err(|_| HostError::DraftSourceChanged {
            draft_id: draft.draft_id.clone(),
            source_feature_id: draft.bracket_id.clone(),
            recovery: "reload_source_and_reopen",
        })?;
        if source_sha != draft.source_brep_sha256 {
            return Err(HostError::DraftSourceChanged {
                draft_id: draft.draft_id.clone(),
                source_feature_id: draft.bracket_id.clone(),
                recovery: "reload_source_and_reopen",
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn promote_brep_bytes(
        &self,
        root: &Path,
        feature_id: &str,
        kind: &str,
        expected_revision: &str,
        bytes: &[u8],
        source_brep_sha256: Option<&str>,
        idempotency_key: Option<&str>,
    ) -> Result<SnapshotView, HostError> {
        let bundle = Bundle::at(root);
        let updated = match source_brep_sha256 {
            Some(source_brep_sha256) => bundle
                .append_feature_with_brep_if_revision_and_source_and_idempotency(
                    feature_id,
                    kind,
                    expected_revision,
                    source_brep_sha256,
                    idempotency_key,
                    bytes,
                )?,
            None => bundle.append_feature_with_brep_if_revision(
                feature_id,
                kind,
                expected_revision,
                bytes,
            )?,
        };
        let snapshot = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        Ok(snapshot)
    }

    fn validate_draft_source(
        &self,
        draft: &CommandDraft,
        loaded: &LoadedBundle,
    ) -> Result<(), HostError> {
        let current_revision = loaded.revision_hash_hex();
        if current_revision != draft.source_revision {
            return Err(HostError::DraftStale {
                draft_id: draft.draft_id.clone(),
                source_revision: draft.source_revision.clone(),
                current_revision: current_revision.to_string(),
                recovery: "discard_and_reopen",
            });
        }
        let source_path = committed_brep_path(&draft.bundle_root, &draft.source_feature_id);
        let source_sha =
            sha256_path(&source_path).map_err(|_error| HostError::DraftSourceChanged {
                draft_id: draft.draft_id.clone(),
                source_feature_id: draft.source_feature_id.clone(),
                recovery: "reload_source_and_reopen",
            })?;
        if source_sha != draft.source_brep_sha256 {
            return Err(HostError::DraftSourceChanged {
                draft_id: draft.draft_id.clone(),
                source_feature_id: draft.source_feature_id.clone(),
                recovery: "reload_source_and_reopen",
            });
        }
        Ok(())
    }

    /// Accept a completed worker lifecycle and publish its one Derived Result.
    /// The Host owns this boundary because only it can compare the staged
    /// result with its current Revision Snapshot and register the result.
    pub fn accept_derived_result(
        &self,
        artifact_root: impl AsRef<Path>,
        request: &Layer1ArtifactRequest,
        expected_worker: &WorkerFingerprint,
        outcome: SupervisorOutcome,
    ) -> Result<Layer1DerivedResult, Diagnostic> {
        let root = artifact_root.as_ref();
        let SupervisorOutcome::Completed {
            request_id,
            result: _,
            mut artifact_headers,
        } = outcome
        else {
            cleanup_staged_artifact(root, &request.staging_name);
            return Err(Diagnostic::artifact_promotion_failure(
                "worker_result_not_completed",
            ));
        };
        if request_id != request.request_id {
            for artifact in &artifact_headers {
                cleanup_staged_artifact(root, &artifact.header.staging_name);
            }
            cleanup_staged_artifact(root, &request.staging_name);
            return Err(Diagnostic::artifact_request_mismatch(
                "completed_request_id_mismatch",
            ));
        }
        if artifact_headers.len() != 1 {
            for artifact in &artifact_headers {
                cleanup_staged_artifact(root, &artifact.header.staging_name);
            }
            cleanup_staged_artifact(root, &request.staging_name);
            return Err(Diagnostic::artifact_promotion_failure(
                "expected_exactly_one_artifact",
            ));
        }
        let artifact = artifact_headers
            .pop()
            .expect("checked exactly one artifact");
        if artifact.schema_version != threeterm_protocol::schema_version() {
            cleanup_staged_artifact(root, &request.staging_name);
            cleanup_staged_artifact(root, &artifact.header.staging_name);
            return Err(Diagnostic::artifact_promotion_failure(
                "artifact_schema_mismatch",
            ));
        }
        self.accept_staged_artifact(root, request, expected_worker, artifact.header)
    }

    fn accept_staged_artifact(
        &self,
        artifact_root: impl AsRef<Path>,
        request: &Layer1ArtifactRequest,
        expected_worker: &WorkerFingerprint,
        header: threeterm_protocol::artifact::ArtifactHeader,
    ) -> Result<Layer1DerivedResult, Diagnostic> {
        let root = artifact_root.as_ref();
        let header_staging_name = header.staging_name.clone();
        let reject = |diagnostic| {
            cleanup_staged_artifact(root, &request.staging_name);
            cleanup_staged_artifact(root, &header_staging_name);
            diagnostic
        };
        // Exclusion policy: never cache Command Drafts, hover/pointer/candidate,
        // stale last-valid geometry, preview-only beyond session, worker internals.
        if is_layer1_excluded(request) {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "excluded_layer1_artifact: draft/hover/candidate/pointer/stale/preview-only/worker-internal/tmp/stderr",
            )));
        }
        let current = self.current().ok_or_else(|| {
            reject(Diagnostic::artifact_promotion_failure(
                "canonical_snapshot_missing",
            ))
        })?;
        let expected_cache_key = Layer1CacheKey::issue(request, expected_worker);
        if request.source_revision_id != current.revision_hash
            || header.source_revision_id != request.source_revision_id
            || header.cache_key.source_revision_id != request.source_revision_id
        {
            return Err(reject(Diagnostic::artifact_revision_mismatch(
                "artifact_source_revision_mismatch",
            )));
        }
        if header.request_id != request.request_id {
            return Err(reject(Diagnostic::artifact_request_mismatch(
                "artifact_request_id_mismatch",
            )));
        }
        if header.cache_key != expected_cache_key {
            return Err(reject(Diagnostic::artifact_cache_key_mismatch(
                "artifact_cache_key_mismatch",
            )));
        }
        if header.artifact_kind != request.artifact_kind
            || header.staging_name != request.staging_name
            || header.worker_fingerprint != *expected_worker
        {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "artifact_header_mismatch",
            )));
        }

        let stage = Stage::open(root)
            .map_err(|error| reject(Diagnostic::artifact_promotion_failure(&error.to_string())))?;
        stage
            .verify(&header)
            .map_err(|error| reject(artifact_error_diagnostic(&error)))?;
        let final_name = header.cache_key.final_artifact_name();
        if let Some(existing) = self.layer1_result(&header.cache_key) {
            if stage
                .published_matches(&final_name, existing.byte_count, &existing.sha256)
                .map_err(|error| reject(artifact_error_diagnostic(&error)))?
            {
                stage.discard_verified(&header.staging_name);
                return Ok(existing);
            }
            stage
                .discard_final(&final_name)
                .map_err(|error| reject(artifact_error_diagnostic(&error)))?;
        }
        let path = stage
            .publish_verified(&header.staging_name, &final_name)
            .map_err(|error| reject(artifact_error_diagnostic(&error)))?;
        let result = Layer1DerivedResult {
            request_id: header.request_id,
            source_revision_id: header.source_revision_id,
            cache_key: header.cache_key,
            worker_fingerprint: header.worker_fingerprint,
            artifact_kind: header.artifact_kind,
            artifact_name: header.staging_name,
            byte_count: header.byte_count,
            sha256: header.sha256,
            path,
        };
        self.layer1_results
            .borrow_mut()
            .insert(result.cache_key.clone(), result.clone());
        Ok(result)
    }

    pub fn layer1_result(&self, cache_key: &Layer1CacheKey) -> Option<Layer1DerivedResult> {
        self.layer1_results.borrow().get(cache_key).cloned()
    }

    /// Atomically commit a worker-emitted BREP file into the bundle.
    ///
    /// The worker's stage lives outside the canonical log (a derived
    /// artifact under the host-managed staging directory). This seam
    /// copies the validated BREP into `<root>/brep/<feature_id>.brep`,
    /// advances the canonical log by one entry (kind = "brep:<feature_id>"),
    /// seals the manifest, and returns the new `SnapshotView`. On any
    /// filesystem failure the prior manifest, log, and any prior
    /// committed BREP for the same `feature_id` are preserved
    /// byte-equal and `Host::current()` is restored.
    pub fn commit_brep_feature(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        brep_path: &Path,
    ) -> Result<SnapshotView, HostError> {
        self.commit_brep_feature_inner(root.as_ref(), feature_id, brep_path, None, None)
    }

    /// Commit a worker artifact while verifying the advertised size and
    /// digest from the same no-follow file handle used for promotion.
    pub fn commit_brep_feature_verified(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        brep_path: &Path,
        expected_bytes: usize,
        expected_sha256: &str,
    ) -> Result<SnapshotView, HostError> {
        self.commit_brep_feature_inner(
            root.as_ref(),
            feature_id,
            brep_path,
            Some((expected_bytes, expected_sha256)),
            None,
        )
    }

    /// Commit a verified BREP only if the bundle is still at the revision
    /// that authorized the worker request. The persistence lock performs the
    /// final comparison immediately before publication.
    pub fn commit_brep_feature_verified_at_revision(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        brep_path: &Path,
        expected_revision: &str,
        expected_bytes: usize,
        expected_sha256: &str,
    ) -> Result<SnapshotView, HostError> {
        self.commit_brep_feature_inner(
            root.as_ref(),
            feature_id,
            brep_path,
            Some((expected_bytes, expected_sha256)),
            Some(expected_revision),
        )
    }

    fn commit_brep_feature_inner(
        &self,
        root: &Path,
        feature_id: &str,
        brep_path: &Path,
        expected: Option<(usize, &str)>,
        expected_revision: Option<&str>,
    ) -> Result<SnapshotView, HostError> {
        if !brep_path.is_file() {
            return Err(HostError::BrepFileMissing {
                path: brep_path.to_path_buf(),
            });
        }
        let _stage_cleanup = WorkerStageCleanup {
            root,
            path: brep_path,
        };
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        if !root.exists() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepIo {
                detail: "cannot commit a BREP while recovering a sealed previous generation"
                    .to_string(),
            });
        }
        let prior_view = SnapshotView::from(&loaded);
        let prior_manifest = read_bundle_file(&bundle_root(root), "manifest.json")?;
        let prior_log = read_bundle_file(&bundle_root(root), "transactions.log")?;

        let brep_dir = bundle_root(root).join(BREP_SUBDIR);
        if let Err(detail) = ensure_dir(&brep_dir) {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepIo { detail });
        }
        let target = brep_dir.join(format!("{feature_id}.brep"));
        // Preserve any prior committed BREP for this feature id so a
        // mid-commit failure doesn't orphan the canonical log entry.
        let prior_brep = if target.is_file() {
            let mut buffer = Vec::new();
            std::io::Read::read_to_end(
                &mut fs::File::open(&target).map_err(|error| HostError::BrepIo {
                    detail: format!("open prior BREP failed: {error}"),
                })?,
                &mut buffer,
            )
            .map_err(|error| HostError::BrepIo {
                detail: format!("read prior BREP failed: {error}"),
            })?;
            Some(buffer)
        } else {
            None
        };
        if let Err(detail) = copy_brep_verified(brep_path, &target, expected) {
            cleanup_worker_stage(root, brep_path);
            self.current.replace(Some(loaded));
            return Err(HostError::BrepIo { detail });
        }

        let kind = format!("brep:{feature_id}");
        let updated_result = match expected_revision {
            Some(expected_revision) => {
                bundle.append_feature_if_revision(feature_id, &kind, expected_revision)
            }
            None => bundle.append_feature(feature_id, &kind),
        };
        let updated = match updated_result {
            Ok(loaded) => loaded,
            Err(error) => {
                if let (Ok(manifest), Ok(log)) = (
                    read_bundle_file(&bundle_root(root), "manifest.json"),
                    read_bundle_file(&bundle_root(root), "transactions.log"),
                ) && (manifest != prior_manifest || log != prior_log)
                {
                    if let Ok(committed) = bundle.open() {
                        self.current.replace(Some(committed));
                    }
                    return Err(HostError::from(error));
                }
                // Restore the prior BREP bytes (or remove the new file if
                // there was no prior) and verify the canonical state
                // survived. The prior manifest and log are untouched
                // because we never reached a successful append.
                restore_brep(&target, prior_brep.as_deref());
                cleanup_worker_stage(root, brep_path);
                // Fail-closed: if the canonical state was not preserved
                // by the append, surface the persistence error so the
                // diagnostic taxonomy sees the failure.
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        let _ = prior_view;
        let view = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        cleanup_worker_stage(root, brep_path);
        Ok(view)
    }

    /// Extrude `request` against the disposable OCCT worker and, on
    /// success, commit the BREP into a new revision. Returns the new
    /// snapshot view plus the typed `ExtrudeResult`.
    ///
    /// Atomicity: a worker spawn / exit / parse failure, a non-`ok`
    /// status, a `BRepCheck_Analyzer` failure, or a persistence
    /// append failure all leave the bundle's `manifest.json` and
    /// `transactions.log` byte-identical to the pre-call snapshot.
    pub fn extrude(
        &self,
        root: impl AsRef<Path>,
        request: ExtrudeRequest,
        worker: &OcctWorker,
    ) -> Result<ExtrudeCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .extrude(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "extrude returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(ExtrudeCommitView { snapshot, result })
    }

    /// Extrude `request` with a cooperative cancellation token. Mirrors
    /// `OcctWorker::extrude_with_cancel` but preserves the host's
    /// canonical-state atomicity and `HostError` projection.
    pub fn extrude_with_cancel(
        &self,
        root: impl AsRef<Path>,
        request: ExtrudeRequest,
        worker: &OcctWorker,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<ExtrudeCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .extrude_with_cancel(&request, cancel)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "extrude returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(ExtrudeCommitView { snapshot, result })
    }

    /// Boolean-fuse `request` against the disposable OCCT worker and,
    /// on success, commit the fused BREP into a new revision.
    pub fn boolean_fuse(
        &self,
        root: impl AsRef<Path>,
        request: BooleanFuseRequest,
        worker: &OcctWorker,
    ) -> Result<BooleanFuseCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .boolean_fuse(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "boolean_fuse returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(BooleanFuseCommitView { snapshot, result })
    }

    /// Fillet `request` against the disposable OCCT worker and, on
    /// success, commit the filleted BREP into a new revision.
    pub fn fillet(
        &self,
        root: impl AsRef<Path>,
        request: FilletRequest,
        worker: &OcctWorker,
    ) -> Result<FilletCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .fillet(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "fillet returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(FilletCommitView { snapshot, result })
    }

    /// Chamfer `request` against the disposable OCCT worker and, on
    /// success, commit the chamfered BREP into a new revision.
    pub fn chamfer(
        &self,
        root: impl AsRef<Path>,
        request: ChamferRequest,
        worker: &OcctWorker,
    ) -> Result<ChamferCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .chamfer(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "chamfer returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(ChamferCommitView { snapshot, result })
    }

    /// Hole `request` against the disposable OCCT worker and, on
    /// success, commit the holed BREP into a new revision.
    pub fn hole(
        &self,
        root: impl AsRef<Path>,
        request: HoleRequest,
        worker: &OcctWorker,
    ) -> Result<HoleCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .hole(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "hole returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(HoleCommitView { snapshot, result })
    }

    /// Revolve `request` against the disposable OCCT worker and, on
    /// success, commit the revolved BREP into a new revision.
    pub fn revolve(
        &self,
        root: impl AsRef<Path>,
        request: RevolveRequest,
        worker: &OcctWorker,
    ) -> Result<RevolveCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .revolve(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "revolve returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(RevolveCommitView { snapshot, result })
    }

    /// Mirror `request` against the disposable OCCT worker and, on
    /// success, commit the mirrored BREP into a new revision.
    pub fn mirror(
        &self,
        root: impl AsRef<Path>,
        request: MirrorRequest,
        worker: &OcctWorker,
    ) -> Result<MirrorCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .mirror(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "mirror returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(MirrorCommitView { snapshot, result })
    }

    /// Linear pattern `request` against the disposable OCCT worker
    /// and, on success, commit the patterned BREP into a new
    /// revision.
    pub fn linear_pattern(
        &self,
        root: impl AsRef<Path>,
        request: LinearPatternRequest,
        worker: &OcctWorker,
    ) -> Result<LinearPatternCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .linear_pattern(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "linear_pattern returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(LinearPatternCommitView { snapshot, result })
    }

    /// Circular pattern `request` against the disposable OCCT worker
    /// and, on success, commit the patterned BREP into a new
    /// revision.
    pub fn circular_pattern(
        &self,
        root: impl AsRef<Path>,
        request: CircularPatternRequest,
        worker: &OcctWorker,
    ) -> Result<CircularPatternCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .circular_pattern(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "circular_pattern returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(CircularPatternCommitView { snapshot, result })
    }

    /// Shell `request` against the disposable OCCT worker and, on
    /// success, commit the shelled BREP into a new revision.
    pub fn shell(
        &self,
        root: impl AsRef<Path>,
        request: ShellRequest,
        worker: &OcctWorker,
    ) -> Result<ShellCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .shell(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "shell returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(ShellCommitView { snapshot, result })
    }

    /// Draft `request` against the disposable OCCT worker and, on
    /// success, commit the drafted BREP into a new revision.
    pub fn draft(
        &self,
        root: impl AsRef<Path>,
        request: DraftRequest,
        worker: &OcctWorker,
    ) -> Result<DraftCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .draft(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "draft returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        )?;
        let _ = prior_view;
        Ok(DraftCommitView { snapshot, result })
    }

    /// Loft `request` against the disposable OCCT worker and, on
    /// success, commit the lofted BREP into a new revision.
    pub fn loft(
        &self,
        root: impl AsRef<Path>,
        request: LoftRequest,
        worker: &OcctWorker,
    ) -> Result<LoftCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let prior_view = SnapshotView::from(&loaded);

        let result = match worker
            .clone()
            .with_revision_id(prior_view.revision_hash.clone())
            .loft(&request)
        {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                request_id: Some(request.request_id.clone()),
                detail: format!(
                    "loft returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = self.commit_brep_feature_verified_at_revision(
            root,
            &feature_id,
            &result.brep_path,
            &prior_view.revision_hash,
            result.brep_bytes,
            &result.brep_sha256,
        )?;
        let _ = prior_view;
        Ok(LoftCommitView { snapshot, result })
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        for draft in self.drafts.get_mut().values() {
            if let Some(path) = &draft.preview_path {
                remove_preview_stage(path);
            }
        }
        for draft in self.bracket_drafts.get_mut().values() {
            if let Some(path) = &draft.preview_path {
                remove_preview_stage(path);
            }
        }
    }
}

fn artifact_error_diagnostic(error: &ArtifactError) -> Diagnostic {
    match error {
        ArtifactError::HashMismatch { expected, actual } => {
            Diagnostic::artifact_hash_mismatch(expected, actual)
        }
        _ => Diagnostic::artifact_promotion_failure(&error.to_string()),
    }
}

fn valid_feature_path_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn committed_brep_path(root: &Path, feature_id: &str) -> PathBuf {
    root.join(BREP_SUBDIR).join(format!("{feature_id}.brep"))
}

fn sha256_path(path: &Path) -> Result<String, std::io::Error> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_verified_worker_brep(result: &BracketResult) -> Result<Vec<u8>, HostError> {
    let bytes = fs::read(&result.brep_path).map_err(|error| HostError::BrepIo {
        detail: format!("read bracket BREP failed: {error}"),
    })?;
    if bytes.len() != result.brep_bytes
        || format!("{:x}", Sha256::digest(&bytes)) != result.brep_sha256
    {
        return Err(HostError::BrepIo {
            detail: "bracket BREP changed after worker verification".to_string(),
        });
    }
    Ok(bytes)
}

fn bracket_kind(request: &BracketRequest) -> String {
    bracket_kind_with_draft(request, None)
}

fn bracket_kind_with_draft(request: &BracketRequest, draft_id: Option<&str>) -> String {
    let _ = draft_id;
    format!(
        "bracket:length={:.17};width={:.17};height={:.17};thickness={:.17}",
        request.length, request.width, request.height, request.thickness,
    )
}

fn bracket_input_fingerprint(draft: &BracketParameterDraft, result_sha256: &str) -> String {
    let semantic = format!(
        "source_revision={}|source_brep_sha256={}|bracket_id={}|length={:.17}|width={:.17}|height={:.17}|thickness={:.17}|result_sha256={}",
        draft.source_revision,
        draft.source_brep_sha256,
        draft.bracket_id,
        draft.request.length,
        draft.request.width,
        draft.request.height,
        draft.request.thickness,
        result_sha256,
    );
    format!("{:x}", Sha256::digest(semantic.as_bytes()))
}

fn preview_stage_path(root: &Path, draft_id: &str) -> PathBuf {
    static NEXT_STAGE_NONCE: AtomicU64 = AtomicU64::new(0);
    let mut hasher = Sha256::new();
    hasher.update(root.as_os_str().as_encoded_bytes());
    hasher.update(draft_id.as_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(
        NEXT_STAGE_NONCE
            .fetch_add(1, Ordering::Relaxed)
            .to_le_bytes(),
    );
    let identity = format!("{:x}", hasher.finalize());
    std::env::temp_dir().join(format!("threeterm-command-draft-{identity}"))
}

fn remove_preview_stage(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn draft_input_fingerprint(draft: &CommandDraft, result_sha256: &str) -> String {
    let semantic = serde_json::json!({
        "source_revision": draft.source_revision,
        "source_brep_sha256": draft.source_brep_sha256,
        "source_feature_id": draft.source_feature_id,
        "angle": draft.request.angle,
        "pull_direction": draft.request.pull_direction,
        "result_sha256": result_sha256,
    });
    let bytes = serde_json::to_vec(&semantic).expect("draft fingerprint serializes");
    format!("{:x}", Sha256::digest(bytes))
}

fn draft_preview_revision(source_revision: &str, input_fingerprint: &str) -> String {
    let mut bytes = Vec::with_capacity(source_revision.len() + input_fingerprint.len());
    bytes.extend_from_slice(source_revision.as_bytes());
    bytes.extend_from_slice(input_fingerprint.as_bytes());
    format!("{:x}", Sha256::digest(bytes))
}

fn cleanup_staged_artifact(root: &Path, staging_name: &str) {
    if staging_name.is_empty()
        || staging_name.contains('/')
        || staging_name.contains('\\')
        || staging_name.contains('\0')
    {
        return;
    }
    let _ = fs::remove_file(root.join(format!("{staging_name}.partial")));
    let _ = fs::remove_file(root.join(format!(".{staging_name}.verified")));
}

fn bundle_root(root: &Path) -> PathBuf {
    if root.exists() {
        return root.to_path_buf();
    }
    let mut previous = root.to_path_buf();
    previous.set_file_name(format!(
        "{}.previous-generation",
        root.file_name().unwrap_or_default().to_string_lossy()
    ));
    previous
}

fn read_bundle_file(root: &Path, name: &str) -> Result<Vec<u8>, HostError> {
    let path = root.join(name);
    let mut file = fs::File::open(&path).map_err(|error| HostError::BrepIo {
        detail: format!("could not read {}: {}", path.display(), error),
    })?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|error| HostError::BrepIo {
            detail: format!("could not read {}: {}", path.display(), error),
        })?;
    Ok(buffer)
}

fn ensure_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("create_dir_all failed: {error}"))
}

#[cfg(test)]
fn copy_brep(source: &Path, target: &Path) -> Result<(), String> {
    copy_brep_verified(source, target, None)
}

fn copy_brep_verified(
    source: &Path,
    target: &Path,
    expected: Option<(usize, &str)>,
) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    // Open the source without following symlinks and pin the opened
    // handle: promotion copies from one verified file identity, so a
    // path swapped between validation and promotion cannot redirect the
    // copy.
    let mut options = fs::OpenOptions::new();
    // O_NOFOLLOW = 0o400000 on Linux: refuse to open through a symlink.
    options.read(true).custom_flags(0o400000);
    let mut reader = options
        .open(source)
        .map_err(|error| format!("open source BREP {} failed: {error}", source.display()))?;
    let opened_metadata = reader
        .metadata()
        .map_err(|error| format!("stat opened BREP {} failed: {error}", source.display()))?;
    let verified_metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("stat source BREP {} failed: {error}", source.display()))?;
    use std::os::unix::fs::MetadataExt;
    if opened_metadata.dev() != verified_metadata.dev()
        || opened_metadata.ino() != verified_metadata.ino()
    {
        return Err(format!(
            "source BREP {} changed identity between validation and promotion",
            source.display()
        ));
    }
    let artifact_limit = threeterm_protocol::worker::MAX_ARTIFACT_BYTES as u64;
    if opened_metadata.len() > artifact_limit {
        return Err(format!(
            "source BREP {} exceeds the {artifact_limit} byte bound",
            source.display()
        ));
    }
    let mut buffer = vec![0u8; 8 * 1024];
    let mut content = Vec::new();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("read source BREP failed: {error}"))?;
        if read == 0 {
            break;
        }
        if content.len() + read > artifact_limit as usize {
            return Err(format!(
                "source BREP {} exceeds the {artifact_limit} byte bound",
                source.display()
            ));
        }
        content.extend_from_slice(&buffer[..read]);
    }
    if let Some((expected_bytes, expected_sha256)) = expected {
        use sha2::{Digest, Sha256};
        let actual_sha256 = format!("{:x}", Sha256::digest(&content));
        if content.len() != expected_bytes || actual_sha256 != expected_sha256 {
            return Err(format!(
                "source BREP content does not match the worker advertisement: bytes={} expected_bytes={} sha256={} expected_sha256={}",
                content.len(),
                expected_bytes,
                actual_sha256,
                expected_sha256
            ));
        }
    }
    // Never create the canonical target before the complete replacement is
    // durable: File::create(target) would truncate a prior BREP before a
    // later write or sync failure could be reported.
    let file_name = target
        .file_name()
        .ok_or_else(|| format!("target BREP {} has no file name", target.display()))?;
    let temporary = target.with_file_name(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "create temporary BREP {} failed: {error}",
                temporary.display()
            )
        })?;
    if let Err(error) = writer.write_all(&content) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("write temporary BREP failed: {error}"));
    }
    if let Err(error) = writer.flush() {
        let _ = fs::remove_file(&temporary);
        return Err(format!("flush temporary BREP failed: {error}"));
    }
    if let Err(error) = writer.sync_all() {
        let _ = fs::remove_file(&temporary);
        return Err(format!("sync temporary BREP failed: {error}"));
    }
    if let Err(error) = fs::rename(&temporary, target) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("rename temporary BREP failed: {error}"));
    }
    Ok(())
}

fn cleanup_worker_stage(root: &Path, path: &Path) {
    let stage = root.join("stage");
    if !path.starts_with(&stage) {
        return;
    }
    let _ = fs::remove_file(path);
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let temporary_prefix = format!("{file_name}.tmp-");
    let Ok(entries) = fs::read_dir(&stage) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(&temporary_prefix))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

struct WorkerStageCleanup<'a> {
    root: &'a Path,
    path: &'a Path,
}

impl Drop for WorkerStageCleanup<'_> {
    fn drop(&mut self) {
        cleanup_worker_stage(self.root, self.path);
    }
}

fn restore_brep(target: &Path, prior_bytes: Option<&[u8]>) {
    match prior_bytes {
        Some(bytes) => {
            let Some(file_name) = target.file_name() else {
                return;
            };
            let temporary = target.with_file_name(format!(
                ".{}.restore-tmp-{}",
                file_name.to_string_lossy(),
                std::process::id()
            ));
            if let Ok(mut writer) = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                if writer.write_all(bytes).is_ok()
                    && writer.sync_all().is_ok()
                    && fs::rename(&temporary, target).is_ok()
                {
                    return;
                }
                let _ = fs::remove_file(&temporary);
            }
        }
        None => {
            let _ = fs::remove_file(target);
        }
    }
}

pub fn schema_version() -> &'static str {
    "threeterm.host/1"
}

#[cfg(test)]
mod tests {
    use super::*;
    use threeterm_domain::ProjectGeneration;
    use threeterm_persistence::{
        Bundle, BundleError, MANIFEST_FILENAME, PRE_MIGRATION_BACKUP_SUFFIX,
        PublicationFailurePoint, fail_next_publication_at, schema_epoch, write_fresh,
        write_v0_fixture,
    };

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
    fn save_reconciles_current_snapshot_after_post_promotion_sync_failure() {
        let root = temp_root("save-parent-sync");
        let host = Host::new();
        host.save(&root, "box-1", "box").expect("first save");

        fail_next_publication_at(PublicationFailurePoint::ParentSync);
        assert!(host.save(&root, "box-2", "box").is_err());

        let on_disk = Bundle::at(&root).open().expect("promoted generation opens");
        assert_eq!(host.current(), Some(SnapshotView::from(&on_disk)));
        assert_eq!(on_disk.log.len(), 2);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(root.with_file_name(format!(
            "{}.previous-generation",
            root.file_name().unwrap_or_default().to_string_lossy()
        )));
    }

    #[test]
    fn migration_sync_failure_reconciles_current_snapshot_after_promotion() {
        let existing_root = temp_root("existing-snapshot");
        let migration_root = temp_root("migration-parent-sync");
        let existing = Bundle::create_for_test(&existing_root, "00".repeat(16).as_str())
            .expect("existing bundle creates");
        existing
            .append_feature("box-1", "box")
            .expect("existing feature appends");
        write_v0_fixture(
            &migration_root,
            ProjectGeneration::with_id("migration-generation"),
        )
        .expect("v0 fixture writes");

        let host = Host::new();
        host.load(&existing_root).expect("existing bundle loads");
        fail_next_publication_at(PublicationFailurePoint::ParentSync);
        assert!(host.load(&migration_root).is_err());

        let promoted = Bundle::at(&migration_root)
            .open()
            .expect("promoted generation opens");
        assert_eq!(host.current(), Some(SnapshotView::from(&promoted)));

        let _ = std::fs::remove_dir_all(existing_root);
        let _ = std::fs::remove_dir_all(migration_root);
    }

    #[test]
    fn save_preflight_reconciles_current_snapshot_after_migration_parent_sync_failure() {
        let existing_root = temp_root("save-existing-snapshot");
        let migration_root = temp_root("save-migration-parent-sync");
        let existing = Bundle::create_for_test(&existing_root, "00".repeat(16).as_str())
            .expect("existing bundle creates");
        existing
            .append_feature("box-1", "box")
            .expect("existing feature appends");
        write_v0_fixture(
            &migration_root,
            ProjectGeneration::with_id("save-migration-generation"),
        )
        .expect("v0 fixture writes");

        let host = Host::new();
        host.load(&existing_root).expect("existing bundle loads");
        fail_next_publication_at(PublicationFailurePoint::ParentSync);
        assert!(host.save(&migration_root, "box-1", "box").is_err());

        let promoted = Bundle::at(&migration_root)
            .open()
            .expect("promoted generation opens");
        assert_eq!(host.current(), Some(SnapshotView::from(&promoted)));

        let _ = std::fs::remove_dir_all(existing_root);
        let _ = std::fs::remove_dir_all(migration_root);
    }

    #[test]
    fn load_migrates_prior_epoch_bundle_and_publishes_snapshot() {
        let root = temp_root("prior-epoch");
        write_v0_fixture(&root, ProjectGeneration::with_id("generation-prior"))
            .expect("prior-epoch bundle writes");
        let backup = root.with_file_name(format!(
            "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
            root.file_name()
                .expect("root has filename")
                .to_string_lossy()
        ));

        let host = Host::new();
        let view = host.load(&root).expect("prior epoch migrates and loads");

        assert_eq!(host.current(), Some(view.clone()));
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(MANIFEST_FILENAME)).expect("migrated manifest reads"),
        )
        .expect("migrated manifest parses");
        assert_eq!(manifest["schema_version"], schema_epoch());
        assert!(backup.is_dir(), "pre-migration backup is retained");
        let reopened = Bundle::at(&root).open().expect("migrated bundle reopens");
        assert_eq!(view, SnapshotView::from(&reopened));

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(backup);
    }

    #[test]
    fn rejected_manifests_preserve_current_snapshot_and_source_bytes() {
        let valid_root = temp_root("valid-manifest");
        Bundle::create_for_test(&valid_root, "00".repeat(16).as_str())
            .expect("valid bundle creates");
        let host = Host::new();
        let current = host.load(&valid_root).expect("valid bundle loads");

        for (label, mutation) in [
            ("malformed", serde_json::json!({ "future_field": true })),
            (
                "unsupported",
                serde_json::json!({ "schema_version": "threeterm.persistence/99" }),
            ),
        ] {
            let root = temp_root(label);
            write_fresh(
                &root,
                ProjectGeneration::with_id(format!("generation-{label}")),
            )
            .expect("current bundle writes");
            let manifest_path = root.join(MANIFEST_FILENAME);
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&manifest_path).expect("manifest reads"))
                    .expect("manifest parses");
            for (key, value) in mutation.as_object().expect("mutation is an object") {
                manifest[key] = value.clone();
            }
            std::fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
            )
            .expect("manifest writes");
            let source = std::fs::read(&manifest_path).expect("source manifest reads");

            assert!(host.load(&root).is_err(), "{label} manifest is rejected");
            assert_eq!(host.current(), Some(current.clone()));
            assert_eq!(
                std::fs::read(&manifest_path).expect("source manifest re-reads"),
                source,
                "{label} manifest remains byte-identical"
            );

            let _ = std::fs::remove_dir_all(root);
        }

        let _ = std::fs::remove_dir_all(valid_root);
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

    #[test]
    fn commit_brep_feature_writes_brep_and_advances_canonical_log() {
        let root = temp_root("brep-commit");
        Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).expect("staging dir creates");
        let brep_source = staging.join("extrude.brep");
        let payload: Vec<u8> = (0..512u32).map(|i| (i & 0xff) as u8).collect();
        std::fs::write(&brep_source, &payload).expect("brep writes");

        let host = Host::new();
        let view = host
            .commit_brep_feature(&root, "box-1", &brep_source)
            .expect("commit succeeds");

        let committed = root.join("brep/box-1.brep");
        assert!(committed.is_file(), "committed BREP is on disk");
        let committed_bytes = std::fs::read(&committed).expect("committed BREP reads");
        assert_eq!(committed_bytes, payload);

        let reloaded = host.load(&root).expect("reloads after commit");
        assert_eq!(reloaded.feature_graph_hash, view.feature_graph_hash);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commit_brep_feature_rejects_missing_source_brep() {
        let root = temp_root("brep-missing");
        Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        let host = Host::new();
        let prior = host.load(&root).expect("loads");
        let missing = root.join("no-such.brep");
        let result = host.commit_brep_feature(&root, "box-1", &missing);
        assert!(matches!(result, Err(HostError::BrepFileMissing { .. })));
        assert_eq!(host.current(), Some(prior));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commit_brep_feature_replaces_prior_bytes_post_commit() {
        let root = temp_root("brep-replace");
        Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).expect("staging dir creates");
        let brep_dir = root.join("brep");
        std::fs::create_dir_all(&brep_dir).expect("brep dir creates");
        let prior_bytes: Vec<u8> = (0..128u8).collect();
        std::fs::write(brep_dir.join("box-1.brep"), &prior_bytes).expect("prior BREP writes");

        let new_source = staging.join("new.brep");
        let new_bytes: Vec<u8> = (128..=255u8).cycle().take(128).collect();
        std::fs::write(&new_source, &new_bytes).expect("new BREP writes");

        let host = Host::new();
        let view = host
            .commit_brep_feature(&root, "box-1", &new_source)
            .expect("commit succeeds");
        assert!(view.feature_graph_hash.len() == 64);

        let committed = std::fs::read(brep_dir.join("box-1.brep")).expect("reads");
        assert_eq!(committed, new_bytes, "BREP bytes are replaced post-commit");
        let reloaded = host.load(&root).expect("reloads");
        assert_eq!(reloaded.feature_graph_hash, view.feature_graph_hash);

        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod promotion_tests {
    use super::*;

    #[test]
    fn copy_brep_rejects_a_symlinked_source() {
        let dir = std::env::temp_dir().join(format!(
            "threeterm-host-copy-nofollow-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir creates");
        let real = dir.join("real.brep");
        std::fs::write(&real, b"real bytes").expect("real file writes");
        let link = dir.join("link.brep");
        std::os::unix::fs::symlink(&real, &link).expect("symlink creates");
        let target = dir.join("out.brep");

        let error = copy_brep(&link, &target).expect_err("symlinked source must fail closed");
        assert!(
            error.contains("source BREP"),
            "error must name the source; got {error:?}"
        );
        assert!(
            !target.exists(),
            "no file may be promoted from a symlinked source"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_brep_copies_a_regular_source_byte_exactly() {
        let dir = std::env::temp_dir().join(format!(
            "threeterm-host-copy-regular-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir creates");
        let source = dir.join("src.brep");
        std::fs::write(&source, b"verified worker bytes").expect("source writes");
        let target = dir.join("out.brep");

        copy_brep(&source, &target).expect("regular source copies");
        assert_eq!(
            std::fs::read(&target).expect("target reads"),
            b"verified worker bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_brep_rejects_an_oversized_source_without_truncating_the_target() {
        let dir = std::env::temp_dir().join(format!(
            "threeterm-host-copy-atomic-bound-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir creates");
        let source = dir.join("src.brep");
        let target = dir.join("out.brep");
        std::fs::write(
            &source,
            vec![b'x'; threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1],
        )
        .expect("oversized source writes");
        std::fs::write(&target, b"prior canonical bytes").expect("prior target writes");

        let error = copy_brep(&source, &target).expect_err("oversized source must fail closed");

        assert!(
            error.contains("exceeds"),
            "error must name the bound: {error:?}"
        );
        assert_eq!(
            std::fs::read(&target).expect("prior target reads"),
            b"prior canonical bytes",
            "a rejected promotion must preserve the prior target"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

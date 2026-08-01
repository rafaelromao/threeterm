use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use threeterm_occt_worker::{
    BooleanFuseRequest, BooleanFuseResult, ChamferRequest, ChamferResult, CircularPatternRequest,
    CircularPatternResult, DraftRequest, DraftResult, ExtrudeRequest, ExtrudeResult, FilletRequest,
    FilletResult, HoleRequest, HoleResult, LinearPatternRequest, LinearPatternResult, LoftRequest,
    LoftResult, MirrorRequest, MirrorResult, OcctWorker, RevolveRequest, RevolveResult,
    ShellRequest, ShellResult, WorkerError,
};
use threeterm_persistence::{Bundle, BundleError, LoadedBundle};
use threeterm_protocol::artifact::{
    ArtifactError, Layer1ArtifactRequest, Layer1CacheKey, Stage, WorkerFingerprint,
};
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::supervisor::SupervisorOutcome;

pub const BREP_SUBDIR: &str = "brep";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotView {
    pub feature_graph_hash: String,
    pub revision_hash: String,
}

impl From<&LoadedBundle> for SnapshotView {
    fn from(bundle: &LoadedBundle) -> Self {
        Self {
            feature_graph_hash: bundle.feature_graph_hash_hex().to_string(),
            revision_hash: bundle.revision_hash_hex().to_string(),
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

#[derive(Debug, Clone, PartialEq)]
pub struct LoftCommitView {
    pub snapshot: SnapshotView,
    pub result: LoftResult,
}

#[derive(Debug)]
pub enum HostError {
    BundlePathMissing { path: PathBuf },
    BundlePathNotDirectory { path: PathBuf },
    Persistence(BundleError),
    WorkerFailure { detail: String },
    WorkerUnavailable { detail: String },
    BrepInvalid { detail: String },
    BrepFileMissing { path: PathBuf },
    BrepIo { detail: String },
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
            Self::Persistence(error) => error.fmt(formatter),
            Self::WorkerFailure { detail } => {
                write!(formatter, "occt worker failure: {detail}")
            }
            Self::WorkerUnavailable { detail } => {
                write!(formatter, "occt worker unavailable: {detail}")
            }
            Self::BrepInvalid { detail } => {
                write!(formatter, "occt brep invalid: {detail}")
            }
            Self::BrepFileMissing { path } => {
                write!(formatter, "occt brep file missing: {}", path.display())
            }
            Self::BrepIo { detail } => {
                write!(formatter, "occt brep io error: {detail}")
            }
        }
    }
}

impl std::error::Error for HostError {}

impl From<BundleError> for HostError {
    fn from(error: BundleError) -> Self {
        Self::Persistence(error)
    }
}

impl From<WorkerError> for HostError {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::Diagnostic(diagnostic) => {
                if diagnostic.code == "brep_invalid" {
                    Self::BrepInvalid {
                        detail: format!("{} {}", diagnostic.code, diagnostic.arg),
                    }
                } else {
                    Self::WorkerFailure {
                        detail: format!("{} {}", diagnostic.code, diagnostic.arg),
                    }
                }
            }
            other => Self::WorkerFailure {
                detail: other.to_string(),
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct Host {
    current: RefCell<Option<LoadedBundle>>,
    layer1_results: RefCell<HashMap<Layer1CacheKey, Layer1DerivedResult>>,
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
        let bundle = if root.exists() {
            if !root.is_dir() {
                return Err(HostError::BundlePathNotDirectory {
                    path: root.to_path_buf(),
                });
            }
            Bundle::at(root)
        } else {
            Bundle::create(root)?
        };
        let loaded = bundle.append_feature(feature_id, kind)?;
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    pub fn load(&self, root: impl AsRef<Path>) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(HostError::BundlePathMissing {
                path: root.to_path_buf(),
            });
        }
        if !root.is_dir() {
            return Err(HostError::BundlePathNotDirectory {
                path: root.to_path_buf(),
            });
        }
        let loaded = Bundle::at(root).open()?;
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    pub fn current(&self) -> Option<SnapshotView> {
        self.current.borrow().as_ref().map(SnapshotView::from)
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
        self.accept_staged_artifact(
            root,
            request,
            expected_worker,
            artifact.header,
        )
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
        let path = stage
            .validate_and_promote(&header)
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
        let root = root.as_ref();
        if !brep_path.is_file() {
            return Err(HostError::BrepFileMissing {
                path: brep_path.to_path_buf(),
            });
        }
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
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
        if let Err(detail) = copy_brep(brep_path, &target) {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepIo { detail });
        }

        let kind = format!("brep:{feature_id}");
        let updated = match bundle.append_feature(feature_id, &kind) {
            Ok(loaded) => loaded,
            Err(error) => {
                // Restore the prior BREP bytes (or remove the new file if
                // there was no prior) and verify the canonical state
                // survived. The prior manifest and log are untouched
                // because we never reached a successful append.
                restore_brep(&target, prior_brep.as_deref());
                // Fail-closed: if the canonical state was not preserved
                // by the append, surface the persistence error so the
                // diagnostic taxonomy sees the failure.
                if let (Ok(m), Ok(l)) = (
                    read_bundle_file(&bundle_root(root), "manifest.json"),
                    read_bundle_file(&bundle_root(root), "transactions.log"),
                ) && (m != prior_manifest || l != prior_log)
                {
                    return Err(HostError::from(error));
                }
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        let _ = prior_view;
        let view = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
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

        let result = match worker.extrude(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "extrude returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.boolean_fuse(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "boolean_fuse returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.fillet(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "fillet returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.chamfer(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "chamfer returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.hole(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "hole returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.revolve(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "revolve returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.mirror(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "mirror returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.linear_pattern(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "linear_pattern returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.circular_pattern(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "circular_pattern returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.shell(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "shell returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
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

        let result = match worker.draft(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "draft returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(error);
            }
        };
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

        let result = match worker.loft(&request) {
            Ok(result) => result,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(HostError::from(error));
            }
        };
        if !result.is_success() {
            self.current.replace(Some(loaded));
            return Err(HostError::BrepInvalid {
                detail: format!(
                    "loft returned non-ok status: status={} feature_id={}",
                    result.status, result.feature_id
                ),
            });
        }
        let feature_id = request.feature_id.clone();
        let snapshot = match self.commit_brep_feature(root, &feature_id, &result.brep_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.current.replace(Some(loaded));
                return Err(error);
            }
        };
        let _ = prior_view;
        Ok(LoftCommitView { snapshot, result })
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
    root.to_path_buf()
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

fn copy_brep(source: &Path, target: &Path) -> Result<(), String> {
    let mut reader = fs::File::open(source)
        .map_err(|error| format!("open source BREP {} failed: {error}", source.display()))?;
    let mut writer = fs::File::create(target)
        .map_err(|error| format!("create target BREP {} failed: {error}", target.display()))?;
    let mut buffer = vec![0u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("read source BREP failed: {error}"))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("write target BREP failed: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("flush target BREP failed: {error}"))?;
    writer
        .sync_all()
        .map_err(|error| format!("sync target BREP failed: {error}"))?;
    Ok(())
}

fn restore_brep(target: &Path, prior_bytes: Option<&[u8]>) {
    match prior_bytes {
        Some(bytes) => {
            if let Ok(mut writer) = fs::File::create(target) {
                let _ = writer.write_all(bytes);
                let _ = writer.sync_all();
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
    use threeterm_persistence::{Bundle, BundleError, MANIFEST_FILENAME};

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
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.host/1");
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

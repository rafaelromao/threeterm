use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use threeterm_occt_worker::{
    BooleanFuseRequest, BooleanFuseResult, ChamferRequest, ChamferResult, CircularPatternRequest,
    CircularPatternResult, DraftRequest, DraftResult, ExtrudeRequest, ExtrudeResult, FilletRequest,
    FilletResult, HoleRequest, HoleResult, LinearPatternRequest, LinearPatternResult, LoftRequest,
    LoftResult, MirrorRequest, MirrorResult, OcctWorker, RevolveRequest, RevolveResult,
    ShellRequest, ShellResult, WorkerError,
};
use threeterm_persistence::{Bundle, BundleError, LoadedBundle, load, previous_generation_path};
use threeterm_protocol::artifact::{
    ArtifactError, Layer1ArtifactRequest, Layer1CacheKey, Stage, WorkerFingerprint, sha256_hex,
};
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::supervisor::SupervisorOutcome;

pub const BREP_SUBDIR: &str = "brep";

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
    pub operation: String,
    pub feature_id: String,
    pub artifact_kind: String,
    pub artifact_name: String,
    pub byte_count: u64,
    pub sha256: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrudeDerivedResult {
    pub source_snapshot: SnapshotView,
    pub result: ExtrudeResult,
    pub artifact: Layer1DerivedResult,
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
        detail: String,
    },
    WorkerUnavailable {
        detail: String,
    },
    UnsupportedGeometry {
        detail: String,
    },
    BrepInvalid {
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
    DerivedResult {
        diagnostic: Diagnostic,
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
            Self::WorkerFailure { detail } => {
                write!(formatter, "occt worker failure: {detail}")
            }
            Self::WorkerUnavailable { detail } => {
                write!(formatter, "occt worker unavailable: {detail}")
            }
            Self::UnsupportedGeometry { detail } => {
                write!(formatter, "occt unsupported geometry: {detail}")
            }
            Self::BrepInvalid { detail } => {
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
            Self::DerivedResult { diagnostic } => {
                write!(
                    formatter,
                    "derived result rejected: {:?}: {}",
                    diagnostic.code, diagnostic.arg
                )
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
                } else if diagnostic.code == "unsupported_geometry" {
                    Self::UnsupportedGeometry {
                        detail: diagnostic.arg,
                    }
                } else {
                    Self::WorkerFailure {
                        detail: format!("{} {}", diagnostic.code, diagnostic.arg),
                    }
                }
            }
            WorkerError::Supervised { record } => Self::WorkerTerminated { record },
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

    /// Run one extrude through the Host-owned Derived Result boundary. This
    /// captures the source Revision Snapshot and retains a validated result
    /// outside canonical persistence for a later promotion slice.
    pub fn stage_extrude(
        &self,
        root: impl AsRef<Path>,
        request: ExtrudeRequest,
        worker: &OcctWorker,
    ) -> Result<ExtrudeDerivedResult, HostError> {
        let root = root.as_ref();
        let source_snapshot = self.load(root)?;

        let mut binding = extrude_artifact_request(&request, &source_snapshot)?;
        let stage = Stage::create_fresh(root.join(".derived"), "extrude").map_err(|error| {
            HostError::BrepIo {
                detail: format!("create extrude request stage failed: {error}"),
            }
        })?;
        let staging_name = format!("extrude-{}.brep", threeterm_occt_worker::new_request_id());
        binding.staging_name = staging_name.clone();
        let staged_request = request
            .clone()
            .with_output_path(stage.root(), format!("{staging_name}.partial"))
            .with_artifact_request(binding.clone());
        if let Err(detail) = staged_request.validate() {
            let _ = stage.discard();
            return Err(HostError::Validation { detail });
        }

        let completion = match worker
            .clone()
            .with_revision_id(source_snapshot.revision_hash.clone())
            .extrude_staged(&staged_request, stage)
        {
            Ok(completion) => completion,
            Err(error) => return Err(HostError::from(error)),
        };
        let artifact = self
            .accept_staged_extrude(
                completion.stage,
                &binding,
                &completion.result,
                completion.outcome,
            )
            .map_err(|diagnostic| HostError::DerivedResult { diagnostic })?;

        Ok(ExtrudeDerivedResult {
            source_snapshot,
            result: completion.result,
            artifact,
        })
    }

    /// Independently validate a completed staged extrude before the generic
    /// non-authoritative cache link. This seam accepts completed facts and a
    /// typed result separately so neither worker-side validation path can
    /// substitute for Host checks.
    pub fn accept_staged_extrude(
        &self,
        stage: Stage,
        binding: &Layer1ArtifactRequest,
        typed_result: &ExtrudeResult,
        outcome: SupervisorOutcome,
    ) -> Result<Layer1DerivedResult, Diagnostic> {
        let stage_root = stage.root().to_path_buf();
        let reject = |diagnostic| {
            let _ = stage.discard();
            diagnostic
        };
        let SupervisorOutcome::Completed {
            request_id,
            result,
            mut artifact_headers,
        } = outcome
        else {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "worker_result_not_completed",
            )));
        };
        if binding.request_id.is_empty() {
            return Err(reject(Diagnostic::artifact_request_mismatch(
                "empty_artifact_request_id",
            )));
        }
        if binding.operation != "extrude" {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "artifact_request_operation_mismatch",
            )));
        }
        if binding.feature_id.is_empty() {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "empty_artifact_feature_id",
            )));
        }
        if !is_sha256_hex(&binding.source_revision_id) {
            return Err(reject(Diagnostic::artifact_revision_mismatch(
                "invalid_artifact_source_revision",
            )));
        }
        if binding.artifact_kind != "brep" {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "artifact_request_kind_mismatch",
            )));
        }
        if binding.staging_name.is_empty()
            || binding.staging_name.contains('/')
            || binding.staging_name.contains('\\')
            || binding.staging_name.contains('\0')
        {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "artifact_request_staging_name_invalid",
            )));
        }
        if !is_sha256_hex(&binding.semantic_input_sha256)
            || !is_sha256_hex(&binding.deterministic_settings_sha256)
        {
            return Err(reject(Diagnostic::artifact_cache_key_mismatch(
                "invalid_artifact_cache_identity",
            )));
        }
        if request_id != binding.request_id {
            return Err(reject(Diagnostic::artifact_request_mismatch(
                "completed_request_id_mismatch",
            )));
        }
        let outcome_result = match serde_json::from_value::<ExtrudeResult>(result) {
            Ok(result) => result,
            Err(error) => {
                return Err(reject(Diagnostic::artifact_promotion_failure(&format!(
                    "typed_result_schema_mismatch:{error}"
                ))));
            }
        };
        if outcome_result != *typed_result {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "typed_result_does_not_match_completion",
            )));
        }
        if typed_result.schema_version != threeterm_occt_worker::SCHEMA_VERSION {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "typed_result_schema_mismatch",
            )));
        }
        if !typed_result.is_success() {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "typed_result_not_ok",
            )));
        }
        if typed_result.request_id != binding.request_id {
            return Err(reject(Diagnostic::artifact_request_mismatch(
                "typed_result_request_id_mismatch",
            )));
        }
        if typed_result.operation != threeterm_occt_worker::Operation::Extrude
            || binding.operation != "extrude"
        {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "typed_result_operation_mismatch",
            )));
        }
        if typed_result.feature_id != binding.feature_id {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "typed_result_feature_id_mismatch",
            )));
        }
        let expected_path = stage_root.join(format!("{}.partial", binding.staging_name));
        if typed_result.brep_path != expected_path {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "typed_result_path_mismatch",
            )));
        }
        if artifact_headers.len() != 1 {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "expected_exactly_one_artifact",
            )));
        }
        let artifact = artifact_headers
            .pop()
            .expect("checked exactly one artifact");
        if artifact.schema_version != threeterm_protocol::schema_version() {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "artifact_schema_mismatch",
            )));
        }
        if typed_result.brep_bytes as u64 != artifact.header.byte_count
            || typed_result.brep_sha256 != artifact.header.sha256
        {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "typed_result_artifact_metadata_mismatch",
            )));
        }
        let expected_worker = expected_occt_worker_fingerprint();
        if artifact.header.worker_fingerprint != expected_worker {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "artifact_worker_fingerprint_mismatch",
            )));
        }

        match self.accept_staged_artifact(&stage_root, binding, &expected_worker, artifact.header) {
            Ok(result) => Ok(result),
            Err(diagnostic) => Err(reject(diagnostic)),
        }
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
        if header.operation != request.operation || header.feature_id != request.feature_id {
            return Err(reject(Diagnostic::artifact_promotion_failure(
                "artifact_operation_or_feature_id_mismatch",
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
            operation: header.operation,
            feature_id: header.feature_id,
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

fn artifact_error_diagnostic(error: &ArtifactError) -> Diagnostic {
    match error {
        ArtifactError::HashMismatch { expected, actual } => {
            Diagnostic::artifact_hash_mismatch(expected, actual)
        }
        _ => Diagnostic::artifact_promotion_failure(&error.to_string()),
    }
}

fn expected_occt_worker_fingerprint() -> WorkerFingerprint {
    WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: threeterm_occt_worker::SCHEMA_VERSION.to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn extrude_artifact_request(
    request: &ExtrudeRequest,
    source_snapshot: &SnapshotView,
) -> Result<Layer1ArtifactRequest, HostError> {
    let semantic_input = threeterm_protocol::worker::serialize_capped(
        &ExtrudeSemanticInput {
            operation: "extrude",
            feature_id: &request.feature_id,
            profile: &request.profile,
            height: request.height,
        },
        threeterm_protocol::frame::MAX_FRAME_BUFFER,
    )
    .map_err(|error| HostError::Validation {
        detail: format!("extrude semantic input serialization failed: {error}"),
    })?;
    Ok(Layer1ArtifactRequest {
        request_id: request.request_id.clone(),
        source_revision_id: source_snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: request.feature_id.clone(),
        artifact_kind: "brep".to_string(),
        staging_name: String::new(),
        semantic_input_sha256: sha256_hex(&semantic_input),
        deterministic_settings_sha256: sha256_hex(b"threeterm.extrude.derived-settings/1"),
    })
}

#[derive(Debug, Serialize)]
struct ExtrudeSemanticInput<'a> {
    operation: &'static str,
    feature_id: &'a str,
    profile: &'a [[f64; 2]],
    height: f64,
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

    #[test]
    fn extrude_artifact_request_uses_the_pinned_semantic_input_order() {
        let request =
            ExtrudeRequest::new("request-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 2.0)
                .with_feature_id("feature-1");
        let snapshot = SnapshotView {
            feature_graph_hash: "a".repeat(64),
            revision_hash: "b".repeat(64),
            recovered_from_previous: false,
        };
        let binding = extrude_artifact_request(&request, &snapshot).expect("binding derives");
        let expected = br#"{"operation":"extrude","feature_id":"feature-1","profile":[[0.0,0.0],[1.0,0.0],[1.0,1.0]],"height":2.0}"#;

        assert_eq!(
            binding.semantic_input_sha256,
            threeterm_protocol::artifact::sha256_hex(expected)
        );
        assert_eq!(
            binding.deterministic_settings_sha256,
            threeterm_protocol::artifact::sha256_hex(b"threeterm.extrude.derived-settings/1")
        );
    }

    #[test]
    fn extrude_artifact_request_rejects_an_oversized_profile_before_materializing_it() {
        let mut request =
            ExtrudeRequest::new("request-1", vec![(0.0, 0.0); 3], 2.0).with_feature_id("feature-1");
        request.profile = vec![[0.0, 0.0]; threeterm_protocol::frame::MAX_FRAME_BUFFER];
        let snapshot = SnapshotView {
            feature_graph_hash: "a".repeat(64),
            revision_hash: "b".repeat(64),
            recovered_from_previous: false,
        };

        let error = extrude_artifact_request(&request, &snapshot)
            .expect_err("oversized semantic input must fail closed");
        assert!(
            matches!(error, HostError::Validation { ref detail } if detail.contains("serialization failed")),
            "expected bounded serialization failure; got {error:?}"
        );
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

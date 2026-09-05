use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use threeterm_domain::{
    ComponentCommand, ComponentGraph, EdgeReattachmentOutcome, FeatureGraph, FitDimension,
    PlanarFaceCandidate, PlanarFaceReattachmentOutcome, PlanarFaceReference, PostEditEdgeCandidate,
    SelectedEdgeReference, SketchConstraint as DomainSketchConstraint,
    SketchDiagnostic as DomainSketchDiagnostic, SketchEntity as DomainSketchEntity, SketchPayload,
    SolvedCoordinate as DomainSolvedCoordinate,
    history::{HistoryEvaluation, HistorySnapshot, HistoryState, HistoryStatus, HistoryTimeline},
    resolve_edge_reference, resolve_planar_face_reference, resolve_split_edge_reference,
};
use threeterm_occt_worker::{
    BooleanCommonRequest, BooleanCommonResult, BooleanCutRequest, BooleanCutResult,
    BooleanFuseRequest, BooleanFuseResult, BooleanPatternRequest, BooleanPatternResult,
    BracketRequest, BracketResult, ChamferRequest, ChamferResult, CircularPatternRequest,
    CircularPatternResult, DraftRequest, DraftResult, EdgeCandidateEvidence, ExportRequest,
    ExtrudeMode, ExtrudeRequest, ExtrudeResult, FilletRequest, FilletResult, HoleRequest,
    HoleResult, LinearPatternRequest, LinearPatternResult, LoftRequest, LoftResult, MirrorRequest,
    MirrorResult, OcctDiagnostic, OcctWorker, RevolveRequest, RevolveResult, SelectedEdgeContext,
    ShellRequest, ShellResult, SplitRequest, SplitResult, WorkerError,
};
use threeterm_persistence::{
    BOOLEAN_INTENT_SCHEMA_VERSION, Bundle, BundleError, CanonicalBooleanIntent,
    CanonicalExtrudeIntent, CanonicalHoleIntent, CanonicalIntent, EXTRUDE_INTENT_SCHEMA_VERSION,
    ExtrudeDeterministicInputs, HOLE_INTENT_SCHEMA_VERSION, HistoryBrepReplacement,
    HoleDeterministicInputs, LoadPolicy, LoadedBundle, load, load_with_policy,
    previous_generation_path, replay_canonical_state,
};
use threeterm_protocol::artifact::{
    ArtifactError, Layer1ArtifactRequest, Layer1CacheKey, Stage, WorkerFingerprint, sha256_hex,
};
use threeterm_protocol::command_execution::{ExecutionError, execute, validate_request};
use threeterm_protocol::diagnostic::{Diagnostic, DiagnosticCode};
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, BOOLEAN_COMMON_COMMAND_ID, BOOLEAN_CUT_COMMAND_ID, BOOLEAN_FUSE_COMMAND_ID,
    BOOLEAN_PATTERN_COMMAND_ID, CommandId, EXTRUDE_COMMAND_ID, HOLE_COMMAND_ID,
    IDENTITY_COMMAND_ID, SKETCH_SOLVE_COMMAND_ID, find,
};
use threeterm_protocol::supervisor::SupervisorOutcome;
use threeterm_slvs_worker::{SketchSolveRequest, SketchSolveResponse, SlvsWorker};
use threeterm_viewport::{SceneSolid, SceneTriangle, ViewportScene};

pub const BREP_SUBDIR: &str = "brep";
const MAX_VIEWPORT_TESSELLATION_BYTES: u64 = 64 * 1024 * 1024;
static TESSELLATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const LAYER1_CACHE_DIR: &str = "cache";
const LAYER1_CACHE_RECORD: &str = "layer1.json";
const LAYER1_CACHE_SCHEMA: &str = "threeterm.host.layer1-cache/1";

struct ThreeMfBody {
    label: String,
    stl: PathBuf,
}

fn write_3mf(
    bodies: &[ThreeMfBody],
    generation_id: &str,
    revision_id: &str,
    feature_ids: &[String],
    feature_graph_hash: &str,
    revision_hash: &str,
    destination: &Path,
) -> Result<(), HostError> {
    let mut objects = String::new();
    let mut build = String::new();
    for (index, body) in bodies.iter().enumerate() {
        let source = fs::read_to_string(&body.stl).map_err(|error| HostError::BrepIo {
            detail: format!("3MF body {} could not be read: {error}", body.label),
        })?;
        let vertices: Vec<[f64; 3]> = source
            .lines()
            .filter_map(|line| {
                let values: Vec<_> = line.split_whitespace().collect();
                (values.first() == Some(&"vertex") && values.len() == 4)
                    .then(|| {
                        Some([
                            values[1].parse().ok()?,
                            values[2].parse().ok()?,
                            values[3].parse().ok()?,
                        ])
                    })
                    .flatten()
            })
            .collect();
        if vertices.len() < 3 || !vertices.len().is_multiple_of(3) {
            return Err(HostError::BrepInvalid {
                request_id: Some(format!("export-{}", body.label)),
                detail: format!(
                    "3MF body {} has empty or malformed tessellation",
                    body.label
                ),
            });
        }
        let object_id = index + 1;
        objects.push_str(&format!(
            "<object id=\"{object_id}\" type=\"model\" name=\"{}\"><mesh><vertices>",
            xml_escape(&body.label)
        ));
        for vertex in &vertices {
            objects.push_str(&format!(
                "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                vertex[0], vertex[1], vertex[2]
            ));
        }
        objects.push_str("</vertices><triangles>");
        for triangle in (0..vertices.len()).step_by(3) {
            objects.push_str(&format!(
                "<triangle v1=\"{triangle}\" v2=\"{}\" v3=\"{}\"/>",
                triangle + 1,
                triangle + 2
            ));
        }
        objects.push_str("</triangles></mesh></object>");
        build.push_str(&format!("<item objectid=\"{object_id}\"/>",));
    }
    let metadata = [
        ("generation_id", generation_id.to_string()),
        ("revision_id", revision_id.to_string()),
        (
            "feature_ids",
            serde_json::to_string(feature_ids).expect("feature IDs serialize"),
        ),
        ("feature_graph_hash", feature_graph_hash.to_string()),
        ("revision_hash", revision_hash.to_string()),
    ]
    .into_iter()
    .map(|(name, value)| {
        format!(
            "<metadata name=\"threeterm.{name}\">{}</metadata>",
            xml_escape(&value)
        )
    })
    .collect::<String>();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><model unit=\"millimeter\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" xmlns:threeterm=\"https://threeterm.dev/3mf/2026\">{metadata}<resources>{objects}</resources><build>{build}</build></model>"
    );
    if xml.is_empty() {
        return Err(HostError::BrepInvalid {
            request_id: Some("export".to_string()),
            detail: "3MF model is empty".to_string(),
        });
    }
    let content_types = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>";
    let relationships = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>";
    let zip = zip_stored(&[
        ("[Content_Types].xml", content_types.as_slice()),
        ("_rels/.rels", relationships.as_slice()),
        ("3D/3dmodel.model", xml.as_bytes()),
    ]);
    fs::write(destination, zip).map_err(|error| HostError::BrepIo {
        detail: error.to_string(),
    })
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn publish_export_artifacts(staged: &[(PathBuf, PathBuf)]) -> Result<Vec<PathBuf>, HostError> {
    let mut previous = Vec::with_capacity(staged.len());
    for (_, destination) in staged {
        let saved = if destination.is_file() {
            Some(fs::read(destination).map_err(|error| HostError::BrepIo {
                detail: error.to_string(),
            })?)
        } else {
            None
        };
        previous.push((destination.clone(), saved));
    }
    let mut published = Vec::with_capacity(staged.len());
    for (source, destination) in staged {
        if let Some(parent) = destination.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            rollback_export_artifacts(&published, &previous);
            return Err(HostError::BrepIo {
                detail: error.to_string(),
            });
        }
        if let Err(error) = fs::rename(source, destination) {
            rollback_export_artifacts(&published, &previous);
            return Err(HostError::BrepIo {
                detail: error.to_string(),
            });
        }
        published.push(destination.clone());
    }
    Ok(published)
}

fn rollback_export_artifacts(published: &[PathBuf], previous: &[(PathBuf, Option<Vec<u8>>)]) {
    for path in published {
        if let Some((_, Some(bytes))) = previous
            .iter()
            .find(|(previous_path, _)| previous_path == path)
        {
            let _ = fs::write(path, bytes);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

fn prepare_3mf_bodies(
    root: &Path,
    prior: &LoadedBundle,
    body_ids: &[String],
    stage: &Path,
    deflection: f64,
    accept_stale_geometry: bool,
    worker: &OcctWorker,
) -> Result<Vec<ThreeMfBody>, HostError> {
    let mut bodies = Vec::with_capacity(body_ids.len());
    for body_id in body_ids {
        let stale_body_features = stale_last_valid_geometry_for_export(&prior.history, body_id);
        if !accept_stale_geometry && !stale_body_features.is_empty() {
            return Err(HostError::StaleLastValidGeometry {
                feature_id: body_id.clone(),
                active_revision: prior.history.active_snapshot().revision_id.clone(),
                stale_features: stale_body_features,
            });
        }
        if !prior
            .graph
            .features()
            .any(|feature| feature.id.as_str() == body_id)
        {
            return Err(HostError::Validation {
                detail: format!("3MF body is not a canonical feature: {body_id}"),
            });
        }
        let body_brep = bundle_root(root)
            .join(BREP_SUBDIR)
            .join(format!("{body_id}.brep"));
        if !body_brep.is_file() {
            return Err(HostError::BrepFileMissing { path: body_brep });
        }
        let body_request = ExportRequest::new(format!("export-{body_id}"), body_brep, deflection)
            .with_output_path(stage.join("bodies"), format!("{body_id}.stl"))
            .with_feature_id(body_id);
        let body_result = worker
            .clone()
            .export(&body_request)
            .map_err(HostError::from)?;
        if !body_result.is_success() || !body_result.brep_path.is_file() {
            return Err(HostError::BrepInvalid {
                request_id: Some(format!("export-{body_id}")),
                detail: format!("3MF body export did not produce a mesh: {body_id}"),
            });
        }
        bodies.push(ThreeMfBody {
            label: body_id.clone(),
            stl: body_result.brep_path,
        });
    }
    Ok(bodies)
}

fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = Vec::new();
    let mut entries = Vec::new();
    for (name, content) in files {
        let offset = zip.len() as u32;
        let crc = crc32(content);
        let name = name.as_bytes();
        entries.push((offset, crc, name, *content));
        zip.extend_from_slice(&0x04034b50_u32.to_le_bytes());
        zip.extend_from_slice(&20_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(name);
        zip.extend_from_slice(content);
    }
    let central_start = zip.len();
    for (offset, crc, name, content) in &entries {
        zip.extend_from_slice(&0x02014b50_u32.to_le_bytes());
        zip.extend_from_slice(&20_u16.to_le_bytes());
        zip.extend_from_slice(&20_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u32.to_le_bytes());
        zip.extend_from_slice(&offset.to_le_bytes());
        zip.extend_from_slice(name);
    }
    let end = zip.len();
    zip.extend_from_slice(&0x06054b50_u32.to_le_bytes());
    zip.extend_from_slice(&0_u16.to_le_bytes());
    zip.extend_from_slice(&0_u16.to_le_bytes());
    zip.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    zip.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    zip.extend_from_slice(&((end - central_start) as u32).to_le_bytes());
    zip.extend_from_slice(&(central_start as u32).to_le_bytes());
    zip.extend_from_slice(&0_u16.to_le_bytes());
    zip
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

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
    for (index, field) in fields.into_iter().enumerate() {
        let lower = field.to_ascii_lowercase();
        let canonical_staging_name = index == 1
            && lower.starts_with(&format!("{}-", request.operation.to_ascii_lowercase()));
        if (lower.contains("draft") && !canonical_staging_name)
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
    pub generation_id: String,
    pub feature_graph_hash: String,
    pub revision_hash: String,
    pub recovered_from_previous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIdentity {
    pub generation_id: String,
    pub revision_id: String,
    pub feature_graph_hash: String,
    pub revision_hash: String,
    pub transaction_count: usize,
    pub terminal_log_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyView {
    pub operation: String,
    pub feature_id: String,
    pub identity: ProjectIdentity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FitDimensionCommitView {
    pub snapshot: SnapshotView,
    pub fit: FitDimension,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeReattachmentView {
    pub outcome: EdgeReattachmentOutcome,
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub edit_feature_id: String,
    pub committed: bool,
}

impl From<&LoadedBundle> for SnapshotView {
    fn from(bundle: &LoadedBundle) -> Self {
        Self {
            generation_id: bundle.generation.id.clone(),
            feature_graph_hash: bundle.feature_graph_hash_hex().to_string(),
            revision_hash: bundle.revision_hash_hex().to_string(),
            recovered_from_previous: bundle.recovered_from_previous,
        }
    }
}

impl From<&LoadedBundle> for ProjectIdentity {
    fn from(bundle: &LoadedBundle) -> Self {
        Self {
            generation_id: bundle.generation.id.clone(),
            revision_id: bundle.manifest.revision_id.clone(),
            feature_graph_hash: bundle.feature_graph_hash_hex().to_string(),
            revision_hash: bundle.revision_hash_hex().to_string(),
            transaction_count: bundle.log.len(),
            terminal_log_digest: bundle.log.terminal_digest_hex().to_string(),
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
pub struct ExtrudeDerivedResult {
    pub source_snapshot: SnapshotView,
    pub result: ExtrudeResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainCommandPreview {
    pub command: CommandId,
    pub source_revision: String,
    pub preview_revision: String,
    pub input_fingerprint: String,
    pub geometry_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StagedOcctResult<R> {
    pub source_snapshot: SnapshotView,
    pub request: serde_json::Value,
    pub result: R,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtrudeCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: ExtrudeResult,
    pub worker_fingerprint: WorkerFingerprint,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BooleanFuseCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: BooleanFuseResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BooleanCutCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: BooleanCutResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BooleanCommonCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: BooleanCommonResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilletCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: FilletResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChamferCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: ChamferResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoleCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: HoleResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RevolveCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: RevolveResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirrorCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: MirrorResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinearPatternCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: LinearPatternResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CircularPatternCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: CircularPatternResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BooleanPatternCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: BooleanPatternResult,
    pub artifact: Layer1DerivedResult,
    pub identity: ProjectIdentity,
}

impl BooleanPatternCommitView {
    pub fn response_value(&self, schema_version: &str) -> serde_json::Value {
        serde_json::json!({
            "status": self.result.status,
            "operation": self.result.operation,
            "feature_id": self.result.feature_id,
            "request_id": self.result.request_id,
            "cut_count": self.result.cut_count,
            "source_snapshot": {
                "feature_graph_hash": self.source_snapshot.feature_graph_hash,
                "revision_hash": self.source_snapshot.revision_hash,
            },
            "generation_id": self.identity.generation_id,
            "revision_id": self.identity.revision_id,
            "feature_graph_hash": self.identity.feature_graph_hash,
            "revision_hash": self.identity.revision_hash,
            "transaction_count": self.identity.transaction_count,
            "terminal_log_digest": self.identity.terminal_log_digest,
            "authoritative": true,
            "artifact_kind": self.artifact.artifact_kind,
            "artifact_name": self.artifact.artifact_name,
            "brep_path": self.result.brep_path,
            "brep_sha256": self.result.brep_sha256,
            "brep_bytes": self.result.brep_bytes,
            "worker_fingerprint": self.artifact.worker_fingerprint,
            "derived_result": {
                "request_id": self.artifact.request_id,
                "operation": self.artifact.operation,
                "feature_id": self.artifact.feature_id,
                "source_revision_id": self.artifact.source_revision_id,
                "worker_fingerprint": self.artifact.worker_fingerprint,
                "artifact_kind": self.artifact.artifact_kind,
                "artifact_name": self.artifact.artifact_name,
                "byte_count": self.artifact.byte_count,
                "sha256": self.artifact.sha256,
            },
            "schema_version": schema_version,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShellCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: ShellResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DraftCommitView {
    pub source_snapshot: Option<SnapshotView>,
    pub snapshot: SnapshotView,
    pub result: DraftResult,
    pub artifact: Option<Layer1DerivedResult>,
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
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: BracketResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BracketDraftCommitView {
    pub snapshot: SnapshotView,
    pub input_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryCommitView {
    pub snapshot: SnapshotView,
    pub history: HistoryState,
    pub evaluation: Option<HistoryEvaluation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTimelineView {
    pub snapshot: SnapshotView,
    pub timeline: HistoryTimeline,
}

/// Shared history-commit response shared by every adapter.
///
/// The CLI, MCP, and TUI boundaries all serialize cursor movement and
/// historical edits through this single builder so the wire shape never
/// depends on which adapter executed the command.
pub fn history_commit_value(
    operation: &str,
    schema_version: &str,
    view: &HistoryCommitView,
) -> serde_json::Value {
    let active = view.history.active_snapshot();
    let diagnostics: Vec<_> = active
        .features
        .values()
        .filter_map(|feature| feature.diagnostic.clone())
        .collect();
    let named_revisions: Vec<_> = view
        .history
        .named_revisions()
        .values()
        .map(|revision| {
            serde_json::json!({
                "name": revision.name,
                "revision_id": revision.snapshot.revision_id,
                "provenance": revision.provenance,
            })
        })
        .collect();
    let features: Vec<_> = active
        .features
        .values()
        .map(|feature| {
            let mut value = serde_json::json!({
                "id": feature.id,
                "status": history_status_name(feature.status),
                "geometry_fingerprint": feature.geometry_fingerprint.clone().unwrap_or_default(),
                "last_valid_geometry_fingerprint": feature
                    .last_valid_geometry_fingerprint
                    .clone()
                    .unwrap_or_default(),
                "stale_last_valid_geometry": feature.last_valid_geometry_fingerprint.is_some(),
            });
            if let Some(diagnostic) = &feature.diagnostic {
                value["diagnostic"] = serde_json::json!(diagnostic);
            }
            value
        })
        .collect();
    let (dirty_features, evaluated_features, blocked_features) =
        view.evaluation.as_ref().map_or_else(
            || (Vec::new(), Vec::new(), Vec::new()),
            |evaluation| {
                (
                    evaluation.dirty_features.clone(),
                    evaluation.evaluated_features.clone(),
                    evaluation.blocked_features.clone(),
                )
            },
        );
    let degraded = active.features.values().any(|feature| {
        matches!(
            feature.status,
            HistoryStatus::Broken | HistoryStatus::BlockedByFailure
        )
    });
    serde_json::json!({
        "status": if degraded { "degraded" } else { "ok" },
        "operation": operation,
        "active_revision": active.revision_id,
        "dirty_features": dirty_features,
        "evaluated_features": evaluated_features,
        "blocked_features": blocked_features,
        "diagnostics": diagnostics,
        "named_revisions": named_revisions,
        "features": features,
        "feature_graph_hash": view.snapshot.feature_graph_hash,
        "revision_hash": view.snapshot.revision_hash,
        "schema_version": schema_version,
    })
}

fn history_status_name(status: HistoryStatus) -> &'static str {
    match status {
        HistoryStatus::CurrentValid => "current-valid",
        HistoryStatus::Broken => "broken",
        HistoryStatus::BlockedByFailure => "blocked-by-failure",
        HistoryStatus::Suppressed => "suppressed",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayVerification {
    pub deterministic: bool,
    pub fingerprint: String,
    pub model_state_fingerprint: String,
    pub geometry_fingerprints: Vec<String>,
    pub mismatch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtrudeReplayView {
    pub snapshot: SnapshotView,
    pub model_state_fingerprint: String,
    pub recomputed: usize,
    pub feature_ids: Vec<String>,
    pub geometry_fingerprints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleLastValidGeometryEntry {
    pub feature_id: String,
    pub status: String,
    pub last_valid_geometry_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleLastValidGeometryAcceptance {
    pub feature_id: String,
    pub active_revision: String,
    pub stale_features: Vec<StaleLastValidGeometryEntry>,
}

/// A validated worker result retained for inspection after its source
/// Revision Snapshot stopped being current. The bytes are deliberately
/// session-owned and never participate in canonical persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedStaleLastValidGeometry {
    pub source_revision_id: String,
    pub current_revision_id: String,
    pub feature_id: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub authoritative: bool,
    pub diagnostic_code: DiagnosticCode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Layer1CacheRecord {
    pub schema_version: String,
    pub source_revision: String,
    pub operation: String,
    pub feature_id: String,
    pub worker_fingerprint: WorkerFingerprint,
    pub artifact_name: String,
    pub byte_count: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer1CacheRebuild {
    pub record: Layer1CacheRecord,
    pub recomputations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportCommitView {
    pub source_snapshot: SnapshotView,
    pub artifacts: Vec<PathBuf>,
    pub derived_artifacts: Vec<ExportDerivedArtifact>,
    pub stale_last_valid_geometry_acceptance: StaleLastValidGeometryAcceptance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportDerivedArtifact {
    pub request_id: String,
    pub source_revision_id: String,
    pub operation: String,
    pub feature_id: String,
    pub artifact_kind: String,
    pub artifact_name: String,
    pub byte_count: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoftCommitView {
    pub source_snapshot: SnapshotView,
    pub snapshot: SnapshotView,
    pub result: LoftResult,
    pub artifact: Layer1DerivedResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SketchSolveCommitView {
    pub snapshot: SnapshotView,
    pub result: SketchSolveResponse,
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
    StaleLastValidGeometry {
        feature_id: String,
        active_revision: String,
        stale_features: Vec<StaleLastValidGeometryEntry>,
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
        source_revision: String,
        current_revision: String,
        recovery: &'static str,
    },
    DraftInvalid {
        draft_id: String,
        detail: String,
    },
    DraftInputConflict {
        draft_id: String,
        source_revision: String,
        current_revision: String,
        recovery: &'static str,
    },
    DraftSequenceConflict {
        draft_id: String,
        expected: u64,
        current: u64,
    },
    DraftIdempotencyConflict {
        draft_id: String,
        source_revision: String,
        current_revision: String,
        recovery: &'static str,
    },
    DraftUnknownOutcome {
        draft_id: String,
        source_revision: String,
        current_revision: String,
        recovery: &'static str,
    },
    DerivedResult {
        diagnostic: Diagnostic,
    },
    Layer1FingerprintMismatch {
        expected: Box<WorkerFingerprint>,
        found: Box<WorkerFingerprint>,
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
            Self::StaleLastValidGeometry {
                feature_id,
                active_revision,
                stale_features,
            } => write!(
                formatter,
                "stale last-valid geometry requires explicit acceptance for {feature_id} at {active_revision}: {} stale features",
                stale_features.len()
            ),
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
            Self::DerivedResult { diagnostic } => {
                write!(
                    formatter,
                    "derived result rejected: {:?}: {}",
                    diagnostic.code, diagnostic.arg
                )
            }
            Self::Layer1FingerprintMismatch { expected, found } => write!(
                formatter,
                "LAYER_1_FINGERPRINT_MISMATCH: expected={expected:?} found={found:?}"
            ),
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
                source_revision,
                current_revision,
                recovery,
            } => write!(
                formatter,
                "command draft {draft_id} source {source_feature_id} changed: source_revision={source_revision} current_revision={current_revision} recovery={recovery}"
            ),
            Self::DraftInvalid { draft_id, detail } => {
                write!(formatter, "command draft {draft_id} is invalid: {detail}")
            }
            Self::DraftInputConflict {
                draft_id,
                source_revision,
                current_revision,
                recovery,
            } => write!(
                formatter,
                "command draft {draft_id} input conflicts: source_revision={source_revision} current_revision={current_revision} recovery={recovery}"
            ),
            Self::DraftSequenceConflict {
                draft_id,
                expected,
                current,
            } => write!(
                formatter,
                "command draft {draft_id} update conflicts: expected_sequence={expected} current_sequence={current}"
            ),
            Self::DraftIdempotencyConflict {
                draft_id,
                source_revision,
                current_revision,
                recovery,
            } => write!(
                formatter,
                "command draft {draft_id} idempotency key conflicts: source_revision={source_revision} current_revision={current_revision} recovery={recovery}"
            ),
            Self::DraftUnknownOutcome {
                draft_id,
                source_revision,
                current_revision,
                recovery,
            } => write!(
                formatter,
                "command draft {draft_id} has unknown publication outcome: source_revision={source_revision} current_revision={current_revision} recovery={recovery}"
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
                reason,
                last_progress,
                elapsed,
                stderr_tail,
                exit_signal,
                exit_code,
            } => Self::WorkerTerminated {
                record: Box::new(threeterm_protocol::supervisor::TerminationRecord {
                    request_id: request_id.clone(),
                    stage: "cancelled".to_string(),
                    cancel_reason: Some(reason),
                    elapsed,
                    last_progress,
                    last_artifact_error: None,
                    exit_signal,
                    exit_code,
                    stderr_tail,
                    failed_code: None,
                    failed_detail: None,
                    protocol_diagnostic: None,
                    termination_error: None,
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

impl From<threeterm_slvs_worker::WorkerError> for HostError {
    fn from(error: threeterm_slvs_worker::WorkerError) -> Self {
        Self::WorkerFailure {
            request_id: None,
            detail: error.to_string(),
        }
    }
}

fn sketch_payload(
    request: &SketchSolveRequest,
    result: &SketchSolveResponse,
) -> Result<SketchPayload, HostError> {
    let entities = request
        .entities
        .iter()
        .map(|entity| match entity {
            threeterm_slvs_worker::SketchEntity::Point { id, x, y } => DomainSketchEntity::Point {
                id: id.clone(),
                x: *x,
                y: *y,
            },
            threeterm_slvs_worker::SketchEntity::LineSegment { id, start, end } => {
                DomainSketchEntity::LineSegment {
                    id: id.clone(),
                    start: start.clone(),
                    end: end.clone(),
                }
            }
            threeterm_slvs_worker::SketchEntity::Circle { id, center, radius } => {
                DomainSketchEntity::Circle {
                    id: id.clone(),
                    center: center.clone(),
                    radius: *radius,
                }
            }
            threeterm_slvs_worker::SketchEntity::Arc {
                id,
                center,
                start,
                end,
            } => DomainSketchEntity::Arc {
                id: id.clone(),
                center: center.clone(),
                start: start.clone(),
                end: end.clone(),
            },
        })
        .collect();
    let constraints = request
        .constraints
        .iter()
        .map(|constraint| DomainSketchConstraint {
            id: constraint.id.clone(),
            kind: constraint.kind.clone(),
            entities: constraint.entities.clone(),
            value: constraint.value,
        })
        .collect();
    let diagnostics = result
        .diagnostics
        .iter()
        .map(|diagnostic| DomainSketchDiagnostic {
            code: diagnostic.code.clone(),
            detail: diagnostic.detail.clone(),
            constraint_ids: diagnostic.constraint_ids.clone(),
        })
        .collect();
    let solved_coordinates = result.solved_coordinates.as_ref().map(|coordinates| {
        coordinates
            .iter()
            .map(|coordinate| DomainSolvedCoordinate {
                entity_id: coordinate.entity_id.clone(),
                x: coordinate.x,
                y: coordinate.y,
            })
            .collect()
    });
    let payload = SketchPayload {
        feature_id: request.feature_id.clone(),
        entities,
        constraints,
        status: result.status.clone(),
        dof: result.dof,
        entity_ids: result.entity_ids.clone(),
        related_constraint_ids: result.related_constraint_ids.clone(),
        diagnostics,
        solved_coordinates,
        support: request.support.clone(),
        placement: request.placement,
    };
    payload
        .validate()
        .map_err(|detail| HostError::Validation { detail })?;
    Ok(payload)
}

fn sketch_request_from_value(
    request: &serde_json::Value,
    _loaded: &LoadedBundle,
) -> Result<SketchSolveRequest, HostError> {
    let request_id = request
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("sketch-request")
        .to_string();
    let feature_id = request
        .get("feature_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| HostError::Validation {
            detail: "missing sketch feature_id".to_string(),
        })?;
    let entities = serde_json::from_value(request.get("entities").cloned().ok_or_else(|| {
        HostError::Validation {
            detail: "missing sketch entities".to_string(),
        }
    })?)
    .map_err(|error| HostError::Validation {
        detail: format!("invalid sketch entities: {error}"),
    })?;
    let constraints =
        serde_json::from_value(request.get("constraints").cloned().ok_or_else(|| {
            HostError::Validation {
                detail: "missing sketch constraints".to_string(),
            }
        })?)
        .map_err(|error| HostError::Validation {
            detail: format!("invalid sketch constraints: {error}"),
        })?;
    let mut typed = SketchSolveRequest::new(request_id, feature_id, entities, constraints)
        .with_source_revision(
            request
                .get("source_revision")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );
    let support = request
        .get("support")
        .cloned()
        .map(serde_json::from_value::<PlanarFaceReference>)
        .transpose()
        .map_err(|error| HostError::Validation {
            detail: format!("invalid sketch support: {error}"),
        })?;
    let placement = request
        .get("placement")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| HostError::Validation {
            detail: format!("invalid sketch placement: {error}"),
        })?;
    match (support, placement) {
        (Some(support), Some(placement)) => {
            typed = typed.with_attachment(support, placement);
        }
        (None, None) => {}
        _ => {
            return Err(HostError::Validation {
                detail: "sketch support and placement must be provided together".to_string(),
            });
        }
    }
    typed
        .validate()
        .map_err(|detail| HostError::Validation { detail })?;
    Ok(typed)
}

fn sketch_request_from_payload(
    payload: &SketchPayload,
    source_revision: &str,
) -> Result<SketchSolveRequest, HostError> {
    let entities =
        serde_json::from_value(serde_json::to_value(&payload.entities).map_err(|error| {
            HostError::Validation {
                detail: format!("sketch entities serialization failed: {error}"),
            }
        })?)
        .map_err(|error| HostError::Validation {
            detail: format!("sketch entities conversion failed: {error}"),
        })?;
    let constraints =
        serde_json::from_value(serde_json::to_value(&payload.constraints).map_err(|error| {
            HostError::Validation {
                detail: format!("sketch constraints serialization failed: {error}"),
            }
        })?)
        .map_err(|error| HostError::Validation {
            detail: format!("sketch constraints conversion failed: {error}"),
        })?;
    let mut request = SketchSolveRequest::new(
        format!("reload-{}", payload.feature_id),
        payload.feature_id.clone(),
        entities,
        constraints,
    )
    .with_source_revision(source_revision);
    if let (Some(support), Some(placement)) = (&payload.support, payload.placement) {
        request = request.with_attachment(support.clone(), placement);
    }
    request
        .validate()
        .map_err(|detail| HostError::Validation { detail })?;
    Ok(request)
}

fn resolve_sketch_support(
    loaded: &LoadedBundle,
    request: &SketchSolveRequest,
) -> Result<PlanarFaceReattachmentOutcome, HostError> {
    let Some(reference) = &request.support else {
        return Ok(PlanarFaceReattachmentOutcome::Resolved {
            semantic_id: String::new(),
        });
    };
    if reference.provenance.source_revision_id != loaded.revision_hash_hex()
        || !loaded
            .graph
            .contains_feature(&reference.provenance.source_feature_id)
    {
        return Ok(PlanarFaceReattachmentOutcome::Lost);
    }
    let feature_kind = loaded
        .graph
        .features()
        .find(|feature| feature.id.as_str() == reference.provenance.source_feature_id)
        .map(|feature| feature.kind)
        .unwrap_or_default();
    if !(feature_kind.starts_with("brep:")
        || feature_kind.starts_with("bracket:")
        || feature_kind == "brep")
    {
        return Ok(PlanarFaceReattachmentOutcome::Incompatible {
            candidate_ids: Vec::new(),
        });
    }
    let candidate = PlanarFaceCandidate {
        semantic_id: reference.semantic_id.clone(),
        provenance: reference.provenance.clone(),
        role: reference.role.clone(),
        evidence: reference.evidence.clone(),
    };
    Ok(resolve_planar_face_reference(reference, [candidate]))
}

fn reattachment_outcome_name(outcome: &PlanarFaceReattachmentOutcome) -> &'static str {
    match outcome {
        PlanarFaceReattachmentOutcome::Resolved { .. } => "resolved",
        PlanarFaceReattachmentOutcome::Ambiguous { .. } => "ambiguous",
        PlanarFaceReattachmentOutcome::Lost => "lost",
        PlanarFaceReattachmentOutcome::Incompatible { .. } => "incompatible",
    }
}

fn sketch_support_failure_response(
    request: &SketchSolveRequest,
    outcome: &PlanarFaceReattachmentOutcome,
) -> SketchSolveResponse {
    let candidate_ids = match outcome {
        PlanarFaceReattachmentOutcome::Ambiguous { candidate_ids }
        | PlanarFaceReattachmentOutcome::Incompatible { candidate_ids } => candidate_ids.clone(),
        PlanarFaceReattachmentOutcome::Resolved { .. } | PlanarFaceReattachmentOutcome::Lost => {
            Vec::new()
        }
    };
    SketchSolveResponse {
        schema_version: threeterm_slvs_worker::SCHEMA_VERSION.to_string(),
        request_id: request.request_id.clone(),
        operation: "sketch_solve".to_string(),
        feature_id: request.feature_id.clone(),
        status: "invalid_request".to_string(),
        dof: 0,
        entity_ids: request
            .entities
            .iter()
            .map(|entity| entity.id().to_string())
            .collect(),
        related_constraint_ids: candidate_ids.clone(),
        diagnostics: vec![threeterm_slvs_worker::SketchDiagnostic {
            code: format!("sketch_support_{}", reattachment_outcome_name(outcome)),
            detail: format!("planar face reattachment outcome: {outcome:?}"),
            constraint_ids: candidate_ids,
        }],
        solved_coordinates: None,
        support: request.support.clone(),
        placement: request.placement,
        reattachment_outcome: Some(reattachment_outcome_name(outcome).to_string()),
    }
}

fn sketch_dimension_value(
    graph: &FeatureGraph,
    feature_id: &str,
    dimension_id: &str,
) -> Result<f64, HostError> {
    let sketch = graph
        .sketch(feature_id)
        .ok_or_else(|| HostError::Validation {
            detail: format!("fit dimension sketch is missing: {feature_id}"),
        })?;
    let constraint = sketch
        .constraints
        .iter()
        .find(|constraint| constraint.id == dimension_id)
        .ok_or_else(|| HostError::Validation {
            detail: format!("fit dimension constraint is missing: {feature_id}/{dimension_id}"),
        })?;
    if constraint.kind != "distance" {
        return Err(HostError::Validation {
            detail: format!("fit dimension constraint is not a distance: {dimension_id}"),
        });
    }
    let value = constraint.value.ok_or_else(|| HostError::Validation {
        detail: format!("fit dimension constraint has no value: {dimension_id}"),
    })?;
    if !value.is_finite() || value <= 0.0 {
        return Err(HostError::Validation {
            detail: format!("fit dimension constraint value is invalid: {dimension_id}"),
        });
    }
    Ok(value)
}

#[derive(Debug, Default)]
pub struct Host {
    current: RefCell<Option<LoadedBundle>>,
    layer1_results: RefCell<HashMap<Layer1CacheKey, Layer1DerivedResult>>,
    stale_last_valid_geometry: RefCell<HashMap<String, RetainedStaleLastValidGeometry>>,
    drafts: RefCell<HashMap<(PathBuf, String), CommandDraft>>,
    bracket_drafts: RefCell<HashMap<(PathBuf, String), BracketParameterDraft>>,
}

fn draft_map_key(root: &Path, draft_id: &str) -> (PathBuf, String) {
    (root.to_path_buf(), draft_id.to_string())
}

fn domain_edge_candidates(result: &[EdgeCandidateEvidence]) -> Vec<PostEditEdgeCandidate> {
    result
        .iter()
        .map(|candidate| PostEditEdgeCandidate {
            semantic_id: candidate.semantic_id.clone(),
            provenance: threeterm_domain::EdgeProvenance {
                source_feature_id: candidate.source_feature_id.clone(),
                source_revision_id: candidate.source_revision_id.clone(),
                source_edge_id: candidate.source_edge_id.clone(),
            },
            role: candidate.role.clone(),
            evidence: threeterm_domain::EdgeGeometricEvidence {
                midpoint: candidate.midpoint,
                tangent: candidate.tangent,
                length: candidate.length,
            },
        })
        .collect()
}

impl Host {
    #[allow(clippy::too_many_arguments)]
    pub fn export(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        formats: &[String],
        output_dir: &Path,
        deflection: f64,
        override_warnings: bool,
        accept_stale_geometry: bool,
        body_ids: &[String],
    ) -> Result<ExportCommitView, HostError> {
        // Export outputs are disposable Derived Results, not canonical BREP
        // mutations. They use the same host-owned private staging and atomic
        // publication discipline, but remain outside the transaction log.
        if deflection > 0.5 && !override_warnings {
            return Err(HostError::Validation {
                detail: format!(
                    "{{\"severity\":\"warning\",\"code\":\"coarse_tessellation\",\"affected_feature_id\":\"{feature_id}\",\"recovery\":\"use --override-warnings or deflection <= 0.5\",\"override_eligible\":true}}"
                ),
            });
        }
        let root = root.as_ref();
        let unique_formats = formats.iter().map(String::as_str).collect::<BTreeSet<_>>();
        if unique_formats.len() != formats.len() {
            return Err(HostError::Validation {
                detail: "duplicate export format".to_string(),
            });
        }
        let mut prior = Bundle::at(root).open()?;
        let brep = bundle_root(root)
            .join(BREP_SUBDIR)
            .join(format!("{feature_id}.brep"));
        if !brep.is_file() {
            self.load_with_extrude_replay(root)?;
            prior = Bundle::at(root).open()?;
        }
        let stale_features = stale_last_valid_geometry_for_export(&prior.history, feature_id);
        if !stale_features.is_empty() && !accept_stale_geometry {
            return Err(HostError::StaleLastValidGeometry {
                feature_id: feature_id.to_string(),
                active_revision: prior.history.active_snapshot().revision_id.clone(),
                stale_features,
            });
        }
        if !brep.is_file() {
            return Err(HostError::BrepFileMissing { path: brep });
        }
        if formats
            .iter()
            .any(|format| !matches!(format.as_str(), "stl" | "3mf" | "step"))
        {
            return Err(HostError::Validation {
                detail: "unsupported export format".to_string(),
            });
        }
        let body_ids = if body_ids.is_empty() {
            vec![feature_id.to_string()]
        } else {
            body_ids.to_vec()
        };
        let unique_bodies = body_ids.iter().collect::<BTreeSet<_>>();
        if unique_bodies.len() != body_ids.len() || body_ids.iter().any(String::is_empty) {
            return Err(HostError::Validation {
                detail: "duplicate or empty 3MF body ID".to_string(),
            });
        }
        let stage = output_dir.join(format!(".threeterm-export-{}", std::process::id()));
        fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
            detail: error.to_string(),
        })?;
        let request = ExportRequest::new("export", brep, deflection)
            .with_output_path(&stage, format!("{feature_id}.stl"))
            .with_feature_id(feature_id);
        let worker = OcctWorker::locate()
            .map_err(|error| {
                let _ = fs::remove_dir_all(&stage);
                HostError::from(error)
            })?
            .with_revision_id(prior.revision_hash_hex());
        let result = worker.export(&request).map_err(|error| {
            let _ = fs::remove_dir_all(&stage);
            HostError::from(error)
        })?;
        if !result.is_success() || !result.step_path.is_file() {
            let _ = fs::remove_dir_all(&stage);
            return Err(HostError::BrepInvalid {
                request_id: Some("export".to_string()),
                detail: "export worker did not produce validated artifacts".to_string(),
            });
        }
        let bodies = if formats.iter().any(|format| format == "3mf") {
            prepare_3mf_bodies(
                root,
                &prior,
                &body_ids,
                &stage,
                deflection,
                accept_stale_geometry,
                &worker,
            )
            .inspect_err(|_| {
                let _ = fs::remove_dir_all(&stage);
            })?
        } else {
            Vec::new()
        };
        if formats.iter().any(|format| format == "3mf") {
            let feature_ids = prior
                .generation
                .revisions
                .last()
                .map(|revision| {
                    revision
                        .features
                        .iter()
                        .map(|feature_id| feature_id.as_str().to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            write_3mf(
                &bodies,
                &prior.generation.id,
                &prior
                    .generation
                    .revisions
                    .last()
                    .map_or_else(|| "revision-0".to_string(), |revision| revision.id.clone()),
                &feature_ids,
                prior.feature_graph_hash_hex(),
                prior.revision_hash_hex(),
                &stage.join(format!("{feature_id}.3mf")),
            )
            .inspect_err(|_| {
                let _ = fs::remove_dir_all(&stage);
            })?;
        }
        let staged_artifacts = formats
            .iter()
            .map(|format| {
                let source = match format.as_str() {
                    "stl" => result.brep_path.clone(),
                    "step" => result.step_path.clone(),
                    "3mf" => stage.join(format!("{feature_id}.3mf")),
                    _ => unreachable!(),
                };
                (source, output_dir.join(format!("{feature_id}.{format}")))
            })
            .collect::<Vec<_>>();
        let derived_artifacts = staged_artifacts
            .iter()
            .zip(formats)
            .map(|((source, destination), format)| {
                let byte_count = fs::metadata(source)
                    .map_err(|error| HostError::BrepIo {
                        detail: format!("read staged export metadata failed: {error}"),
                    })?
                    .len();
                let sha256 = sha256_path(source).map_err(|error| HostError::BrepIo {
                    detail: format!("hash staged export failed: {error}"),
                })?;
                Ok(ExportDerivedArtifact {
                    request_id: "export".to_string(),
                    source_revision_id: prior.revision_hash_hex().to_string(),
                    operation: "export".to_string(),
                    feature_id: feature_id.to_string(),
                    artifact_kind: format.to_string(),
                    artifact_name: destination
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    byte_count,
                    sha256,
                })
            })
            .collect::<Result<Vec<_>, HostError>>()?;
        let artifacts = publish_export_artifacts(&staged_artifacts).inspect_err(|_| {
            let _ = fs::remove_dir_all(&stage);
        })?;
        let _ = fs::remove_dir_all(stage);
        Ok(ExportCommitView {
            source_snapshot: SnapshotView::from(&prior),
            artifacts,
            derived_artifacts,
            stale_last_valid_geometry_acceptance: StaleLastValidGeometryAcceptance {
                feature_id: feature_id.to_string(),
                active_revision: prior.history.active_snapshot().revision_id.clone(),
                stale_features,
            },
        })
    }
    pub fn new() -> Self {
        Self::default()
    }

    pub fn preview_sketch_solve(
        &self,
        root: impl AsRef<Path>,
        request: &SketchSolveRequest,
    ) -> Result<SketchSolveResponse, HostError> {
        let loaded = Bundle::at(root.as_ref()).open()?;
        let source_revision = if request.source_revision.is_empty() {
            loaded.revision_hash_hex().to_string()
        } else {
            request.source_revision.clone()
        };
        if source_revision != loaded.revision_hash_hex() {
            return Err(HostError::Validation {
                detail: format!(
                    "sketch solve source revision {source_revision:?} does not match current revision {:?}",
                    loaded.revision_hash_hex()
                ),
            });
        }
        let request = request
            .clone()
            .with_source_revision(source_revision.clone());
        request
            .validate()
            .map_err(|detail| HostError::Validation { detail })?;
        let support_outcome = resolve_sketch_support(&loaded, &request)?;
        if !matches!(
            support_outcome,
            PlanarFaceReattachmentOutcome::Resolved { .. }
        ) {
            return Ok(sketch_support_failure_response(&request, &support_outcome));
        }
        let mut result = SlvsWorker::locate()?
            .with_revision_id(source_revision)
            .solve(&request)
            .map_err(HostError::from)?;
        result.support = request.support;
        result.placement = request.placement;
        result.reattachment_outcome = Some("resolved".to_string());
        Ok(result)
    }

    pub fn commit_sketch_solve(
        &self,
        root: impl AsRef<Path>,
        request: &SketchSolveRequest,
    ) -> Result<SketchSolveCommitView, HostError> {
        let worker = SlvsWorker::locate()?;
        self.commit_sketch_solve_with_worker(root, request, &worker)
    }

    /// Recompute a canonical sketch from its durable intent after disposable
    /// solver results have been removed. No transaction or derived artifact is
    /// published by this method.
    pub fn reload_sketch(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
    ) -> Result<SketchSolveResponse, HostError> {
        let worker = SlvsWorker::locate()?;
        self.reload_sketch_with_worker(root, feature_id, &worker)
    }

    pub fn reload_sketch_with_worker(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        worker: &SlvsWorker,
    ) -> Result<SketchSolveResponse, HostError> {
        let loaded = Bundle::at(root.as_ref()).open()?;
        let payload = loaded
            .graph
            .sketch(feature_id)
            .ok_or_else(|| HostError::Validation {
                detail: format!("sketch is missing: {feature_id}"),
            })?;
        let request = sketch_request_from_payload(payload, loaded.revision_hash_hex())?;
        let outcome = resolve_sketch_support(&loaded, &request)?;
        if !matches!(outcome, PlanarFaceReattachmentOutcome::Resolved { .. }) {
            return Ok(sketch_support_failure_response(&request, &outcome));
        }
        let mut result = worker
            .clone()
            .with_revision_id(loaded.revision_hash_hex())
            .solve(&request)
            .map_err(HostError::from)?;
        result.support = request.support;
        result.placement = request.placement;
        result.reattachment_outcome = Some("resolved".to_string());
        Ok(result)
    }

    pub fn commit_sketch_solve_with_worker(
        &self,
        root: impl AsRef<Path>,
        request: &SketchSolveRequest,
        worker: &SlvsWorker,
    ) -> Result<SketchSolveCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let source_revision = if request.source_revision.is_empty() {
            loaded.revision_hash_hex().to_string()
        } else {
            request.source_revision.clone()
        };
        if source_revision != loaded.revision_hash_hex() {
            return Err(HostError::Validation {
                detail: format!(
                    "sketch solve source revision {source_revision:?} does not match current revision {:?}",
                    loaded.revision_hash_hex()
                ),
            });
        }
        let request = request
            .clone()
            .with_source_revision(source_revision.clone());
        request
            .validate()
            .map_err(|detail| HostError::Validation { detail })?;
        let support_outcome = resolve_sketch_support(&loaded, &request)?;
        if !matches!(
            support_outcome,
            PlanarFaceReattachmentOutcome::Resolved { .. }
        ) {
            return Err(HostError::Validation {
                detail: serde_json::to_string(&sketch_support_failure_response(
                    &request,
                    &support_outcome,
                ))
                .expect("sketch support response serializes"),
            });
        }
        let result = worker
            .clone()
            .with_revision_id(source_revision.clone())
            .solve(&request)
            .map_err(HostError::from)?;
        if !result.is_success() {
            return Err(HostError::Validation {
                detail: serde_json::to_string(&result).expect("sketch result serializes"),
            });
        }
        let payload = sketch_payload(&request, &result)?;
        let updated = match bundle.append_sketch_if_revision(&payload, &source_revision) {
            Ok(updated) => updated,
            Err(error) => {
                if let Ok(reopened) = bundle.open() {
                    self.current.replace(Some(reopened));
                }
                return Err(error.into());
            }
        };
        let snapshot = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        Ok(SketchSolveCommitView { snapshot, result })
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

    pub fn identity(&self, root: impl AsRef<Path>) -> Result<ProjectIdentity, HostError> {
        Ok(ProjectIdentity::from(&Bundle::at(root.as_ref()).open()?))
    }

    /// Execute the migrated versioned domain commands behind one semantic
    /// boundary. Transport adapters may frame or render the result, but they
    /// do not validate or implement these commands independently.
    pub fn execute_domain_command(
        &self,
        command: CommandId,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, ExecutionError<HostError>> {
        execute(command, request, |request| {
            let string_field = |name: &str| {
                request
                    .get(name)
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| HostError::Validation {
                        detail: format!("missing string field {name:?}"),
                    })
            };

            match command {
                SKETCH_SOLVE_COMMAND_ID => {
                    let bundle_path = string_field("bundle_path")?;
                    let loaded = Bundle::at(bundle_path).open()?;
                    if let Some(expected_revision) = request
                        .get("expected_revision")
                        .and_then(serde_json::Value::as_str)
                        && expected_revision != loaded.revision_hash_hex()
                    {
                        return Err(HostError::Validation {
                            detail: format!(
                                "sketch source revision {expected_revision:?} does not match current revision {:?}",
                                loaded.revision_hash_hex()
                            ),
                        });
                    }
                    let typed_request = sketch_request_from_value(&request, &loaded)?;
                    let phase = request
                        .get("phase")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("commit");
                    let (result, revision_hash) = match phase {
                        "preview" => (
                            self.preview_sketch_solve(bundle_path, &typed_request)?,
                            None,
                        ),
                        "commit" => {
                            let preview = self.preview_sketch_solve(bundle_path, &typed_request)?;
                            if !preview.is_success() {
                                (preview, None)
                            } else {
                                let view = self.commit_sketch_solve(bundle_path, &typed_request)?;
                                (view.result, Some(view.snapshot.revision_hash))
                            }
                        }
                        _ => {
                            return Err(HostError::Validation {
                                detail: "phase must be preview or commit".to_string(),
                            });
                        }
                    };
                    let mut response =
                        serde_json::to_value(result).map_err(|error| HostError::Validation {
                            detail: format!("sketch response serialization failed: {error}"),
                        })?;
                    response["schema_version"] = serde_json::Value::String(
                        find(command)
                            .expect("sketch solve is registered")
                            .response_schema_version
                            .to_string(),
                    );
                    if let Some(revision_hash) = revision_hash {
                        response["revision_hash"] = serde_json::Value::String(revision_hash);
                    }
                    Ok(response)
                }
                IDENTITY_COMMAND_ID => {
                    let identity = self.identity(string_field("bundle_path")?)?;
                    Ok(serde_json::json!({
                        "generation_id": identity.generation_id,
                        "revision_id": identity.revision_id,
                        "feature_graph_hash": identity.feature_graph_hash,
                        "revision_hash": identity.revision_hash,
                        "transaction_count": identity.transaction_count,
                        "terminal_log_digest": identity.terminal_log_digest,
                        "schema_version": find(command)
                            .expect("identity is registered")
                            .response_schema_version,
                    }))
                }
                EXTRUDE_COMMAND_ID => {
                    let bundle_path = string_field("bundle_path")?;
                    let expected_revision = request
                        .get("expected_revision")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    if let Some(expected_revision) = &expected_revision {
                        let current = self.load(bundle_path)?;
                        if current.revision_hash != expected_revision.as_str() {
                            return Err(HostError::Validation {
                                detail: format!(
                                    "extrude source revision {expected_revision:?} does not match current revision {:?}",
                                    current.revision_hash
                                ),
                            });
                        }
                    }
                    let profile: Vec<[f64; 2]> =
                        serde_json::from_value(request.get("profile").cloned().ok_or_else(
                            || HostError::Validation {
                                detail: "missing profile".to_string(),
                            },
                        )?)
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid extrude profile: {error}"),
                        })?;
                    let height = request
                        .get("height")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or_else(|| HostError::Validation {
                            detail: "missing extrude height".to_string(),
                        })?;
                    let mode = parse_extrude_mode(request.get("mode"))?;
                    let target_feature_id = request
                        .get("target_feature_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let bundle_path = string_field("bundle_path")?.to_string();
                    let extrusion = ExtrudeRequest::new(
                        threeterm_occt_worker::new_request_id(),
                        profile.into_iter().map(|[x, y]| (x, y)).collect(),
                        height,
                    )
                    .with_mode(mode)
                    .with_optional_target_feature_id(target_feature_id.clone())
                    .with_feature_id(string_field("feature_id")?);
                    let source_snapshot = self.load(&bundle_path)?;
                    let extrusion = self.resolve_extrude_request(
                        Path::new(&bundle_path),
                        extrusion,
                        Some(&source_snapshot.revision_hash),
                    )?;
                    let worker =
                        OcctWorker::locate().map_err(|error| HostError::WorkerUnavailable {
                            detail: error.to_string(),
                        })?;
                    let view = if let Some(expected_revision) = expected_revision {
                        self.extrude_at_revision(
                            bundle_path,
                            extrusion,
                            &worker,
                            &expected_revision,
                        )?
                    } else {
                        self.extrude(bundle_path, extrusion, &worker)?
                    };
                    Ok(serde_json::json!({
                        "status": view.result.status,
                        "operation": "extrude",
                        "feature_id": view.result.feature_id,
                        "request_id": view.result.request_id,
                        "mode": mode.as_str(),
                        "target_feature_id": target_feature_id,
                        "source_snapshot": {
                            "feature_graph_hash": view.source_snapshot.feature_graph_hash,
                            "revision_hash": view.source_snapshot.revision_hash,
                        },
                        "feature_graph_hash": view.snapshot.feature_graph_hash,
                        "revision_hash": view.snapshot.revision_hash,
                        "authoritative": true,
                        "artifact_kind": "brep",
                        "artifact_name": format!("{}.brep", view.result.feature_id),
                        "brep_path": view.result.brep_path,
                        "brep_sha256": view.result.brep_sha256,
                        "brep_bytes": view.result.brep_bytes,
                        "worker_fingerprint": view.worker_fingerprint,
                        "derived_result": {
                            "request_id": view.artifact.request_id,
                            "operation": view.artifact.operation,
                            "feature_id": view.artifact.feature_id,
                            "source_revision_id": view.source_snapshot.revision_hash,
                            "worker_fingerprint": view.artifact.worker_fingerprint,
                            "artifact_kind": view.artifact.artifact_kind,
                            "artifact_name": view.artifact.artifact_name,
                            "byte_count": view.artifact.byte_count,
                            "sha256": view.artifact.sha256,
                        },
                        "schema_version": find(command).expect("extrude is registered").response_schema_version,
                    }))
                }
                BOOLEAN_PATTERN_COMMAND_ID => {
                    let origin: [f64; 3] =
                        serde_json::from_value(request.get("origin").cloned().ok_or_else(
                            || HostError::Validation {
                                detail: "missing boolean pattern origin".to_string(),
                            },
                        )?)
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid boolean pattern origin: {error}"),
                        })?;
                    let spacing: [f64; 2] =
                        serde_json::from_value(request.get("spacing").cloned().ok_or_else(
                            || HostError::Validation {
                                detail: "missing boolean pattern spacing".to_string(),
                            },
                        )?)
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid boolean pattern spacing: {error}"),
                        })?;
                    let columns = request
                        .get("columns")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(|| HostError::Validation {
                            detail: "invalid boolean pattern columns".to_string(),
                        })?;
                    let rows = request
                        .get("rows")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(|| HostError::Validation {
                            detail: "invalid boolean pattern rows".to_string(),
                        })?;
                    let diameter = request
                        .get("diameter")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or_else(|| HostError::Validation {
                            detail: "invalid boolean pattern diameter".to_string(),
                        })?;
                    if !origin.iter().all(|value| value.is_finite())
                        || !spacing
                            .iter()
                            .all(|value| value.is_finite() && *value > 0.0)
                        || !(1..=1000).contains(&columns)
                        || !(1..=1000).contains(&rows)
                        || !diameter.is_finite()
                        || diameter <= 0.0
                    {
                        return Err(HostError::Validation {
                            detail: "invalid boolean pattern numeric bounds".to_string(),
                        });
                    }
                    let bundle_path = string_field("bundle_path")?;
                    let feature_id = string_field("feature_id")?;
                    let base_feature_id = string_field("base_feature_id")?;
                    if !valid_feature_path_component(feature_id)
                        || !valid_feature_path_component(base_feature_id)
                    {
                        return Err(HostError::Validation {
                            detail: "boolean pattern feature IDs must be plain path components"
                                .to_string(),
                        });
                    }
                    let root = Bundle::at(bundle_path).canonical_root().to_path_buf();
                    let base_path = root
                        .join(BREP_SUBDIR)
                        .join(format!("{base_feature_id}.brep"));
                    let request = BooleanPatternRequest::new(
                        threeterm_occt_worker::new_request_id(),
                        base_path,
                        origin,
                        spacing,
                        columns,
                        rows,
                        diameter,
                    )
                    .with_output_path(root.join("stage"), "boolean-pattern.brep")
                    .with_feature_id(feature_id);
                    let worker =
                        OcctWorker::locate().map_err(|error| HostError::WorkerUnavailable {
                            detail: error.to_string(),
                        })?;
                    let view = self.boolean_pattern(bundle_path, request, &worker)?;
                    Ok(view.response_value(
                        find(command)
                            .expect("boolean pattern is registered")
                            .response_schema_version,
                    ))
                }
                threeterm_protocol::schema::UNDO_COMMAND_ID => {
                    let view = self.undo(string_field("bundle_path")?)?;
                    Ok(history_commit_value(
                        "undo",
                        find(command)
                            .expect("undo is registered")
                            .response_schema_version,
                        &view,
                    ))
                }
                threeterm_protocol::schema::REDO_COMMAND_ID => {
                    let view = self.redo(string_field("bundle_path")?)?;
                    Ok(history_commit_value(
                        "redo",
                        find(command)
                            .expect("redo is registered")
                            .response_schema_version,
                        &view,
                    ))
                }
                APPLY_COMMAND_ID => {
                    let operation = string_field("operation")?;
                    let feature_id = string_field("feature_id")?;
                    let kind = request.get("kind").and_then(serde_json::Value::as_str);
                    if matches!(operation, "add" | "set") && kind.is_none() {
                        return Err(HostError::Validation {
                            detail: format!("{operation} requires kind"),
                        });
                    }
                    if operation == "remove" && kind.is_some() {
                        return Err(HostError::Validation {
                            detail: "remove does not accept kind".to_string(),
                        });
                    }
                    let view = self.apply_feature(
                        string_field("bundle_path")?,
                        operation,
                        feature_id,
                        kind,
                        string_field("expected_revision")?,
                    )?;
                    Ok(serde_json::json!({
                        "status": "committed",
                        "operation": view.operation,
                        "feature_id": view.feature_id,
                        "generation_id": view.identity.generation_id,
                        "revision_id": view.identity.revision_id,
                        "feature_graph_hash": view.identity.feature_graph_hash,
                        "revision_hash": view.identity.revision_hash,
                        "transaction_count": view.identity.transaction_count,
                        "terminal_log_digest": view.identity.terminal_log_digest,
                        "schema_version": find(command)
                            .expect("apply is registered")
                            .response_schema_version,
                    }))
                }
                threeterm_protocol::schema::REATTACH_EDGE_COMMAND_ID => {
                    let reference: SelectedEdgeReference =
                        serde_json::from_value(request.get("reference").cloned().ok_or_else(
                            || HostError::Validation {
                                detail: "missing selected edge reference".to_string(),
                            },
                        )?)
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid selected edge reference: {error}"),
                        })?;
                    let edit_target: SelectedEdgeReference =
                        serde_json::from_value(request.get("edit_target").cloned().ok_or_else(
                            || HostError::Validation {
                                detail: "missing edge edit target reference".to_string(),
                            },
                        )?)
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid edge edit target reference: {error}"),
                        })?;
                    let bundle_path = string_field("bundle_path")?;
                    let expected_revision = string_field("expected_revision")?;
                    let edit_feature_id = string_field("edit_feature_id")?;
                    let edit_kind = string_field("edit_kind")?;
                    if !matches!(edit_kind, "fillet" | "split") {
                        return Err(HostError::Validation {
                            detail: "edge reattachment requires fillet or split".to_string(),
                        });
                    }
                    if edit_kind == "fillet"
                        && (request.get("plane_point").is_some()
                            || request.get("plane_normal").is_some())
                    {
                        return Err(HostError::Validation {
                            detail: "fillet edge reattachment does not accept a split plane"
                                .to_string(),
                        });
                    }
                    let base_feature_id = string_field("base_feature_id")?;
                    if edit_target.provenance.source_feature_id != base_feature_id
                        || edit_target.provenance.source_revision_id != expected_revision
                    {
                        return Err(HostError::Validation {
                            detail: "edge edit target provenance does not match edit source"
                                .to_string(),
                        });
                    }
                    let radius = request
                        .get("radius")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or_else(|| HostError::Validation {
                            detail: "edge reattachment requires radius".to_string(),
                        })?;
                    let view = if edit_kind == "fillet" {
                        let worker =
                            OcctWorker::locate().map_err(|error| HostError::WorkerUnavailable {
                                detail: error.to_string(),
                            })?;
                        self.reattach_edge_with_fillet(
                            bundle_path,
                            expected_revision,
                            base_feature_id,
                            edit_feature_id,
                            radius,
                            reference,
                            edit_target,
                            &worker,
                        )?
                    } else {
                        let plane_point = serde_json::from_value::<[f64; 3]>(
                            request.get("plane_point").cloned().ok_or_else(|| {
                                HostError::Validation {
                                    detail: "split edge reattachment requires plane_point"
                                        .to_string(),
                                }
                            })?,
                        )
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid split plane_point: {error}"),
                        })?;
                        let plane_normal = serde_json::from_value::<[f64; 3]>(
                            request.get("plane_normal").cloned().ok_or_else(|| {
                                HostError::Validation {
                                    detail: "split edge reattachment requires plane_normal"
                                        .to_string(),
                                }
                            })?,
                        )
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid split plane_normal: {error}"),
                        })?;
                        if !plane_point
                            .iter()
                            .chain(plane_normal.iter())
                            .all(|component| component.is_finite())
                            || plane_normal
                                .iter()
                                .map(|component| component * component)
                                .sum::<f64>()
                                <= f64::EPSILON
                        {
                            return Err(HostError::Validation {
                                detail: "split plane must have finite point and non-zero normal"
                                    .to_string(),
                            });
                        }
                        let worker =
                            OcctWorker::locate().map_err(|error| HostError::WorkerUnavailable {
                                detail: error.to_string(),
                            })?;
                        self.reattach_edge_with_split(
                            bundle_path,
                            expected_revision,
                            base_feature_id,
                            edit_feature_id,
                            plane_point,
                            plane_normal,
                            reference,
                            &worker,
                        )?
                    };
                    let (outcome, selected_edge_id, candidate_edge_ids) = match view.outcome {
                        EdgeReattachmentOutcome::Resolved { semantic_id } => {
                            ("resolved", Some(semantic_id), Vec::new())
                        }
                        EdgeReattachmentOutcome::Ambiguous { candidate_ids } => {
                            ("ambiguous", None, candidate_ids)
                        }
                        EdgeReattachmentOutcome::Lost => ("lost", None, Vec::new()),
                        EdgeReattachmentOutcome::Incompatible { candidate_ids } => {
                            ("incompatible", None, candidate_ids)
                        }
                    };
                    Ok(serde_json::json!({
                        "outcome": outcome,
                        "selected_edge_id": selected_edge_id.unwrap_or_default(),
                        "candidate_edge_ids": candidate_edge_ids,
                        "committed": view.committed,
                        "edit_feature_id": view.edit_feature_id,
                        "source_revision": view.source_snapshot.revision_hash,
                        "revision_hash": view.snapshot.revision_hash,
                        "schema_version": find(command).expect("reattach-edge is registered").response_schema_version,
                    }))
                }
                BOOLEAN_FUSE_COMMAND_ID | BOOLEAN_CUT_COMMAND_ID | BOOLEAN_COMMON_COMMAND_ID => {
                    let bundle_path = string_field("bundle_path")?;
                    let feature_id = string_field("feature_id")?;
                    let base_feature_id = string_field("base_feature_id")?;
                    let tool_feature_id = string_field("tool_feature_id")?;
                    // Shared boolean behavior: semantic operands must exist as features and
                    // resolve to brep provenance, fail closed without mutation.
                    let loaded = Bundle::at(bundle_path).open()?;
                    for operand_id in [&base_feature_id, &tool_feature_id] {
                        if !loaded.graph.contains_feature(operand_id) {
                            return Err(HostError::Validation {
                                detail: format!("boolean operand feature is missing: {operand_id}"),
                            });
                        }
                    }
                    // Semantic fail-closed: incompatible not-brep kind.
                    let kind_of = |id: &str| {
                        loaded
                            .graph
                            .features()
                            .find(|feature| feature.id.as_str() == id)
                            .map(|feature| feature.kind.clone())
                            .unwrap_or_default()
                    };
                    let base_kind = kind_of(base_feature_id);
                    let tool_kind = kind_of(tool_feature_id);
                    let is_solid_kind = |kind: &str| {
                        kind == "brep"
                            || kind.starts_with("brep:")
                            || kind.starts_with("bracket:")
                            || matches!(
                                kind,
                                "extrude"
                                    | "revolve"
                                    | "fillet"
                                    | "chamfer"
                                    | "hole"
                                    | "mirror"
                                    | "linear-pattern"
                                    | "circular-pattern"
                                    | "boolean-fuse"
                                    | "boolean-cut"
                                    | "boolean-common"
                                    | "boolean-pattern"
                                    | "boolean_fuse"
                                    | "boolean_cut"
                                    | "boolean_common"
                                    | "shell"
                                    | "draft"
                                    | "loft"
                            )
                    };
                    if !is_solid_kind(&base_kind) || !is_solid_kind(&tool_kind) {
                        return Err(HostError::Validation {
                            detail: "boolean operand is incompatible: not a solid".to_string(),
                        });
                    }
                    let bundle_root = PathBuf::from(bundle_path);
                    let base_path = bundle_root
                        .join("brep")
                        .join(format!("{base_feature_id}.brep"));
                    let tool_path = bundle_root
                        .join("brep")
                        .join(format!("{tool_feature_id}.brep"));
                    // Provenance check: the committed BREP bytes must match the
                    // canonical log entry, otherwise the operand is lost.
                    for (operand_id, operand_path) in [
                        (&base_feature_id, &base_path),
                        (&tool_feature_id, &tool_path),
                    ] {
                        let entry = loaded
                            .log
                            .entries()
                            .iter()
                            .rev()
                            .find(|entry| {
                                entry.feature_id == **operand_id
                                    && entry.brep_byte_count.is_some()
                                    && entry.brep_sha256.is_some()
                            })
                            .ok_or_else(|| HostError::Validation {
                                detail: format!(
                                    "boolean operand has no committed BREP: {operand_id}"
                                ),
                            })?;
                        let expected_bytes = entry
                            .brep_byte_count
                            .and_then(|value| usize::try_from(value).ok());
                        let expected_sha = entry.brep_sha256.as_deref();
                        match (expected_bytes, expected_sha) {
                            (Some(expected_bytes), Some(expected_sha)) => {
                                read_brep_verified(
                                    operand_path,
                                    Some((expected_bytes, expected_sha)),
                                )
                                .map_err(|detail| HostError::Validation { detail })?;
                            }
                            _ => {
                                return Err(HostError::Validation {
                                    detail: format!(
                                        "boolean operand BREP provenance is invalid: {operand_id}"
                                    ),
                                });
                            }
                        }
                    }
                    let worker =
                        OcctWorker::locate().map_err(|error| HostError::WorkerUnavailable {
                            detail: error.to_string(),
                        })?;
                    let output_dir = bundle_root.join("stage");
                    let derived_result = match command {
                        BOOLEAN_FUSE_COMMAND_ID => {
                            let req = BooleanFuseRequest::new(
                                threeterm_occt_worker::new_request_id(),
                                base_path,
                                tool_path,
                            )
                            .with_output_path(output_dir, feature_id)
                            .with_feature_id(feature_id);
                            let v = self.boolean_fuse(bundle_path, req, &worker)?;
                            boolean_derived_result_json(&v.artifact)
                        }
                        BOOLEAN_CUT_COMMAND_ID => {
                            let req = BooleanCutRequest::new(
                                threeterm_occt_worker::new_request_id(),
                                base_path,
                                tool_path,
                            )
                            .with_output_path(output_dir, feature_id)
                            .with_feature_id(feature_id);
                            let v = self.boolean_cut(bundle_path, req, &worker)?;
                            boolean_derived_result_json(&v.artifact)
                        }
                        BOOLEAN_COMMON_COMMAND_ID => {
                            let req = BooleanCommonRequest::new(
                                threeterm_occt_worker::new_request_id(),
                                base_path,
                                tool_path,
                            )
                            .with_output_path(output_dir, feature_id)
                            .with_feature_id(feature_id);
                            let v = self.boolean_common(bundle_path, req, &worker)?;
                            boolean_derived_result_json(&v.artifact)
                        }
                        _ => unreachable!(),
                    };
                    let snapshot = self.load(bundle_path)?;
                    let brep_path = bundle_root.join("brep").join(format!("{feature_id}.brep"));
                    let brep_bytes = std::fs::metadata(&brep_path).map(|m| m.len()).unwrap_or(0);
                    let brep_sha256 = threeterm_protocol::artifact::sha256_hex(
                        &std::fs::read(&brep_path).unwrap_or_default(),
                    );
                    Ok(serde_json::json!({
                        "status": "ok",
                        "operation": command.0,
                        "feature_id": feature_id,
                        "feature_graph_hash": snapshot.feature_graph_hash,
                        "revision_hash": snapshot.revision_hash,
                        "brep_path": brep_path.to_string_lossy(),
                        "brep_sha256": brep_sha256,
                        "brep_bytes": brep_bytes,
                        "derived_result": derived_result,
                        "schema_version": find(command).expect("boolean is registered").response_schema_version,
                    }))
                }
                HOLE_COMMAND_ID => {
                    let bundle_path = string_field("bundle_path")?;
                    let feature_id = string_field("feature_id")?;
                    let base_feature_id = string_field("base_feature_id")?;
                    if let Some(expected_revision) = request
                        .get("expected_revision")
                        .and_then(serde_json::Value::as_str)
                    {
                        let current = self.load(bundle_path)?;
                        if current.revision_hash != expected_revision {
                            return Err(HostError::Validation {
                                detail: format!(
                                    "hole source revision {expected_revision:?} does not match current revision {:?}",
                                    current.revision_hash
                                ),
                            });
                        }
                    }
                    let position: [f64; 3] =
                        serde_json::from_value(request.get("position").cloned().ok_or_else(
                            || HostError::Validation {
                                detail: "missing hole position".to_string(),
                            },
                        )?)
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid hole position: {error}"),
                        })?;
                    let direction: [f64; 3] =
                        serde_json::from_value(request.get("direction").cloned().ok_or_else(
                            || HostError::Validation {
                                detail: "missing hole direction".to_string(),
                            },
                        )?)
                        .map_err(|error| HostError::Validation {
                            detail: format!("invalid hole direction: {error}"),
                        })?;
                    let diameter = request
                        .get("diameter")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or_else(|| HostError::Validation {
                            detail: "missing hole diameter".to_string(),
                        })?;
                    let hole_kind = parse_hole_kind(request.get("hole_kind"))?;
                    let thread_designation = request
                        .get("thread_designation")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let thread_pitch = request
                        .get("thread_pitch")
                        .and_then(serde_json::Value::as_f64);
                    let thread_depth = request
                        .get("thread_depth")
                        .and_then(serde_json::Value::as_f64);
                    let bundle_root = PathBuf::from(bundle_path);
                    let base_path = bundle_root
                        .join("brep")
                        .join(format!("{base_feature_id}.brep"));
                    let output_dir = bundle_root.join("stage");
                    let mut hole_request = HoleRequest::new(
                        threeterm_occt_worker::new_request_id(),
                        &base_path,
                        position,
                        direction,
                        diameter,
                    )
                    .with_hole_kind(hole_kind)
                    .with_base_feature_id(base_feature_id)
                    .with_output_path(output_dir, feature_id)
                    .with_feature_id(feature_id);
                    if let Some(designation) = thread_designation {
                        hole_request = hole_request.with_thread(
                            designation,
                            thread_pitch.unwrap_or_default(),
                            thread_depth.unwrap_or_default(),
                        );
                    }
                    // Fail closed before touching the worker so invalid hole
                    // geometry preserves the prior Revision Snapshot without
                    // requiring a worker binary.
                    hole_request
                        .validate()
                        .map_err(|detail| HostError::Validation {
                            detail: format!("hole request is invalid: {detail}"),
                        })?;
                    let loaded = Bundle::at(bundle_path).open()?;
                    if !loaded.graph.contains_feature(base_feature_id) {
                        return Err(HostError::Validation {
                            detail: format!("hole base feature is missing: {base_feature_id}"),
                        });
                    }
                    let worker =
                        OcctWorker::locate().map_err(|error| HostError::WorkerUnavailable {
                            detail: error.to_string(),
                        })?;
                    let view = self.hole(bundle_path, hole_request, &worker)?;
                    let snapshot = self.load(bundle_path)?;
                    let brep_path = bundle_root.join("brep").join(format!("{feature_id}.brep"));
                    Ok(serde_json::json!({
                        "status": view.result.status,
                        "operation": "hole",
                        "feature_id": view.result.feature_id,
                        "feature_graph_hash": snapshot.feature_graph_hash,
                        "revision_hash": snapshot.revision_hash,
                        "brep_path": brep_path.to_string_lossy(),
                        "brep_sha256": view.artifact.sha256,
                        "brep_bytes": view.artifact.byte_count,
                        "derived_result": boolean_derived_result_json(&view.artifact),
                        "schema_version": find(command).expect("hole is registered").response_schema_version,
                    }))
                }
                _ => Err(HostError::Validation {
                    detail: format!(
                        "command {} is not handled by the domain executor",
                        command.0
                    ),
                }),
            }
        })
    }

    /// Evaluate a registered command without publishing its worker result.
    /// The returned fingerprints are transient and are never added to the
    /// canonical bundle or presentation cache.
    pub fn preview_domain_command(
        &self,
        command: CommandId,
        request: serde_json::Value,
    ) -> Result<DomainCommandPreview, ExecutionError<HostError>> {
        if let Err(error) = validate_request(command, &request) {
            return Err(match error {
                ExecutionError::UnknownCommand(command) => ExecutionError::UnknownCommand(command),
                ExecutionError::InvalidRequest(detail) => ExecutionError::InvalidRequest(detail),
                ExecutionError::Handler(()) | ExecutionError::InvalidResponse(_) => unreachable!(),
            });
        }
        if command == SKETCH_SOLVE_COMMAND_ID {
            let bundle_path = request
                .get("bundle_path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ExecutionError::InvalidRequest("missing bundle_path".to_string()))?;
            let loaded = Bundle::at(bundle_path)
                .open()
                .map_err(|error| ExecutionError::Handler(error.into()))?;
            let typed_request =
                sketch_request_from_value(&request, &loaded).map_err(ExecutionError::Handler)?;
            let result = self
                .preview_sketch_solve(bundle_path, &typed_request)
                .map_err(ExecutionError::Handler)?;
            let source_revision = loaded.revision_hash_hex().to_string();
            let input_fingerprint = sha256_hex(request.to_string().as_bytes());
            let geometry_fingerprint = sha256_hex(
                serde_json::to_string(&result)
                    .expect("sketch preview serializes")
                    .as_bytes(),
            );
            return Ok(DomainCommandPreview {
                command,
                source_revision: source_revision.clone(),
                preview_revision: sha256_hex(
                    format!("preview:{source_revision}:{input_fingerprint}:{geometry_fingerprint}")
                        .as_bytes(),
                ),
                input_fingerprint,
                geometry_fingerprint,
            });
        }
        if command == HOLE_COMMAND_ID {
            let bundle_path = request
                .get("bundle_path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ExecutionError::InvalidRequest("missing bundle_path".to_string()))?;
            let current = self.load(bundle_path).map_err(ExecutionError::Handler)?;
            let loaded = Bundle::at(bundle_path)
                .open()
                .map_err(|error| ExecutionError::Handler(error.into()))?;
            if let Some(expected_revision) = request
                .get("expected_revision")
                .and_then(serde_json::Value::as_str)
                && current.revision_hash != expected_revision
            {
                return Err(ExecutionError::Handler(HostError::Validation {
                    detail: format!(
                        "hole source revision {expected_revision:?} does not match current revision {:?}",
                        current.revision_hash
                    ),
                }));
            }
            let feature_id = request
                .get("feature_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ExecutionError::InvalidRequest("missing feature_id".to_string()))?;
            let base_feature_id = request
                .get("base_feature_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    ExecutionError::InvalidRequest("missing base_feature_id".to_string())
                })?;
            if !loaded.graph.contains_feature(base_feature_id) {
                return Err(ExecutionError::Handler(HostError::Validation {
                    detail: format!("hole base feature is missing: {base_feature_id}"),
                }));
            }
            let position: [f64; 3] =
                serde_json::from_value(request.get("position").cloned().ok_or_else(|| {
                    ExecutionError::InvalidRequest("missing position".to_string())
                })?)
                .map_err(|error| {
                    ExecutionError::Handler(HostError::Validation {
                        detail: format!("invalid hole position: {error}"),
                    })
                })?;
            let direction: [f64; 3] =
                serde_json::from_value(request.get("direction").cloned().ok_or_else(|| {
                    ExecutionError::InvalidRequest("missing direction".to_string())
                })?)
                .map_err(|error| {
                    ExecutionError::Handler(HostError::Validation {
                        detail: format!("invalid hole direction: {error}"),
                    })
                })?;
            let diameter = request
                .get("diameter")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| ExecutionError::InvalidRequest("missing diameter".to_string()))?;
            let hole_kind =
                parse_hole_kind(request.get("hole_kind")).map_err(ExecutionError::Handler)?;
            let mut hole_request = HoleRequest::new(
                threeterm_occt_worker::new_request_id(),
                loaded
                    .canonical_root
                    .join(BREP_SUBDIR)
                    .join(format!("{base_feature_id}.brep")),
                position,
                direction,
                diameter,
            )
            .with_hole_kind(hole_kind.clone())
            .with_base_feature_id(base_feature_id)
            .with_feature_id(feature_id);
            if hole_kind == "tapped" {
                hole_request = hole_request.with_thread(
                    request
                        .get("thread_designation")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                    request
                        .get("thread_pitch")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or_default(),
                    request
                        .get("thread_depth")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or_default(),
                );
            }
            hole_request
                .validate()
                .map_err(|detail| ExecutionError::Handler(HostError::Validation { detail }))?;
            let worker = OcctWorker::locate().map_err(|error| {
                ExecutionError::Handler(HostError::WorkerUnavailable {
                    detail: error.to_string(),
                })
            })?;
            let derived = self
                .stage_occt_result::<HoleResult>(
                    Path::new(bundle_path),
                    &hole_request,
                    threeterm_occt_worker::Operation::Hole,
                    &worker,
                )
                .map_err(ExecutionError::Handler)?;
            if let Some(expected_revision) = request
                .get("expected_revision")
                .and_then(serde_json::Value::as_str)
                && derived.source_snapshot.revision_hash != expected_revision
            {
                let current_revision = derived.source_snapshot.revision_hash.clone();
                self.discard_staged_occt_result(&derived);
                return Err(ExecutionError::Handler(HostError::Validation {
                    detail: format!(
                        "hole preview source revision changed from {expected_revision:?} to {current_revision:?}"
                    ),
                }));
            }
            let source_revision = derived.source_snapshot.revision_hash.clone();
            let input_fingerprint = sha256_hex(request.to_string().as_bytes());
            let geometry_fingerprint = derived.result.brep_sha256.clone();
            let preview_revision = sha256_hex(
                format!("preview:{source_revision}:{input_fingerprint}:{geometry_fingerprint}")
                    .as_bytes(),
            );
            self.discard_staged_occt_result(&derived);
            return Ok(DomainCommandPreview {
                command,
                source_revision,
                preview_revision,
                input_fingerprint,
                geometry_fingerprint,
            });
        }

        if command != EXTRUDE_COMMAND_ID {
            return Err(ExecutionError::Handler(HostError::Validation {
                detail: format!(
                    "command {} is discoverable but has no interactive preview handler",
                    command.0
                ),
            }));
        }

        let bundle_path = request
            .get("bundle_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ExecutionError::InvalidRequest("missing bundle_path".to_string()))?;
        let current = self.load(bundle_path).map_err(ExecutionError::Handler)?;
        if let Some(expected_revision) = request
            .get("expected_revision")
            .and_then(serde_json::Value::as_str)
            && current.revision_hash != expected_revision
        {
            return Err(ExecutionError::Handler(HostError::Validation {
                detail: format!(
                    "extrude source revision {expected_revision:?} does not match current revision {:?}",
                    current.revision_hash
                ),
            }));
        }

        let profile: Vec<[f64; 2]> = serde_json::from_value(
            request
                .get("profile")
                .cloned()
                .ok_or_else(|| ExecutionError::InvalidRequest("missing profile".to_string()))?,
        )
        .map_err(|error| {
            ExecutionError::Handler(HostError::Validation {
                detail: format!("invalid extrude profile: {error}"),
            })
        })?;
        let height = request
            .get("height")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| ExecutionError::InvalidRequest("missing extrude height".to_string()))?;
        let feature_id = request
            .get("feature_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ExecutionError::InvalidRequest("missing feature_id".to_string()))?;
        let mode = parse_extrude_mode(request.get("mode")).map_err(ExecutionError::Handler)?;
        let target_feature_id = request
            .get("target_feature_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let worker = OcctWorker::locate().map_err(|error| {
            ExecutionError::Handler(HostError::WorkerUnavailable {
                detail: error.to_string(),
            })
        })?;
        let derived = self
            .stage_extrude(
                bundle_path,
                ExtrudeRequest::new(
                    threeterm_occt_worker::new_request_id(),
                    profile.into_iter().map(|[x, y]| (x, y)).collect(),
                    height,
                )
                .with_mode(mode)
                .with_optional_target_feature_id(target_feature_id)
                .with_feature_id(feature_id),
                &worker,
            )
            .map_err(ExecutionError::Handler)?;
        if let Some(expected_revision) = request
            .get("expected_revision")
            .and_then(serde_json::Value::as_str)
            && derived.source_snapshot.revision_hash != expected_revision
        {
            let current_revision = derived.source_snapshot.revision_hash.clone();
            self.discard_extrude_derived(derived);
            return Err(ExecutionError::Handler(HostError::Validation {
                detail: format!(
                    "extrude preview source revision changed from {expected_revision:?} to {current_revision:?}"
                ),
            }));
        }
        let source_revision = derived.source_snapshot.revision_hash.clone();
        let input_fingerprint = sha256_hex(request.to_string().as_bytes());
        let geometry_fingerprint = derived.result.brep_sha256.clone();
        let preview_revision = sha256_hex(
            format!("preview:{source_revision}:{input_fingerprint}:{geometry_fingerprint}")
                .as_bytes(),
        );
        self.discard_extrude_derived(derived);
        Ok(DomainCommandPreview {
            command,
            source_revision,
            preview_revision,
            input_fingerprint,
            geometry_fingerprint,
        })
    }

    /// Run a real OCCT fillet, resolve the selected edge against evidence
    /// returned by that worker, and publish neither result nor reference until
    /// resolution and the source-revision guard have both succeeded.
    #[allow(clippy::too_many_arguments)]
    pub fn reattach_edge_with_fillet(
        &self,
        root: impl AsRef<Path>,
        expected_revision: &str,
        base_feature_id: &str,
        edit_feature_id: &str,
        radius: f64,
        reference: SelectedEdgeReference,
        edit_target: SelectedEdgeReference,
        worker: &OcctWorker,
    ) -> Result<EdgeReattachmentView, HostError> {
        let root = root.as_ref();
        let loaded = Bundle::at(root).open()?;
        let source_snapshot = SnapshotView::from(&loaded);
        if source_snapshot.revision_hash != expected_revision {
            return Err(HostError::Validation {
                detail: "edge edit source revision does not match current revision".to_string(),
            });
        }
        if reference.provenance.source_feature_id != base_feature_id {
            return Err(HostError::Validation {
                detail: "edge reference source feature does not match edit base feature"
                    .to_string(),
            });
        }
        if reference.provenance.source_revision_id != source_snapshot.revision_hash {
            return Err(HostError::Validation {
                detail: "edge reference source revision does not match current revision"
                    .to_string(),
            });
        }
        if edit_target.provenance.source_feature_id != base_feature_id
            || edit_target.provenance.source_revision_id != source_snapshot.revision_hash
        {
            return Err(HostError::Validation {
                detail: "edge edit target provenance does not match edit source".to_string(),
            });
        }
        let mut worker_reference = reference.clone();
        worker_reference.provenance.source_feature_id = base_feature_id.to_string();
        worker_reference.provenance.source_revision_id = source_snapshot.revision_hash.clone();
        let request = FilletRequest::new(
            threeterm_occt_worker::new_request_id(),
            bundle_root(root)
                .join(BREP_SUBDIR)
                .join(format!("{base_feature_id}.brep")),
            radius,
        )
        .with_output_path(root.join("stage"), format!("{edit_feature_id}.brep"))
        .with_feature_id(edit_feature_id)
        .with_selected_edge(SelectedEdgeContext {
            semantic_id: worker_reference.semantic_id.clone(),
            source_feature_id: worker_reference.provenance.source_feature_id.clone(),
            source_revision_id: worker_reference.provenance.source_revision_id.clone(),
            source_edge_id: worker_reference.provenance.source_edge_id.clone(),
            role: worker_reference.role.clone(),
            midpoint: worker_reference.evidence.midpoint,
            tangent: worker_reference.evidence.tangent,
            length: worker_reference.evidence.length,
        });
        let request = request.with_edit_target(SelectedEdgeContext {
            semantic_id: edit_target.semantic_id.clone(),
            source_feature_id: edit_target.provenance.source_feature_id.clone(),
            source_revision_id: edit_target.provenance.source_revision_id.clone(),
            source_edge_id: edit_target.provenance.source_edge_id.clone(),
            role: edit_target.role.clone(),
            midpoint: edit_target.evidence.midpoint,
            tangent: edit_target.evidence.tangent,
            length: edit_target.evidence.length,
        });
        let derived = self.stage_occt_result::<FilletResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Fillet,
            worker,
        )?;
        let candidates = domain_edge_candidates(&derived.result.edge_candidates);
        let outcome = resolve_edge_reference(&worker_reference, candidates);
        if !matches!(outcome, EdgeReattachmentOutcome::Resolved { .. }) {
            if let Some(stage_root) = derived.artifact.path.parent() {
                let _ = fs::remove_dir_all(stage_root);
            }
            return Ok(EdgeReattachmentView {
                outcome,
                source_snapshot: source_snapshot.clone(),
                snapshot: source_snapshot,
                edit_feature_id: edit_feature_id.to_string(),
                committed: false,
            });
        }
        let selected_edge_id = match &outcome {
            EdgeReattachmentOutcome::Resolved { semantic_id } => semantic_id.clone(),
            _ => unreachable!(),
        };
        let reference_payload = format!(
            "edge-reattachment:{}",
            serde_json::json!({
                "schema": "threeterm.reattach-edge/1",
                "selected_edge_id": selected_edge_id,
                "reference": reference,
            })
        );
        let (snapshot, _result, _artifact) = self.promote_occt_result_with_append(
            root,
            derived,
            |bundle, _current, _derived, artifact, bytes, provenance| {
                bundle.append_new_feature_with_brep_if_revision_and_provenance(
                    &artifact.feature_id,
                    &reference_payload,
                    expected_revision,
                    &artifact.request_id,
                    provenance,
                    bytes,
                )
            },
        )?;
        Ok(EdgeReattachmentView {
            outcome,
            source_snapshot,
            snapshot,
            edit_feature_id: edit_feature_id.to_string(),
            committed: true,
        })
    }

    /// Run a real OCCT split edit and resolve the selected edge against the
    /// split descendants. Ambiguous descendants are returned without
    /// promotion, preserving the canonical revision and feature graph.
    #[allow(clippy::too_many_arguments)]
    pub fn reattach_edge_with_split(
        &self,
        root: impl AsRef<Path>,
        expected_revision: &str,
        base_feature_id: &str,
        edit_feature_id: &str,
        plane_point: [f64; 3],
        plane_normal: [f64; 3],
        reference: SelectedEdgeReference,
        worker: &OcctWorker,
    ) -> Result<EdgeReattachmentView, HostError> {
        let root = root.as_ref();
        let loaded = Bundle::at(root).open()?;
        let source_snapshot = SnapshotView::from(&loaded);
        if source_snapshot.revision_hash != expected_revision {
            return Err(HostError::Validation {
                detail: "edge edit source revision does not match current revision".to_string(),
            });
        }
        if reference.provenance.source_feature_id != base_feature_id {
            return Err(HostError::Validation {
                detail: "edge reference source feature does not match edit base feature"
                    .to_string(),
            });
        }
        if reference.provenance.source_revision_id != source_snapshot.revision_hash {
            return Err(HostError::Validation {
                detail: "edge reference source revision does not match current revision"
                    .to_string(),
            });
        }
        let mut worker_reference = reference.clone();
        worker_reference.provenance.source_feature_id = base_feature_id.to_string();
        worker_reference.provenance.source_revision_id = source_snapshot.revision_hash.clone();
        let request = SplitRequest::new(
            threeterm_occt_worker::new_request_id(),
            bundle_root(root)
                .join(BREP_SUBDIR)
                .join(format!("{base_feature_id}.brep")),
            plane_point,
            plane_normal,
        )
        .with_output_path(root.join("stage"), format!("{edit_feature_id}.brep"))
        .with_feature_id(edit_feature_id)
        .with_selected_edge(SelectedEdgeContext {
            semantic_id: worker_reference.semantic_id.clone(),
            source_feature_id: worker_reference.provenance.source_feature_id.clone(),
            source_revision_id: worker_reference.provenance.source_revision_id.clone(),
            source_edge_id: worker_reference.provenance.source_edge_id.clone(),
            role: worker_reference.role.clone(),
            midpoint: worker_reference.evidence.midpoint,
            tangent: worker_reference.evidence.tangent,
            length: worker_reference.evidence.length,
        });
        let derived = self.stage_occt_result::<SplitResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Split,
            worker,
        )?;
        let candidates = domain_edge_candidates(&derived.result.edge_candidates);
        let outcome = resolve_split_edge_reference(&worker_reference, candidates);
        if !matches!(outcome, EdgeReattachmentOutcome::Resolved { .. }) {
            if let Some(stage_root) = derived.artifact.path.parent() {
                let _ = fs::remove_dir_all(stage_root);
            }
            return Ok(EdgeReattachmentView {
                outcome,
                source_snapshot: source_snapshot.clone(),
                snapshot: source_snapshot,
                edit_feature_id: edit_feature_id.to_string(),
                committed: false,
            });
        }
        let selected_edge_id = match &outcome {
            EdgeReattachmentOutcome::Resolved { semantic_id } => semantic_id.clone(),
            _ => unreachable!(),
        };
        let reference_payload = format!(
            "edge-reattachment:{}",
            serde_json::json!({
                "schema": "threeterm.reattach-edge/1",
                "selected_edge_id": selected_edge_id,
                "reference": reference,
            })
        );
        let (snapshot, _result, _artifact) = self.promote_occt_result_with_append(
            root,
            derived,
            |bundle, _current, _derived, artifact, bytes, provenance| {
                bundle.append_new_feature_with_brep_if_revision_and_provenance(
                    &artifact.feature_id,
                    &reference_payload,
                    expected_revision,
                    &artifact.request_id,
                    provenance,
                    bytes,
                )
            },
        )?;
        Ok(EdgeReattachmentView {
            outcome,
            source_snapshot,
            snapshot,
            edit_feature_id: edit_feature_id.to_string(),
            committed: true,
        })
    }

    pub fn apply_feature(
        &self,
        root: impl AsRef<Path>,
        operation: &str,
        feature_id: &str,
        kind: Option<&str>,
        expected_revision: &str,
    ) -> Result<ApplyView, HostError> {
        let bundle = Bundle::at(root.as_ref());
        let updated = match bundle.apply_feature_if_revision(
            operation,
            feature_id,
            kind,
            expected_revision,
        ) {
            Ok(updated) => updated,
            Err(error) => {
                if let Ok(current) = bundle.open() {
                    self.current.replace(Some(current));
                }
                return Err(error.into());
            }
        };
        let identity = ProjectIdentity::from(&updated);
        self.current.replace(Some(updated));
        Ok(ApplyView {
            operation: operation.to_string(),
            feature_id: feature_id.to_string(),
            identity,
        })
    }

    /// Commit a fit relationship using dimensions already present in the
    /// canonical sketches. The expected revision is checked again by
    /// persistence under its write lock before the relation is published.
    #[allow(clippy::too_many_arguments)]
    pub fn fit_dimension(
        &self,
        root: impl AsRef<Path>,
        expected_revision: &str,
        source_feature_id: &str,
        target_feature_id: &str,
        source_dimension_id: &str,
        target_dimension_id: &str,
        dimension: &str,
        clearance: f64,
    ) -> Result<FitDimensionCommitView, HostError> {
        if !clearance.is_finite() || clearance <= 0.0 {
            return Err(HostError::Validation {
                detail: "fit clearance must be strictly positive and finite".to_string(),
            });
        }
        let bundle = Bundle::at(root.as_ref());
        let loaded = bundle.open()?;
        if loaded.revision_hash_hex() != expected_revision {
            return Err(HostError::Validation {
                detail: format!(
                    "fit dimension source revision {expected_revision:?} does not match current revision {:?}",
                    loaded.revision_hash_hex()
                ),
            });
        }
        let source_value =
            sketch_dimension_value(&loaded.graph, source_feature_id, source_dimension_id)?;
        let target_value =
            sketch_dimension_value(&loaded.graph, target_feature_id, target_dimension_id)?;
        let fit = FitDimension {
            id: format!(
                "fit:{source_feature_id}:{target_feature_id}:{dimension}:{source_dimension_id}:{target_dimension_id}"
            ),
            source_feature_id: source_feature_id.to_string(),
            target_feature_id: target_feature_id.to_string(),
            source_dimension_id: source_dimension_id.to_string(),
            target_dimension_id: target_dimension_id.to_string(),
            dimension: dimension.to_string(),
            source_value,
            target_value,
            clearance,
        };
        fit.validate()
            .map_err(|detail| HostError::Validation { detail })?;
        let updated = match bundle.append_fit_dimension_if_revision(&fit, expected_revision) {
            Ok(updated) => updated,
            Err(error) => {
                if let Ok(reopened) = bundle.open() {
                    self.current.replace(Some(reopened));
                }
                return Err(error.into());
            }
        };
        let snapshot = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        Ok(FitDimensionCommitView { snapshot, fit })
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
        let prior_history = if bundle.canonical_root().exists() {
            bundle.open()?.history
        } else {
            HistoryState::default()
        };
        let history_event = prior_history
            .initialize_l_bracket(bracket_id, length, width, height, thickness)
            .map_err(|error| HostError::Validation {
                detail: error.to_string(),
            })?;
        let vertical_id = format!("{bracket_id}-plate-vertical");
        let horizontal_id = format!("{bracket_id}-plate-horizontal");
        let entries = [
            (vertical_id.as_str(), "plate-vertical"),
            (horizontal_id.as_str(), "plate-horizontal"),
        ];
        let loaded = match bundle.append_features_with_history(&entries, &history_event) {
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

    pub fn history(&self, root: impl AsRef<Path>) -> Result<HistoryState, HostError> {
        Ok(Bundle::at(root.as_ref()).open()?.history)
    }

    pub fn historical_edit(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        parameter: &str,
        value: f64,
    ) -> Result<HistoryCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let (event, evaluation) = loaded
            .history
            .historical_edit(feature_id, parameter, value)
            .map_err(|error| HostError::Validation {
                detail: error.to_string(),
            })?;
        self.commit_history_event_with_geometry(root, "historical-edit", event, Some(evaluation))
    }

    /// Report whether canonical undo and redo would succeed right now.
    ///
    /// The probe runs the pure cursor constructors without appending, so the
    /// TUI session can gate its transient history flags on canonical truth.
    pub fn history_availability(&self, root: impl AsRef<Path>) -> Result<(bool, bool), HostError> {
        let loaded = Bundle::at(root.as_ref()).open()?;
        Ok((
            loaded.history.undo(&loaded.history_events).is_ok(),
            loaded.history.redo(&loaded.history_events).is_ok(),
        ))
    }

    /// Move the active Revision Snapshot back one sealed transaction.
    pub fn undo(&self, root: impl AsRef<Path>) -> Result<HistoryCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let event = loaded
            .history
            .undo(&loaded.history_events)
            .map_err(|error| HostError::Validation {
                detail: error.to_string(),
            })?;
        self.commit_history_event_with_geometry(root, "undo", event, None)
    }

    /// Move the active Revision Snapshot forward over undone transactions.
    pub fn redo(&self, root: impl AsRef<Path>) -> Result<HistoryCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let event = loaded
            .history
            .redo(&loaded.history_events)
            .map_err(|error| HostError::Validation {
                detail: error.to_string(),
            })?;
        self.commit_history_event_with_geometry(root, "redo", event, None)
    }

    /// Commit one history event and recompute the committed geometry that
    /// moved with it.
    ///
    /// Worker recomputation stages before the sealed publication, and the
    /// history event plus every staged BREP replacement publish as one
    /// conditional bundle transaction: a worker failure rejects the
    /// operation with nothing committed, and a failed promotion or an
    /// intervening writer commits neither geometry nor history, so the
    /// active Revision Snapshot and current geometry cannot drift out of
    /// sync. Only families whose parameters changed and are all
    /// current-valid recompute; clean families keep byte-identical results
    /// and degraded families keep last-valid bytes under the stale geometry
    /// gate.
    fn commit_history_event_with_geometry(
        &self,
        root: &Path,
        operation: &str,
        event: threeterm_domain::history::HistoryEvent,
        evaluation: Option<HistoryEvaluation>,
    ) -> Result<HistoryCommitView, HostError> {
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let expected_revision = loaded.revision_hash_hex().to_string();
        let ordinal = loaded.history.event_ordinal() + 1;
        let staged = self.stage_changed_bracket_families(
            root,
            loaded.history.active_snapshot(),
            &event.active,
            &expected_revision,
        )?;
        let updated = if staged.is_empty() {
            bundle.append_features_with_history(&[], &event)?
        } else {
            let key = format!("history-{operation}-{ordinal}");
            let kinds = staged
                .iter()
                .map(|staged| bracket_kind(&staged.request))
                .collect::<Vec<_>>();
            let payloads = staged
                .iter()
                .map(|staged| {
                    history_recompute_idempotency_payload(
                        &staged.family,
                        &staged.request,
                        &staged.source_brep_sha256,
                        &staged.source_revision,
                        &staged.result_sha256,
                    )
                })
                .collect::<Vec<_>>();
            let families = staged
                .iter()
                .enumerate()
                .map(|(index, staged)| HistoryBrepReplacement {
                    feature_id: staged.family.as_str(),
                    kind: kinds[index].as_str(),
                    brep_bytes: staged.bytes.as_slice(),
                    expected_source_sha256: staged.source_brep_sha256.as_str(),
                    idempotency_key: key.as_str(),
                    idempotency_payload: payloads[index].as_str(),
                })
                .collect::<Vec<_>>();
            bundle.replace_bracket_families_with_history_if_revision(
                &families,
                &expected_revision,
                &event,
            )?
        };
        let snapshot = SnapshotView::from(&updated);
        let history = updated.history.clone();
        self.current.replace(Some(updated));
        Ok(HistoryCommitView {
            snapshot,
            history,
            evaluation,
        })
    }

    /// Run the production bracket worker for every committed family whose
    /// parameters changed between two history snapshots.
    fn stage_changed_bracket_families(
        &self,
        root: &Path,
        before: &HistorySnapshot,
        after: &HistorySnapshot,
        source_revision: &str,
    ) -> Result<Vec<StagedBracketFamily>, HostError> {
        let mut families = BTreeSet::new();
        families.extend(bracket_family_ids(before));
        families.extend(bracket_family_ids(after));
        let mut changed = Vec::new();
        for family in families {
            if committed_brep_path(root, &family).is_file()
                && let (Some(before_params), Some(after_params)) = (
                    bracket_family_params(before, &family),
                    bracket_family_params(after, &family),
                )
                && before_params != after_params
                && bracket_family_is_current(after, &family)
                && let Ok(source_brep_sha256) = sha256_path(&committed_brep_path(root, &family))
            {
                changed.push((family, after_params, source_brep_sha256));
            }
        }
        if changed.is_empty() {
            return Ok(Vec::new());
        }
        let worker = OcctWorker::locate().map_err(HostError::from)?;
        let mut staged = Vec::with_capacity(changed.len());
        for (family, params, source_brep_sha256) in changed {
            let stage = preview_stage_path(root, &format!("history-recompute-{family}"));
            fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
                detail: format!("create history recompute stage failed: {error}"),
            })?;
            let request = BracketRequest::new(
                threeterm_occt_worker::new_request_id(),
                params.length,
                params.width,
                params.height,
                params.thickness,
            )
            .with_feature_id(&family)
            .with_output_path(&stage, "recompute.brep");
            request
                .validate()
                .map_err(|detail| HostError::Validation { detail })?;
            let result = match worker
                .clone()
                .with_revision_id(source_revision.to_string())
                .bracket(&request)
            {
                Ok(result) if result.is_success() => result,
                Ok(result) => {
                    remove_preview_stage(&stage);
                    return Err(HostError::BrepInvalid {
                        request_id: Some(request.request_id.clone()),
                        detail: format!("history recompute returned status {}", result.status),
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
            let result_sha256 = result.brep_sha256.clone();
            remove_preview_stage(&stage);
            staged.push(StagedBracketFamily {
                family,
                request,
                bytes,
                result_sha256,
                source_brep_sha256,
                source_revision: source_revision.to_string(),
            });
        }
        Ok(staged)
    }

    pub fn timeline(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
    ) -> Result<HistoryTimelineView, HostError> {
        let bundle = Bundle::at(root.as_ref());
        let loaded = bundle.open()?;
        let timeline =
            loaded
                .feature_timeline(feature_id)
                .map_err(|error| HostError::Validation {
                    detail: error.to_string(),
                })?;
        Ok(HistoryTimelineView {
            snapshot: SnapshotView::from(&loaded),
            timeline,
        })
    }

    pub fn create_named_revision(
        &self,
        root: impl AsRef<Path>,
        name: &str,
    ) -> Result<HistoryCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let event = loaded
            .history
            .create_named_revision(name)
            .map_err(|error| HostError::Validation {
                detail: error.to_string(),
            })?;
        let updated = match bundle.append_features_with_history(&[], &event) {
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
        let snapshot = SnapshotView::from(&updated);
        let history = updated.history.clone();
        self.current.replace(Some(updated));
        Ok(HistoryCommitView {
            snapshot,
            history,
            evaluation: None,
        })
    }

    pub fn restore_named_revision(
        &self,
        root: impl AsRef<Path>,
        feature_id: &str,
        name: &str,
    ) -> Result<HistoryCommitView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let event = loaded
            .history
            .restore_named_revision_for_feature(feature_id, name)
            .map_err(|error| HostError::Validation {
                detail: error.to_string(),
            })?;
        self.commit_history_event_with_geometry(root, "restore-revision", event, None)
    }

    pub fn verify_history_replay(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<ReplayVerification, HostError> {
        let bundle = Bundle::at(root.as_ref());
        let loaded = bundle.open()?;
        let reconstructed = replay_canonical_state(&loaded.log)?;
        let reconstructed_model = canonical_model_fingerprint_from_state(&loaded, &reconstructed);
        let (first, second) = bundle.replay_history_states()?;
        let first_fingerprint = first.fingerprint();
        let has_geometry_intent = loaded
            .log
            .entries()
            .iter()
            .any(|entry| entry.intent.is_some());
        let expected_geometry = canonical_geometry_fingerprints(&loaded);
        let recomputed = if has_geometry_intent {
            let worker = OcctWorker::locate().map_err(HostError::from)?;
            Some(self.reload_and_recompute_geometry(root.as_ref(), &worker)?)
        } else {
            None
        };
        let first_model = recomputed.as_ref().map_or_else(
            || canonical_model_fingerprint(&loaded),
            |replayed| replayed.model_state_fingerprint.clone(),
        );
        let second_loaded = bundle.open()?;
        let mut mismatch = if first == second
            && first == loaded.history
            && loaded.graph == second_loaded.graph
            && loaded.components == second_loaded.components
            && reconstructed_model == canonical_model_fingerprint(&loaded)
            && first_model == reconstructed_model
            && reconstructed_model == canonical_model_fingerprint(&second_loaded)
        {
            None
        } else {
            Some("canonical model replay differs from canonical state".to_string())
        };
        let (geometry_fingerprints, geometry_mismatch) = if let Some(replayed) = recomputed {
            // Intent-backed BREP files are disposable. Recompute them before
            // checking the authenticated files, otherwise a missing result
            // would prevent the very replay that is meant to restore it.
            let authenticated = authenticated_geometry_fingerprints(root.as_ref(), &loaded)?;
            let mismatch = authenticated.1.or_else(|| {
                (authenticated.0 != expected_geometry)
                    .then_some("recomputed geometry differs from canonical provenance".to_string())
            });
            (
                authenticated.0,
                mismatch.or_else(|| {
                    (geometry_fingerprint_map(
                        &replayed.feature_ids,
                        &replayed.geometry_fingerprints,
                    ) != geometry_fingerprint_map_from_loaded(&loaded))
                    .then_some("recomputed geometry differs from canonical provenance".to_string())
                }),
            )
        } else {
            let authenticated = authenticated_geometry_fingerprints(root.as_ref(), &loaded)?;
            let mismatch = authenticated.1.or_else(|| {
                (authenticated.0 != expected_geometry).then_some(
                    "authenticated geometry differs from canonical provenance".to_string(),
                )
            });
            (authenticated.0, mismatch)
        };
        if mismatch.is_none() {
            mismatch = geometry_mismatch;
        }
        Ok(ReplayVerification {
            deterministic: mismatch.is_none(),
            fingerprint: first_fingerprint,
            model_state_fingerprint: first_model,
            geometry_fingerprints,
            mismatch,
        })
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

    /// Open a bundle through the production reload path, replaying canonical
    /// extrude intent when its disposable results are missing.
    pub fn load_with_extrude_replay(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<SnapshotView, HostError> {
        self.load_with_geometry_replay(root)
    }

    /// Open a bundle through the production reload path, replaying canonical
    /// geometry intent (extrude and Boolean) when disposable results are
    /// missing.
    pub fn load_with_geometry_replay(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        let view = self.load(root)?;
        let loaded = Bundle::at(root).open()?;
        let replay_needed = loaded
            .log
            .entries()
            .iter()
            .filter_map(|entry| entry.intent.as_ref())
            .any(|intent| {
                intent.affected_semantic_ids().iter().any(|feature_id| {
                    !bundle_root(root)
                        .join(BREP_SUBDIR)
                        .join(format!("{feature_id}.brep"))
                        .is_file()
                })
            });
        if !replay_needed {
            return Ok(view);
        }
        let worker = OcctWorker::locate().map_err(HostError::from)?;
        Ok(self.reload_and_recompute_geometry(root, &worker)?.snapshot)
    }

    /// Reload canonical extrude intents and rebuild their disposable BREP
    /// results. Replay never appends a transaction or changes the revision.
    pub fn reload_and_recompute_extrudes(
        &self,
        root: impl AsRef<Path>,
        worker: &OcctWorker,
    ) -> Result<ExtrudeReplayView, HostError> {
        let replayed = self.reload_and_recompute_geometry(root.as_ref(), worker)?;
        // The extrude-only view is the geometry replay restricted to extrude
        // intents; geometry order preserves log order so extrude entries keep
        // their relative positions.
        let loaded = Bundle::at(root.as_ref()).open()?;
        let extrude_ids: std::collections::HashSet<String> = loaded
            .log
            .entries()
            .iter()
            .filter_map(|entry| match entry.intent.as_ref() {
                Some(CanonicalIntent::Extrude(extrude)) => {
                    extrude.affected_semantic_ids.first().cloned()
                }
                _ => None,
            })
            .collect();
        let mut feature_ids = Vec::new();
        let mut geometry_fingerprints = Vec::new();
        for (index, feature_id) in replayed.feature_ids.iter().enumerate() {
            if extrude_ids.contains(feature_id) {
                feature_ids.push(feature_id.clone());
                geometry_fingerprints.push(replayed.geometry_fingerprints[index].clone());
            }
        }
        Ok(ExtrudeReplayView {
            snapshot: replayed.snapshot,
            model_state_fingerprint: replayed.model_state_fingerprint,
            recomputed: feature_ids.len(),
            feature_ids,
            geometry_fingerprints,
        })
    }

    /// Reload canonical Boolean intents and rebuild their disposable BREP
    /// results in log order. Replay never appends a transaction or changes
    /// the revision.
    pub fn reload_and_recompute_booleans(
        &self,
        root: impl AsRef<Path>,
        worker: &OcctWorker,
    ) -> Result<ExtrudeReplayView, HostError> {
        let replayed = self.reload_and_recompute_geometry(root.as_ref(), worker)?;
        let loaded = Bundle::at(root.as_ref()).open()?;
        let boolean_ids: std::collections::HashSet<String> = loaded
            .log
            .entries()
            .iter()
            .filter_map(|entry| match entry.intent.as_ref() {
                Some(CanonicalIntent::Boolean(boolean)) => {
                    boolean.affected_semantic_ids.first().cloned()
                }
                _ => None,
            })
            .collect();
        let mut feature_ids = Vec::new();
        let mut geometry_fingerprints = Vec::new();
        for (index, feature_id) in replayed.feature_ids.iter().enumerate() {
            if boolean_ids.contains(feature_id) {
                feature_ids.push(feature_id.clone());
                geometry_fingerprints.push(replayed.geometry_fingerprints[index].clone());
            }
        }
        Ok(ExtrudeReplayView {
            snapshot: replayed.snapshot,
            model_state_fingerprint: replayed.model_state_fingerprint,
            recomputed: feature_ids.len(),
            feature_ids,
            geometry_fingerprints,
        })
    }

    /// Reload all canonical geometry intents (extrude and Boolean) in log
    /// order and rebuild their disposable BREP results. Boolean intents
    /// follow their operands topologically because the log is appended in
    /// dependency order. Replay never appends a transaction or changes the
    /// revision.
    pub fn reload_and_recompute_geometry(
        &self,
        root: impl AsRef<Path>,
        worker: &OcctWorker,
    ) -> Result<ExtrudeReplayView, HostError> {
        let root = root.as_ref();
        let loaded = Bundle::at(root).open()?;
        let source_snapshot = SnapshotView::from(&loaded);
        let source_model_state = canonical_model_fingerprint(&loaded);
        let intents = loaded
            .log
            .entries()
            .iter()
            .filter_map(|entry| entry.intent.clone())
            .collect::<Vec<_>>();
        let mut feature_ids = Vec::with_capacity(intents.len());
        let mut geometry_fingerprints = Vec::with_capacity(intents.len());
        let mut replayed_paths = HashMap::with_capacity(intents.len());
        for intent in intents {
            match intent {
                CanonicalIntent::Extrude(extrude) => {
                    let expected_worker = expected_occt_worker_fingerprint();
                    if extrude.worker_requirements != expected_worker {
                        return Err(HostError::WorkerUnavailable {
                            detail: format!(
                                "incompatible extrude worker requirements: expected {expected_worker:?}, found {:?}",
                                extrude.worker_requirements
                            ),
                        });
                    }
                    let feature_id =
                        extrude
                            .affected_semantic_ids
                            .first()
                            .cloned()
                            .ok_or_else(|| HostError::Validation {
                                detail: "extrude intent has no affected feature".to_string(),
                            })?;
                    if !loaded.graph.contains_feature(&feature_id) {
                        return Err(HostError::Validation {
                            detail: format!(
                                "extrude replay feature is not in the Revision Snapshot: {feature_id}"
                            ),
                        });
                    }
                    let stage =
                        Stage::create_fresh(root.join(".derived"), "replay").map_err(|error| {
                            HostError::BrepIo {
                                detail: format!("create extrude replay stage failed: {error}"),
                            }
                        })?;
                    let stage_root = stage.root().to_path_buf();
                    let request = ExtrudeRequest::new(
                        extrude.request_id.clone(),
                        extrude
                            .deterministic_inputs
                            .profile
                            .iter()
                            .map(|[x, y]| (*x, *y))
                            .collect(),
                        extrude.deterministic_inputs.height,
                    )
                    .with_mode(parse_extrude_mode_value(if extrude.mode.is_empty() {
                        &extrude.operation
                    } else {
                        &extrude.mode
                    })?)
                    .with_optional_target_feature_id(extrude.target_feature_id.clone())
                    .with_output_path(&stage_root, "replay.brep.partial")
                    .with_feature_id(&feature_id);
                    let target_path = if let Some(target_feature_id) = &extrude.target_feature_id {
                        let path = replayed_paths
                            .get(target_feature_id)
                            .cloned()
                            .unwrap_or_else(|| {
                                root.join(BREP_SUBDIR)
                                    .join(format!("{target_feature_id}.brep"))
                            });
                        if !path.is_file() {
                            let _ = stage.discard();
                            return Err(HostError::BrepFileMissing { path });
                        }
                        let target_entry = loaded
                    .log
                    .entries()
                    .iter()
                    .rev()
                    .find(|entry| {
                        entry.feature_id == *target_feature_id
                            && entry.brep_byte_count.is_some()
                            && entry.brep_sha256.is_some()
                    })
                    .ok_or_else(|| HostError::Validation {
                        detail: format!(
                            "extrude replay target has no authenticated geometry: {target_feature_id}"
                        ),
                    })?;
                        let expected_bytes = target_entry
                            .brep_byte_count
                            .and_then(|value| usize::try_from(value).ok())
                            .ok_or_else(|| HostError::BrepIo {
                                detail: "extrude replay target BREP byte count is invalid"
                                    .to_string(),
                            })?;
                        let expected_sha256 =
                            target_entry.brep_sha256.as_deref().ok_or_else(|| {
                                HostError::BrepIo {
                                    detail: "extrude replay target BREP digest is missing"
                                        .to_string(),
                                }
                            })?;
                        read_brep_verified(&path, Some((expected_bytes, expected_sha256)))
                            .map_err(|detail| HostError::BrepIo { detail })?;
                        Some(path)
                    } else {
                        None
                    };
                    let request = request.with_optional_target_path(target_path);
                    let mut binding = extrude_artifact_request(&request, &source_snapshot)?;
                    binding.source_revision_id = extrude.source_revision.clone();
                    binding.staging_name = "replay.brep".to_string();
                    let request = request.with_artifact_request(binding);
                    let result = worker
                        .clone()
                        .with_expected_worker_id("occt")
                        .with_revision_id(extrude.source_revision.clone())
                        .extrude(&request)
                        .map_err(HostError::from);
                    let result = match result {
                        Ok(result) => result,
                        Err(error) => {
                            let _ = stage.discard();
                            return Err(error);
                        }
                    };
                    if result.schema_version != expected_worker.worker_schema_version {
                        let _ = stage.discard();
                        return Err(HostError::WorkerUnavailable {
                            detail: format!(
                                "incompatible extrude worker schema: expected {:?}, found {:?}",
                                expected_worker.worker_schema_version, result.schema_version
                            ),
                        });
                    }
                    let bytes = match read_brep_verified(
                        &result.brep_path,
                        Some((result.brep_bytes, &result.brep_sha256)),
                    ) {
                        Ok(bytes) => bytes,
                        Err(detail) => {
                            let _ = stage.discard();
                            return Err(HostError::BrepIo { detail });
                        }
                    };
                    if result.source_revision_id.as_deref()
                        != Some(extrude.source_revision.as_str())
                    {
                        let _ = stage.discard();
                        return Err(HostError::WorkerUnavailable {
                            detail: format!(
                                "incompatible extrude source revision: expected {:?}, found {:?}",
                                extrude.source_revision, result.source_revision_id
                            ),
                        });
                    }
                    let path = match Bundle::at(root).restore_derived_brep_if_revision(
                        &feature_id,
                        &source_snapshot.revision_hash,
                        &bytes,
                    ) {
                        Ok(path) => path,
                        Err(error) => {
                            let _ = stage.discard();
                            return Err(error.into());
                        }
                    };
                    let _ = stage.discard();
                    replayed_paths.insert(feature_id.clone(), path.clone());
                    feature_ids.push(feature_id);
                    geometry_fingerprints.push(sha256_path(&path).map_err(|error| {
                        HostError::BrepIo {
                            detail: format!("hash replayed BREP failed: {error}"),
                        }
                    })?);
                }
                CanonicalIntent::Boolean(boolean) => {
                    let expected_worker = expected_occt_worker_fingerprint();
                    if boolean.worker_requirements != expected_worker {
                        return Err(HostError::WorkerUnavailable {
                            detail: format!(
                                "incompatible boolean worker requirements: expected {expected_worker:?}, found {:?}",
                                boolean.worker_requirements
                            ),
                        });
                    }
                    let feature_id =
                        boolean
                            .affected_semantic_ids
                            .first()
                            .cloned()
                            .ok_or_else(|| HostError::Validation {
                                detail: "boolean intent has no affected feature".to_string(),
                            })?;
                    if !loaded.graph.contains_feature(&feature_id) {
                        return Err(HostError::Validation {
                            detail: format!(
                                "boolean replay feature is not in the Revision Snapshot: {feature_id}"
                            ),
                        });
                    }
                    // Operands precede the boolean in log order, so a replayed
                    // operand path wins over the on-disk file.
                    let operand_path = |operand_id: &str| -> Result<PathBuf, HostError> {
                        if let Some(path) = replayed_paths.get(operand_id) {
                            return Ok(path.clone());
                        }
                        let path = root.join(BREP_SUBDIR).join(format!("{operand_id}.brep"));
                        if !path.is_file() {
                            return Err(HostError::BrepFileMissing { path });
                        }
                        let entry = loaded
                            .log
                            .entries()
                            .iter()
                            .rev()
                            .find(|entry| {
                                entry.feature_id == operand_id
                                    && entry.brep_byte_count.is_some()
                                    && entry.brep_sha256.is_some()
                            })
                            .ok_or_else(|| HostError::Validation {
                                detail: format!(
                                    "boolean replay operand has no authenticated geometry: {operand_id}"
                                ),
                            })?;
                        let expected_bytes = entry
                            .brep_byte_count
                            .and_then(|value| usize::try_from(value).ok())
                            .ok_or_else(|| HostError::BrepIo {
                                detail: "boolean replay operand BREP byte count is invalid"
                                    .to_string(),
                            })?;
                        let expected_sha256 =
                            entry
                                .brep_sha256
                                .as_deref()
                                .ok_or_else(|| HostError::BrepIo {
                                    detail: "boolean replay operand BREP digest is missing"
                                        .to_string(),
                                })?;
                        read_brep_verified(&path, Some((expected_bytes, expected_sha256)))
                            .map_err(|detail| HostError::BrepIo { detail })?;
                        Ok(path)
                    };
                    let base_path = operand_path(&boolean.base_feature_id)?;
                    let tool_path = operand_path(&boolean.tool_feature_id)?;
                    let stage =
                        Stage::create_fresh(root.join(".derived"), "replay").map_err(|error| {
                            HostError::BrepIo {
                                detail: format!("create boolean replay stage failed: {error}"),
                            }
                        })?;
                    let stage_root = stage.root().to_path_buf();
                    // Run the worker without capturing `stage` in closures:
                    // every failure path discards explicitly, mirroring the
                    // extrude replay arm above.
                    let run_replay =
                        |operation: &str| -> Result<(Vec<u8>, String, usize), HostError> {
                            let invoke = |brep_path: &std::path::Path,
                                      brep_bytes: usize,
                                      brep_sha256: &str,
                                      schema_version: &str|
                         -> Result<(Vec<u8>, String, usize), HostError> {
                            if schema_version != expected_worker.worker_schema_version {
                                return Err(HostError::WorkerUnavailable {
                                    detail: format!(
                                        "incompatible boolean worker schema: expected {:?}, found {:?}",
                                        expected_worker.worker_schema_version, schema_version
                                    ),
                                });
                            }
                            let bytes = read_brep_verified(
                                brep_path,
                                Some((brep_bytes, brep_sha256)),
                            )
                            .map_err(|detail| HostError::BrepIo { detail })?;
                            Ok((bytes, brep_sha256.to_string(), brep_bytes))
                        };
                            match operation {
                                "fuse" => {
                                    let request = BooleanFuseRequest::new(
                                        boolean.request_id.clone(),
                                        &base_path,
                                        &tool_path,
                                    )
                                    .with_output_path(&stage_root, "replay.brep.partial")
                                    .with_feature_id(&feature_id);
                                    let result = worker
                                        .clone()
                                        .with_expected_worker_id("occt")
                                        .with_revision_id(boolean.source_revision.clone())
                                        .boolean_fuse(&request)
                                        .map_err(HostError::from)?;
                                    invoke(
                                        &result.brep_path,
                                        result.brep_bytes,
                                        &result.brep_sha256,
                                        &result.schema_version,
                                    )
                                }
                                "cut" => {
                                    let request = BooleanCutRequest::new(
                                        boolean.request_id.clone(),
                                        &base_path,
                                        &tool_path,
                                    )
                                    .with_output_path(&stage_root, "replay.brep.partial")
                                    .with_feature_id(&feature_id);
                                    let result = worker
                                        .clone()
                                        .with_expected_worker_id("occt")
                                        .with_revision_id(boolean.source_revision.clone())
                                        .boolean_cut(&request)
                                        .map_err(HostError::from)?;
                                    invoke(
                                        &result.brep_path,
                                        result.brep_bytes,
                                        &result.brep_sha256,
                                        &result.schema_version,
                                    )
                                }
                                "common" => {
                                    let request = BooleanCommonRequest::new(
                                        boolean.request_id.clone(),
                                        &base_path,
                                        &tool_path,
                                    )
                                    .with_output_path(&stage_root, "replay.brep.partial")
                                    .with_feature_id(&feature_id);
                                    let result = worker
                                        .clone()
                                        .with_expected_worker_id("occt")
                                        .with_revision_id(boolean.source_revision.clone())
                                        .boolean_common(&request)
                                        .map_err(HostError::from)?;
                                    invoke(
                                        &result.brep_path,
                                        result.brep_bytes,
                                        &result.brep_sha256,
                                        &result.schema_version,
                                    )
                                }
                                other => Err(HostError::Validation {
                                    detail: format!("unknown boolean replay operation: {other}"),
                                }),
                            }
                        };
                    let (bytes, _result_sha, _result_bytes) =
                        match run_replay(boolean.operation.as_str()) {
                            Ok(replayed) => replayed,
                            Err(error) => {
                                let _ = stage.discard();
                                return Err(error);
                            }
                        };
                    let path = match Bundle::at(root).restore_derived_brep_if_revision(
                        &feature_id,
                        &source_snapshot.revision_hash,
                        &bytes,
                    ) {
                        Ok(path) => path,
                        Err(error) => {
                            let _ = stage.discard();
                            return Err(error.into());
                        }
                    };
                    let _ = stage.discard();
                    replayed_paths.insert(feature_id.clone(), path.clone());
                    feature_ids.push(feature_id);
                    geometry_fingerprints.push(sha256_path(&path).map_err(|error| {
                        HostError::BrepIo {
                            detail: format!("hash replayed BREP failed: {error}"),
                        }
                    })?);
                }
                CanonicalIntent::Hole(hole) => {
                    let expected_worker = expected_occt_worker_fingerprint();
                    if hole.worker_requirements != expected_worker {
                        return Err(HostError::WorkerUnavailable {
                            detail: format!(
                                "incompatible hole worker requirements: expected {expected_worker:?}, found {:?}",
                                hole.worker_requirements
                            ),
                        });
                    }
                    let feature_id =
                        hole.affected_semantic_ids.first().cloned().ok_or_else(|| {
                            HostError::Validation {
                                detail: "hole intent has no affected feature".to_string(),
                            }
                        })?;
                    if !loaded.graph.contains_feature(&feature_id) {
                        return Err(HostError::Validation {
                            detail: format!(
                                "hole replay feature is not in the Revision Snapshot: {feature_id}"
                            ),
                        });
                    }
                    let base_path = if let Some(path) = replayed_paths.get(&hole.base_feature_id) {
                        path.clone()
                    } else {
                        let path = root
                            .join(BREP_SUBDIR)
                            .join(format!("{}.brep", hole.base_feature_id));
                        if !path.is_file() {
                            return Err(HostError::BrepFileMissing { path });
                        }
                        let entry = loaded
                            .log
                            .entries()
                            .iter()
                            .rev()
                            .find(|entry| {
                                entry.feature_id == hole.base_feature_id
                                    && entry.brep_byte_count.is_some()
                                    && entry.brep_sha256.is_some()
                            })
                            .ok_or_else(|| HostError::Validation {
                                detail: format!(
                                    "hole replay base has no authenticated geometry: {}",
                                    hole.base_feature_id
                                ),
                            })?;
                        let expected_bytes = entry
                            .brep_byte_count
                            .and_then(|value| usize::try_from(value).ok())
                            .ok_or_else(|| HostError::BrepIo {
                                detail: "hole replay base BREP byte count is invalid".to_string(),
                            })?;
                        let expected_sha256 =
                            entry
                                .brep_sha256
                                .as_deref()
                                .ok_or_else(|| HostError::BrepIo {
                                    detail: "hole replay base BREP digest is missing".to_string(),
                                })?;
                        read_brep_verified(&path, Some((expected_bytes, expected_sha256)))
                            .map_err(|detail| HostError::BrepIo { detail })?;
                        path
                    };
                    let stage =
                        Stage::create_fresh(root.join(".derived"), "replay").map_err(|error| {
                            HostError::BrepIo {
                                detail: format!("create hole replay stage failed: {error}"),
                            }
                        })?;
                    let stage_root = stage.root().to_path_buf();
                    let mut request = HoleRequest::new(
                        hole.request_id.clone(),
                        &base_path,
                        hole.deterministic_inputs.position,
                        hole.deterministic_inputs.direction,
                        hole.deterministic_inputs.diameter,
                    )
                    .with_hole_kind(hole.hole_kind.clone())
                    .with_base_feature_id(hole.base_feature_id.clone())
                    .with_output_path(&stage_root, "replay.brep.partial")
                    .with_feature_id(&feature_id);
                    if hole.hole_kind == "tapped" {
                        request = request.with_thread(
                            hole.deterministic_inputs
                                .thread_designation
                                .clone()
                                .unwrap_or_default(),
                            hole.deterministic_inputs.thread_pitch.unwrap_or_default(),
                            hole.deterministic_inputs.thread_depth.unwrap_or_default(),
                        );
                    }
                    let result = worker
                        .clone()
                        .with_expected_worker_id("occt")
                        .with_revision_id(hole.source_revision.clone())
                        .hole(&request)
                        .map_err(HostError::from);
                    let result = match result {
                        Ok(result) => result,
                        Err(error) => {
                            let _ = stage.discard();
                            return Err(error);
                        }
                    };
                    if result.schema_version != expected_worker.worker_schema_version {
                        let _ = stage.discard();
                        return Err(HostError::WorkerUnavailable {
                            detail: format!(
                                "incompatible hole worker schema: expected {:?}, found {:?}",
                                expected_worker.worker_schema_version, result.schema_version
                            ),
                        });
                    }
                    let bytes = match read_brep_verified(
                        &result.brep_path,
                        Some((result.brep_bytes, result.brep_sha256.as_str())),
                    ) {
                        Ok(bytes) => bytes,
                        Err(detail) => {
                            let _ = stage.discard();
                            return Err(HostError::BrepIo { detail });
                        }
                    };
                    let path = match Bundle::at(root).restore_derived_brep_if_revision(
                        &feature_id,
                        &source_snapshot.revision_hash,
                        &bytes,
                    ) {
                        Ok(path) => path,
                        Err(error) => {
                            let _ = stage.discard();
                            return Err(error.into());
                        }
                    };
                    let _ = stage.discard();
                    replayed_paths.insert(feature_id.clone(), path.clone());
                    feature_ids.push(feature_id);
                    geometry_fingerprints.push(sha256_path(&path).map_err(|error| {
                        HostError::BrepIo {
                            detail: format!("hash replayed BREP failed: {error}"),
                        }
                    })?);
                }
            }
        }
        let reloaded = Bundle::at(root).open()?;
        let snapshot = SnapshotView::from(&reloaded);
        if snapshot.revision_hash != source_snapshot.revision_hash
            || canonical_model_fingerprint(&reloaded) != source_model_state
        {
            return Err(HostError::Validation {
                detail: "geometry replay changed the canonical model state".to_string(),
            });
        }
        self.current.replace(Some(reloaded));
        Ok(ExtrudeReplayView {
            snapshot,
            model_state_fingerprint: source_model_state,
            recomputed: feature_ids.len(),
            feature_ids,
            geometry_fingerprints,
        })
    }

    /// Load a canonical bundle and validate its optional non-authoritative
    /// Layer 1 cache before replacing the in-memory snapshot.
    pub fn load_with_layer1_cache(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        if root.exists() && !root.is_dir() {
            return Err(HostError::BundlePathNotDirectory {
                path: root.to_path_buf(),
            });
        }
        let loaded = load(root).map_err(HostError::from)?;
        validate_layer1_cache(root, &loaded)?;
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    /// Exercise the v0 fail-closed reader without changing the current Host
    /// snapshot when the policy rejects the bundle.
    pub fn load_adversarial_v0(&self, root: impl AsRef<Path>) -> Result<SnapshotView, HostError> {
        let loaded = load_with_policy(root.as_ref(), LoadPolicy::RejectV0RequiresBackup)
            .map_err(HostError::from)?;
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
    }

    /// Rebuild the saved, non-authoritative L-bracket Layer 1 cache through
    /// the real OCCT worker without touching the Canonical Transaction Log.
    pub fn rebuild_lbracket_layer1_cache(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<Layer1CacheRebuild, HostError> {
        let root = root.as_ref();
        let loaded = Bundle::at(root).open()?;
        let cache = root.join(LAYER1_CACHE_DIR);
        fs::create_dir_all(&cache).map_err(|error| HostError::BrepIo {
            detail: format!("create Layer 1 cache failed: {error}"),
        })?;
        let artifact_name = "l-bracket.brep";
        let request = BracketRequest::new(
            format!("layer1-cache-{}", std::process::id()),
            60.0,
            30.0,
            40.0,
            3.0,
        )
        .with_output_path(&cache, artifact_name)
        .with_feature_id("l-bracket");
        let worker = OcctWorker::locate()
            .map_err(|error| HostError::WorkerUnavailable {
                detail: error.to_string(),
            })?
            .with_revision_id(loaded.revision_hash_hex());
        let result = worker.bracket(&request).map_err(HostError::from)?;
        if !result.is_success() || !result.brep_path.is_file() {
            return Err(HostError::BrepInvalid {
                request_id: Some(result.request_id),
                detail: "Layer 1 cache worker did not produce a BREP".to_string(),
            });
        }
        let sha256 = sha256_path(&result.brep_path).map_err(|error| HostError::BrepIo {
            detail: error.to_string(),
        })?;
        let record = Layer1CacheRecord {
            schema_version: LAYER1_CACHE_SCHEMA.to_string(),
            source_revision: loaded.revision_hash_hex().to_string(),
            operation: "bracket".to_string(),
            feature_id: "l-bracket".to_string(),
            worker_fingerprint: expected_occt_worker_fingerprint(),
            artifact_name: artifact_name.to_string(),
            byte_count: result.brep_bytes as u64,
            sha256,
        };
        let record_bytes =
            serde_json::to_vec_pretty(&record).map_err(|error| HostError::BrepIo {
                detail: format!("serialize Layer 1 cache record failed: {error}"),
            })?;
        let temporary = cache.join(format!(".{LAYER1_CACHE_RECORD}.tmp-{}", std::process::id()));
        fs::write(&temporary, record_bytes).map_err(|error| HostError::BrepIo {
            detail: format!("write Layer 1 cache record failed: {error}"),
        })?;
        fs::rename(temporary, cache.join(LAYER1_CACHE_RECORD)).map_err(|error| {
            HostError::BrepIo {
                detail: format!("publish Layer 1 cache record failed: {error}"),
            }
        })?;
        Ok(Layer1CacheRebuild {
            record,
            recomputations: 1,
        })
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

    /// Return the replayed component graph. This is a materialized view of
    /// canonical command transactions, never a separately persisted snapshot.
    pub fn component_graph(&self, root: impl AsRef<Path>) -> Result<ComponentGraph, HostError> {
        Ok(Bundle::at(root.as_ref()).open()?.components)
    }

    /// Capture one dependency-closed L-bracket feature subset as an immutable
    /// component definition. The selected history snapshot is checked again
    /// by persistence under its write lock before publication.
    pub fn capture_component(
        &self,
        root: impl AsRef<Path>,
        definition_id: &str,
        selected_feature_ids: &[String],
    ) -> Result<SnapshotView, HostError> {
        let root = root.as_ref();
        let bundle = Bundle::at(root);
        let loaded = bundle.open()?;
        let selected = canonical_selected_feature_ids(selected_feature_ids)?;
        let snapshot = loaded.history.active_snapshot();
        for feature_id in &selected {
            let feature =
                snapshot
                    .features
                    .get(feature_id)
                    .ok_or_else(|| HostError::Validation {
                        detail: format!("selected feature reference is lost: {feature_id}"),
                    })?;
            if feature.status != HistoryStatus::CurrentValid {
                return Err(HostError::Validation {
                    detail: format!("selected feature is not current-valid: {feature_id}"),
                });
            }
            if feature
                .dependencies
                .iter()
                .any(|dependency| !selected.contains(dependency))
            {
                return Err(HostError::Validation {
                    detail: format!(
                        "selected feature subset is not dependency-closed: {feature_id}"
                    ),
                });
            }
        }
        let descriptor = descriptor_for_selected_l_bracket(definition_id, &selected, snapshot)?;
        let command = ComponentCommand::Capture {
            definition_id: definition_id.to_string(),
            selected_feature_ids: selected,
            descriptor,
        };
        let expected_revision = loaded.revision_hash_hex().to_string();
        let updated =
            match bundle.append_component_command_if_revision(&command, &expected_revision) {
                Ok(updated) => updated,
                Err(error) => {
                    if let Ok(current) = bundle.open() {
                        self.current.replace(Some(current));
                    }
                    return Err(error.into());
                }
            };
        let view = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        Ok(view)
    }

    /// Validate and atomically append one component command. Validation occurs
    /// before persistence publication, so a non-resolved reference leaves both
    /// the host snapshot and sealed bundle unchanged.
    pub fn apply_component_command(
        &self,
        root: impl AsRef<Path>,
        command: ComponentCommand,
    ) -> Result<SnapshotView, HostError> {
        let bundle = Bundle::at(root.as_ref());
        let mut graph = if !bundle.canonical_root().exists()
            && !previous_generation_path(bundle.canonical_root()).exists()
        {
            ComponentGraph::default()
        } else {
            match bundle.open() {
                Ok(loaded) => loaded.components,
                Err(BundleError::BundlePathMissing { .. }) => ComponentGraph::default(),
                Err(error) => return Err(error.into()),
            }
        };
        graph
            .apply(&command)
            .map_err(|detail| HostError::Validation { detail })?;
        let loaded = bundle.append_component_command(&command)?;
        let view = SnapshotView::from(&loaded);
        self.current.replace(Some(loaded));
        Ok(view)
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

    /// Build the production viewport scene from the committed BREP artifacts
    /// in the currently loaded canonical generation.
    pub fn presentation_viewport_scene(&self) -> Result<ViewportScene, HostError> {
        let current = self
            .current
            .borrow()
            .clone()
            .ok_or_else(|| HostError::Validation {
                detail: "host has no canonical presentation snapshot".to_string(),
            })?;
        let revision = current.revision_hash_hex().to_string();
        let root = current.canonical_root.clone();
        let mut scene = ViewportScene::from_feature_graph(revision.clone(), &current.graph, None);
        let stage = std::env::temp_dir().join(format!(
            "threeterm-viewport-tessellation-{}-{}",
            std::process::id(),
            TESSELLATION_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&stage).map_err(|error| HostError::BrepIo {
            detail: format!("create viewport tessellation stage failed: {error}"),
        })?;
        let result = (|| {
            for feature in current.graph.features() {
                if !is_geometric_feature_kind(&feature.kind) {
                    continue;
                }
                let feature_id = feature.id.as_str();
                let brep = root.join(BREP_SUBDIR).join(format!("{feature_id}.brep"));
                if !brep.is_file() {
                    return Err(HostError::BrepFileMissing { path: brep });
                }
                let worker = OcctWorker::locate()
                    .map_err(HostError::from)?
                    .with_revision_id(revision.clone());
                let request =
                    ExportRequest::new(format!("viewport-tessellation-{feature_id}"), brep, 0.1)
                        .with_output_path(&stage, format!("{feature_id}.stl"))
                        .with_feature_id(feature_id);
                let exported = worker.export(&request).map_err(HostError::from)?;
                if !exported.is_success() || !exported.brep_path.is_file() {
                    return Err(HostError::BrepInvalid {
                        request_id: Some(request.request_id),
                        detail: format!(
                            "viewport tessellation did not produce a mesh: {feature_id}"
                        ),
                    });
                }
                let triangles = parse_ascii_stl(&exported.brep_path, feature_id)?;
                scene = scene.with_solid(SceneSolid::new(feature_id, triangles));
            }
            Ok(scene)
        })();
        let _ = fs::remove_dir_all(&stage);
        result
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
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        if self.has_draft(&root, &draft_id) {
            return Err(HostError::DraftAlreadyExists { draft_id });
        }
        let source_feature_id = source_feature_id.into();
        if !valid_feature_path_component(&source_feature_id) {
            return Err(HostError::DraftInvalid {
                draft_id,
                detail: "source_feature_id must be a plain feature id".to_string(),
            });
        }
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
            bundle_root: root.clone(),
            source_feature_id,
            source_revision: loaded.revision_hash_hex().to_string(),
            source_brep_sha256,
            request,
            preview_path: None,
            created_at: Instant::now(),
        };
        self.drafts
            .borrow_mut()
            .insert((root, draft_id), draft.clone());
        Ok(draft)
    }

    /// Replace the semantic values of a draft without changing its source
    /// binding. Any prior preview is invalidated before the new values land.
    pub fn update_draft(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        request: DraftRequest,
    ) -> Result<CommandDraft, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft_key = draft_map_key(&root, draft_id);
        let mut drafts = self.drafts.borrow_mut();
        let draft = drafts
            .get_mut(&draft_key)
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
        let draft_key = draft_map_key(&root, draft_id);
        let draft = self
            .drafts
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
        self.clear_draft_preview(&draft_key);
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
            .get_mut(&draft_key)
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
        let draft_key = draft_map_key(&root, draft_id);
        let draft = self
            .drafts
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
        self.clear_draft_preview(&draft_key);
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
        self.drafts.borrow_mut().remove(&draft_key);
        Ok(DraftCommitView {
            source_snapshot: None,
            snapshot,
            result,
            artifact: None,
        })
    }

    /// Refuse a draft and remove every transient preview artifact.
    pub fn discard_draft(&self, root: impl AsRef<Path>, draft_id: &str) -> Result<(), HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft_key = draft_map_key(&root, draft_id);
        let draft = self.drafts.borrow_mut().remove(&draft_key).ok_or_else(|| {
            HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            }
        })?;
        if let Some(path) = draft.preview_path {
            remove_preview_stage(&path);
        }
        Ok(())
    }

    pub fn has_draft(&self, root: impl AsRef<Path>, draft_id: &str) -> bool {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        self.drafts
            .borrow()
            .contains_key(&draft_map_key(&root, draft_id))
    }

    fn clear_draft_preview(&self, draft_key: &(PathBuf, String)) {
        if let Some(draft) = self.drafts.borrow_mut().get_mut(draft_key)
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

    fn stage_occt_result<R>(
        &self,
        root: &Path,
        request: &impl Serialize,
        operation: threeterm_occt_worker::Operation,
        worker: &OcctWorker,
    ) -> Result<StagedOcctResult<R>, HostError>
    where
        R: DeserializeOwned + Serialize,
    {
        self.stage_occt_result_inner(root, request, operation, worker, None, None)
    }

    fn stage_occt_result_with_cancel_and_progress<R>(
        &self,
        root: &Path,
        request: &impl Serialize,
        operation: threeterm_occt_worker::Operation,
        worker: &OcctWorker,
        cancel: &AtomicBool,
        on_progress: &mut dyn FnMut(&threeterm_protocol::supervisor::Progress),
    ) -> Result<StagedOcctResult<R>, HostError>
    where
        R: DeserializeOwned + Serialize,
    {
        self.stage_occt_result_inner(
            root,
            request,
            operation,
            worker,
            Some(cancel),
            Some(on_progress),
        )
    }

    fn stage_occt_result_inner<R>(
        &self,
        root: &Path,
        request: &impl Serialize,
        operation: threeterm_occt_worker::Operation,
        worker: &OcctWorker,
        cancel: Option<&AtomicBool>,
        on_progress: Option<&mut dyn FnMut(&threeterm_protocol::supervisor::Progress)>,
    ) -> Result<StagedOcctResult<R>, HostError>
    where
        R: DeserializeOwned + Serialize,
    {
        let mut request_value =
            serde_json::to_value(request).map_err(|error| HostError::Validation {
                detail: format!("{operation:?} request serialization failed: {error}"),
            })?;
        if let Some(selected_edge) = request_value.get("selected_edge").cloned() {
            for (target, source) in [
                ("selected_edge_id", "semantic_id"),
                ("source_feature_id", "source_feature_id"),
                ("source_revision_id", "source_revision_id"),
                ("source_edge_id", "source_edge_id"),
                ("selected_role", "role"),
            ] {
                if let Some(value) = selected_edge.get(source) {
                    request_value[target] = value.clone();
                }
            }
        }
        let canonical_request = request_value.clone();
        let request_id = request_value["request_id"]
            .as_str()
            .ok_or_else(|| HostError::Validation {
                detail: "OCCT request is missing request_id".to_string(),
            })?
            .to_string();
        let feature_id = request_value["feature_id"]
            .as_str()
            .ok_or_else(|| HostError::Validation {
                detail: "OCCT request is missing feature_id".to_string(),
            })?
            .to_string();
        worker.verify_identity().map_err(|error| match error {
            WorkerError::Spawn { binary, detail, .. } => HostError::WorkerFailure {
                request_id: Some(request_id.clone()),
                detail: format!("worker spawn failed at {}: {detail}", binary.display()),
            },
            other => HostError::from(other),
        })?;
        let source_snapshot = self.load(root)?;
        let binding = occt_artifact_request(
            &request_value,
            operation,
            &source_snapshot,
            &request_id,
            &feature_id,
        )?;
        let stage =
            Stage::create_fresh(root.join(".derived"), operation.as_str()).map_err(|error| {
                HostError::BrepIo {
                    detail: format!("create {operation:?} request stage failed: {error}"),
                }
            })?;
        request_value["output_dir"] =
            serde_json::Value::String(stage.root().to_string_lossy().into_owned());
        request_value["output_filename"] =
            serde_json::Value::String("pending.brep.partial".to_string());
        request_value["artifact_request"] = match serde_json::to_value(&binding) {
            Ok(value) => value,
            Err(error) => {
                let _ = stage.discard();
                return Err(HostError::Validation {
                    detail: format!("OCCT artifact binding serialization failed: {error}"),
                });
            }
        };
        let completion_result = match cancel {
            Some(cancel) => match on_progress {
                Some(on_progress) => worker
                    .clone()
                    .with_revision_id(source_snapshot.revision_hash.clone())
                    .invoke_staged_with_cancel_and_progress(
                        request_value,
                        operation,
                        stage,
                        cancel,
                        on_progress,
                    ),
                None => worker
                    .clone()
                    .with_revision_id(source_snapshot.revision_hash.clone())
                    .invoke_staged_with_cancel(request_value, operation, stage, cancel),
            },
            None => worker
                .clone()
                .with_revision_id(source_snapshot.revision_hash.clone())
                .invoke_staged(request_value, operation, stage),
        };
        let completion = match completion_result {
            Ok(completion) => completion,
            Err(error) => {
                return Err(HostError::from(error));
            }
        };
        let typed_result = match serde_json::from_value::<R>(completion.result.clone()) {
            Ok(result) => result,
            Err(error) => {
                let _ = completion.stage.discard();
                return Err(HostError::DerivedResult {
                    diagnostic: Diagnostic::artifact_promotion_failure(&format!(
                        "typed_result_schema_mismatch:{error}"
                    )),
                });
            }
        };
        let typed_value = match serde_json::to_value(&typed_result) {
            Ok(value) => value,
            Err(error) => {
                let _ = completion.stage.discard();
                return Err(HostError::Validation {
                    detail: format!("typed OCCT result serialization failed: {error}"),
                });
            }
        };
        if typed_value["status"].as_str() != Some("ok") {
            let _ = completion.stage.discard();
            return Err(HostError::BrepInvalid {
                request_id: typed_value["request_id"].as_str().map(str::to_string),
                detail: format!(
                    "{} returned non-ok status: status={} feature_id={}",
                    operation.as_str(),
                    typed_value["status"].as_str().unwrap_or("unknown"),
                    typed_value["feature_id"].as_str().unwrap_or("unknown"),
                ),
            });
        }
        let artifact = self
            .accept_staged_occt_result(
                completion.stage,
                &binding,
                operation,
                &typed_value,
                completion.outcome,
            )
            .map_err(|diagnostic| HostError::DerivedResult { diagnostic })?;
        Ok(StagedOcctResult {
            source_snapshot,
            request: canonical_request,
            result: typed_result,
            artifact,
        })
    }

    fn accept_staged_occt_result(
        &self,
        stage: Stage,
        binding: &Layer1ArtifactRequest,
        operation: threeterm_occt_worker::Operation,
        typed_result: &serde_json::Value,
        outcome: SupervisorOutcome,
    ) -> Result<Layer1DerivedResult, Diagnostic> {
        let stage_root = stage.root().to_path_buf();
        let SupervisorOutcome::Completed {
            result, request_id, ..
        } = &outcome
        else {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("worker_result_not_completed"),
            ));
        };
        if !json_values_match_worker_result(result, typed_result) {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_does_not_match_completion"),
            ));
        }
        let expected_path = stage_root.join(format!("{}.partial", binding.staging_name));
        if request_id != &binding.request_id
            || typed_result["request_id"].as_str() != Some(binding.request_id.as_str())
            || typed_result["operation"].as_str() != Some(operation.as_str())
            || typed_result["feature_id"].as_str() != Some(binding.feature_id.as_str())
            || typed_result["status"].as_str() != Some("ok")
            || typed_result["brep_path"].as_str() != Some(expected_path.to_string_lossy().as_ref())
            || typed_result["brep_bytes"].as_u64().is_none()
            || typed_result["brep_sha256"].as_str().is_none()
        {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_identity_mismatch"),
            ));
        }
        self.accept_derived_result(
            stage.root(),
            binding,
            &expected_occt_worker_fingerprint(),
            outcome,
        )
    }

    fn promote_occt_result<R>(
        &self,
        root: &Path,
        derived: StagedOcctResult<R>,
    ) -> Result<(SnapshotView, R, Layer1DerivedResult), HostError>
    where
        R: DeserializeOwned + Serialize,
    {
        self.promote_occt_result_with_append(
            root,
            derived,
            |bundle, current, derived, artifact, bytes, provenance| {
                let feature_id = &artifact.feature_id;
                let kind = if artifact.operation == "hole" {
                    let hole_kind = derived
                        .request
                        .get("hole_kind")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("drilled");
                    format!("hole:{hole_kind}")
                } else {
                    format!("brep:{feature_id}")
                };
                if artifact.operation == "extrude" {
                    let intent = canonical_extrude_intent(
                        &derived.request,
                        &derived.source_snapshot,
                        artifact,
                    )
                    .map_err(|error| BundleError::Invalid(error.to_string()))?;
                    bundle.append_new_feature_with_brep_if_revision_and_provenance_and_intent(
                        feature_id,
                        &kind,
                        &current.manifest.revision_hash,
                        &artifact.request_id,
                        provenance,
                        &CanonicalIntent::Extrude(intent),
                        bytes,
                    )
                } else if matches!(
                    artifact.operation.as_str(),
                    "boolean_fuse" | "boolean_cut" | "boolean_common"
                ) {
                    let intent = canonical_boolean_intent(
                        &derived.request,
                        &derived.source_snapshot,
                        artifact,
                    )
                    .map_err(|error| BundleError::Invalid(error.to_string()))?;
                    bundle.append_new_feature_with_brep_if_revision_and_provenance_and_intent(
                        feature_id,
                        &kind,
                        &current.manifest.revision_hash,
                        &artifact.request_id,
                        provenance,
                        &CanonicalIntent::Boolean(intent),
                        bytes,
                    )
                } else if artifact.operation == "hole" {
                    let base_feature_id = derived
                        .request
                        .get("base_feature_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            BundleError::Invalid(
                                "canonical hole base feature is missing".to_string(),
                            )
                        })?;
                    if !current.graph.contains_feature(base_feature_id) {
                        return Err(BundleError::Invalid(format!(
                            "canonical hole base feature is missing: {base_feature_id}"
                        )));
                    }
                    let intent = canonical_hole_intent(
                        &derived.request,
                        base_feature_id,
                        &derived.source_snapshot,
                        artifact,
                    )
                    .map_err(|error| BundleError::Invalid(error.to_string()))?;
                    bundle.append_new_feature_with_brep_if_revision_and_hole_intent(
                        feature_id,
                        &kind,
                        &current.manifest.revision_hash,
                        &artifact.request_id,
                        provenance,
                        &intent,
                        bytes,
                    )
                } else {
                    bundle.append_new_feature_with_brep_if_revision_and_provenance(
                        feature_id,
                        &kind,
                        &current.manifest.revision_hash,
                        &artifact.request_id,
                        provenance,
                        bytes,
                    )
                }
            },
        )
    }

    fn promote_occt_result_with_append<R, F>(
        &self,
        root: &Path,
        derived: StagedOcctResult<R>,
        append: F,
    ) -> Result<(SnapshotView, R, Layer1DerivedResult), HostError>
    where
        R: DeserializeOwned + Serialize,
        F: FnOnce(
            &Bundle,
            &LoadedBundle,
            &StagedOcctResult<R>,
            &Layer1DerivedResult,
            &[u8],
            &str,
        ) -> Result<LoadedBundle, BundleError>,
    {
        let derived_root = root.join(".derived");
        let stage_root = derived
            .artifact
            .path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(
                    "derived_artifact_stage_missing",
                ),
            })?;
        if !stage_root.starts_with(&derived_root) {
            return Err(HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(
                    "derived_artifact_stage_not_owned",
                ),
            });
        }
        let stage =
            Stage::open_existing(&stage_root).map_err(|error| HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(&error.to_string()),
            })?;
        let current = Bundle::at(root).open()?;
        let final_name = derived
            .artifact
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure("derived_artifact_name_missing"),
            })?
            .to_string();
        let bytes = stage
            .read_published(
                &final_name,
                derived.artifact.byte_count,
                &derived.artifact.sha256,
            )
            .map_err(|error| HostError::DerivedResult {
                diagnostic: artifact_error_diagnostic(&error),
            })?;
        if derived.source_snapshot.revision_hash != current.manifest.revision_hash
            || derived.artifact.source_revision_id != current.manifest.revision_hash
        {
            let diagnostic =
                Diagnostic::artifact_revision_mismatch("derived_artifact_source_revision_mismatch");
            self.retain_stale_last_valid_geometry(
                &derived.source_snapshot,
                &current,
                &derived.artifact.feature_id,
                &bytes,
                &diagnostic,
            );
            let _ = stage.discard();
            self.layer1_results
                .borrow_mut()
                .remove(&derived.artifact.cache_key);
            let _ = fs::remove_dir(&derived_root);
            self.current.replace(Some(current));
            return Err(HostError::DerivedResult { diagnostic });
        }
        let provenance = serde_json::json!({
            "request_id": derived.artifact.request_id,
            "operation": derived.artifact.operation,
            "feature_id": derived.artifact.feature_id,
            "source_revision_id": derived.artifact.source_revision_id,
            "worker_fingerprint": derived.artifact.worker_fingerprint,
            "byte_count": derived.artifact.byte_count,
            "sha256": derived.artifact.sha256,
        })
        .to_string();
        let feature_id = derived.artifact.feature_id.clone();
        if let Err(error) = stage.discard() {
            return Err(HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(&error.to_string()),
            });
        }
        self.layer1_results
            .borrow_mut()
            .remove(&derived.artifact.cache_key);
        let _ = fs::remove_dir(&derived_root);
        let bundle = Bundle::at(root);
        let updated = match append(
            &bundle,
            &current,
            &derived,
            &derived.artifact,
            &bytes,
            &provenance,
        ) {
            Ok(updated) => updated,
            Err(error) => {
                let is_stale = matches!(&error, BundleError::Invalid(detail) if detail.starts_with("worker result belongs to revision"));
                if is_stale && let Ok(reconciled) = bundle.open() {
                    let diagnostic = Diagnostic::artifact_revision_mismatch(
                        "derived_artifact_source_revision_mismatch",
                    );
                    self.retain_stale_last_valid_geometry(
                        &derived.source_snapshot,
                        &reconciled,
                        &feature_id,
                        &bytes,
                        &diagnostic,
                    );
                    self.current.replace(Some(reconciled));
                    self.layer1_results
                        .borrow_mut()
                        .remove(&derived.artifact.cache_key);
                    let _ = fs::remove_dir(&derived_root);
                    return Err(HostError::DerivedResult { diagnostic });
                }
                self.layer1_results
                    .borrow_mut()
                    .remove(&derived.artifact.cache_key);
                let _ = fs::remove_dir(&derived_root);
                if let Ok(reconciled) = bundle.open() {
                    self.current.replace(Some(reconciled));
                }
                return Err(error.into());
            }
        };
        let snapshot = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        let mut artifact = derived.artifact;
        artifact.path = root.join(BREP_SUBDIR).join(format!("{feature_id}.brep"));
        let mut value =
            serde_json::to_value(derived.result).map_err(|error| HostError::Validation {
                detail: format!("typed OCCT result serialization failed: {error}"),
            })?;
        value["brep_path"] =
            serde_json::Value::String(artifact.path.to_string_lossy().into_owned());
        value["brep_bytes"] = serde_json::Value::from(artifact.byte_count);
        value["brep_sha256"] = serde_json::Value::String(artifact.sha256.clone());
        let result = serde_json::from_value(value).map_err(|error| HostError::Validation {
            detail: format!("typed OCCT result promotion failed: {error}"),
        })?;
        Ok((snapshot, result, artifact))
    }

    /// Create the initial parameterized L-bracket through the OCCT worker.
    pub fn create_bracket(
        &self,
        root: impl AsRef<Path>,
        request: BracketRequest,
        worker: &OcctWorker,
    ) -> Result<BracketCommitView, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        if root.exists() && !root.is_dir() {
            return Err(HostError::BundlePathNotDirectory { path: root });
        }
        let mut request = request;
        let loaded = if root.exists() {
            Bundle::at(&root).open()?
        } else {
            Bundle::create(&root)?.open()?
        };
        request = request.with_output_path(&root, "bracket.brep");
        request
            .validate()
            .map_err(|detail| HostError::Validation { detail })?;
        let derived = self.stage_occt_result::<BracketResult>(
            &root,
            &request,
            threeterm_occt_worker::Operation::Bracket,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let kind = bracket_kind(&request);
        let history_event = loaded
            .history
            .initialize_l_bracket(
                &request.feature_id,
                request.length,
                request.width,
                request.height,
                request.thickness,
            )
            .map_err(|error| HostError::Validation {
                detail: error.to_string(),
            })?;
        let vertical_id = format!("{}-plate-vertical", request.feature_id);
        let horizontal_id = format!("{}-plate-horizontal", request.feature_id);
        let entries = [
            (request.feature_id.as_str(), kind.as_str()),
            (vertical_id.as_str(), "plate-vertical"),
            (horizontal_id.as_str(), "plate-horizontal"),
        ];
        let request_id = request.request_id.clone();
        let feature_id = request.feature_id.clone();
        let (snapshot, result, artifact) = self.promote_occt_result_with_append(
            &root,
            derived,
            move |bundle, current, _derived, _artifact, bytes, shared_provenance| {
                bundle.append_features_with_brep_if_revision_and_history_and_provenance(
                    &entries,
                    &feature_id,
                    current.revision_hash_hex(),
                    bytes,
                    &history_event,
                    &request_id,
                    shared_provenance,
                )
            },
        )?;
        Ok(BracketCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        if let Some(existing) = self.bracket_drafts.borrow().get(&draft_key).cloned() {
            let request = request.with_feature_id(&bracket_id);
            if request.length != existing.request.length
                || request.width != existing.request.width
                || request.height != existing.request.height
                || request.thickness != existing.request.thickness
            {
                let current_revision = Bundle::at(&root)
                    .open()
                    .map(|bundle| bundle.revision_hash_hex().to_string())
                    .unwrap_or_else(|_| existing.source_revision.clone());
                return Err(HostError::DraftInputConflict {
                    draft_id,
                    source_revision: existing.source_revision,
                    current_revision,
                    recovery: "use_update_or_refresh_draft",
                });
            }
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
        expected_fingerprint: &str,
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
        if bracket_draft_fingerprint(draft) != expected_fingerprint {
            return Err(HostError::DraftInputConflict {
                draft_id: draft_id.to_string(),
                source_revision: draft.source_revision.clone(),
                current_revision: draft.source_revision.clone(),
                recovery: "refresh_draft_and_retry",
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
    ) -> Result<BracketDraftCommitView, HostError> {
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
        let kind = bracket_kind(&draft.request);
        let draft_semantic_fingerprint = bracket_semantic_fingerprint(&draft);
        if let Some(committed) = self.reconcile_bracket_idempotency(
            &root,
            &draft_key,
            draft_id,
            &draft,
            &kind,
            &draft_semantic_fingerprint,
        )? {
            return Ok(committed);
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
        let idempotency_payload =
            bracket_idempotency_payload(&draft, &result.brep_sha256, &input_fingerprint);
        let snapshot = match self.promote_brep_bytes(
            &root,
            &draft.bracket_id,
            &kind,
            &draft.source_revision,
            &bytes,
            Some(&draft.source_brep_sha256),
            Some(draft_id),
            Some(&idempotency_payload),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if let Some(committed) = self.reconcile_bracket_promotion(
                    &root,
                    draft_id,
                    &draft,
                    &result.brep_sha256,
                    &error,
                ) {
                    return Ok(committed);
                }
                remove_preview_stage(&stage);
                return Err(self.classify_bracket_promotion_error(&root, draft_id, &draft, error));
            }
        };
        remove_preview_stage(&stage);
        self.bracket_drafts.borrow_mut().remove(&draft_key);
        Ok(BracketDraftCommitView {
            snapshot,
            input_fingerprint,
        })
    }

    fn reconcile_bracket_idempotency(
        &self,
        root: &Path,
        draft_key: &(PathBuf, String),
        draft_id: &str,
        draft: &BracketParameterDraft,
        kind: &str,
        semantic_fingerprint: &str,
    ) -> Result<Option<BracketDraftCommitView>, HostError> {
        let Some(committed) = Bundle::at(root).find_idempotency_key(draft_id)? else {
            return Ok(None);
        };
        let matching_entry = committed.log.entries().iter().find(|entry| {
            let payload = serde_json::from_str::<serde_json::Value>(
                entry.idempotency_payload.as_deref().unwrap_or_default(),
            )
            .ok();
            let payload_source_revision = payload
                .as_ref()
                .and_then(|payload| payload["source_revision"].as_str());
            let payload_result_sha256 = payload
                .as_ref()
                .and_then(|payload| payload["result_sha256"].as_str());
            let canonical_result_sha256 =
                sha256_path(&committed_brep_path(root, &draft.bracket_id)).ok();
            let source_matches = payload_source_revision == Some(draft.source_revision.as_str())
                || (draft.source_revision == committed.revision_hash_hex()
                    && entry.log_index + 1 == committed.log.entries().len());
            entry.idempotency_key.as_deref() == Some(draft_id)
                && entry.feature_id == draft.bracket_id
                && entry.kind == kind
                && entry.log_index + 1 == committed.log.entries().len()
                && source_matches
                && payload_result_sha256 == canonical_result_sha256.as_deref()
                && payload
                    .as_ref()
                    .and_then(|payload| payload["semantic_fingerprint"].as_str().map(str::to_owned))
                    .as_deref()
                    == Some(semantic_fingerprint)
        });
        let Some(entry) = matching_entry else {
            return Err(HostError::DraftIdempotencyConflict {
                draft_id: draft_id.to_string(),
                source_revision: draft.source_revision.clone(),
                current_revision: committed.revision_hash_hex().to_string(),
                recovery: "use_new_idempotency_key",
            });
        };
        let input_fingerprint = serde_json::from_str::<serde_json::Value>(
            entry.idempotency_payload.as_deref().unwrap_or_default(),
        )
        .ok()
        .and_then(|payload| payload["input_fingerprint"].as_str().map(str::to_owned))
        .unwrap_or_default();
        let snapshot = SnapshotView::from(&committed);
        self.current.replace(Some(committed));
        self.bracket_drafts.borrow_mut().remove(draft_key);
        Ok(Some(BracketDraftCommitView {
            snapshot,
            input_fingerprint,
        }))
    }

    fn reconcile_bracket_promotion(
        &self,
        root: &Path,
        draft_id: &str,
        draft: &BracketParameterDraft,
        result_sha256: &str,
        error: &HostError,
    ) -> Option<BracketDraftCommitView> {
        if matches!(
            error,
            HostError::Persistence(BundleError::PublicationUnknown(_))
        ) {
            return None;
        }
        let draft_key = draft_map_key(root, draft_id);
        let kind = bracket_kind(&draft.request);
        let input_fingerprint = bracket_input_fingerprint(draft, result_sha256);
        let idempotency_payload =
            bracket_idempotency_payload(draft, result_sha256, &input_fingerprint);
        let stage = preview_stage_path(root, &format!("{draft_id}-commit"));
        let committed = Bundle::at(root).open().ok()?;
        let published = committed.log.entries().iter().any(|entry| {
            entry.idempotency_key.as_deref() == Some(draft_id)
                && entry.feature_id == draft.bracket_id
                && entry.kind == kind
                && entry.idempotency_payload.as_deref() == Some(idempotency_payload.as_str())
        });
        if !published
            || sha256_path(&committed_brep_path(root, &draft.bracket_id)).ok()
                != Some(result_sha256.to_string())
        {
            return None;
        }
        let snapshot = SnapshotView::from(&committed);
        self.current.replace(Some(committed));
        self.bracket_drafts.borrow_mut().remove(&draft_key);
        remove_preview_stage(&stage);
        Some(BracketDraftCommitView {
            snapshot,
            input_fingerprint,
        })
    }

    fn classify_bracket_promotion_error(
        &self,
        root: &Path,
        draft_id: &str,
        draft: &BracketParameterDraft,
        error: HostError,
    ) -> HostError {
        if matches!(&error, HostError::Persistence(BundleError::Invalid(_)))
            && let Ok(current) = Bundle::at(root).open()
        {
            if let HostError::Persistence(BundleError::Invalid(detail)) = &error
                && detail.contains("idempotency key")
            {
                return HostError::DraftIdempotencyConflict {
                    draft_id: draft_id.to_string(),
                    source_revision: draft.source_revision.clone(),
                    current_revision: current.revision_hash_hex().to_string(),
                    recovery: "use_new_idempotency_key",
                };
            }
            if current.revision_hash_hex() != draft.source_revision {
                return HostError::DraftStale {
                    draft_id: draft_id.to_string(),
                    source_revision: draft.source_revision.clone(),
                    current_revision: current.revision_hash_hex().to_string(),
                    recovery: "discard_and_reopen",
                };
            }
            if sha256_path(&committed_brep_path(root, &draft.bracket_id)).ok()
                != Some(draft.source_brep_sha256.clone())
            {
                return HostError::DraftSourceChanged {
                    draft_id: draft_id.to_string(),
                    source_feature_id: draft.bracket_id.clone(),
                    source_revision: draft.source_revision.clone(),
                    current_revision: current.revision_hash_hex().to_string(),
                    recovery: "reload_source_and_reopen",
                };
            }
        }
        if matches!(
            &error,
            HostError::Persistence(BundleError::PublicationUnknown(_))
        ) {
            return HostError::DraftUnknownOutcome {
                draft_id: draft_id.to_string(),
                source_revision: draft.source_revision.clone(),
                current_revision: Bundle::at(root)
                    .open()
                    .map(|current| current.revision_hash_hex().to_string())
                    .unwrap_or_else(|_| draft.source_revision.clone()),
                recovery: "retry_same_idempotency_key",
            };
        }
        error
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

    pub fn bracket_draft_fingerprint(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
    ) -> Option<String> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        self.bracket_drafts
            .borrow()
            .get(&draft_map_key(&root, draft_id))
            .map(bracket_draft_fingerprint)
    }

    pub fn validate_bracket_parameter_draft_request(
        &self,
        root: impl AsRef<Path>,
        draft_id: &str,
        request: BracketRequest,
    ) -> Result<(), HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let draft = self
            .bracket_drafts
            .borrow()
            .get(&draft_map_key(&root, draft_id))
            .cloned()
            .ok_or_else(|| HostError::DraftNotFound {
                draft_id: draft_id.to_string(),
            })?;
        let request = request.with_feature_id(&draft.bracket_id);
        request
            .validate()
            .map_err(|detail| HostError::DraftInvalid {
                draft_id: draft_id.to_string(),
                detail,
            })?;
        if request.length != draft.request.length
            || request.width != draft.request.width
            || request.height != draft.request.height
            || request.thickness != draft.request.thickness
        {
            let current_revision = Bundle::at(&root)
                .open()
                .map(|bundle| bundle.revision_hash_hex().to_string())
                .unwrap_or_else(|_| draft.source_revision.clone());
            return Err(HostError::DraftInputConflict {
                draft_id: draft_id.to_string(),
                source_revision: draft.source_revision,
                current_revision,
                recovery: "use_update_or_refresh_draft",
            });
        }
        Ok(())
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
            .map(|(key, _)| key.clone())
            .collect();
        let bracket_ids: Vec<_> = self
            .bracket_drafts
            .borrow()
            .iter()
            .filter(|(_, draft)| now.duration_since(draft.created_at) > max_age)
            .map(|(id, _)| id.clone())
            .collect();
        let mut removed = 0;
        for key in generic_ids {
            if let Some(draft) = self.drafts.borrow_mut().remove(&key) {
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
            source_revision: draft.source_revision.clone(),
            current_revision: loaded.revision_hash_hex().to_string(),
            recovery: "reload_source_and_reopen",
        })?;
        if source_sha != draft.source_brep_sha256 {
            return Err(HostError::DraftSourceChanged {
                draft_id: draft.draft_id.clone(),
                source_feature_id: draft.bracket_id.clone(),
                source_revision: draft.source_revision.clone(),
                current_revision: loaded.revision_hash_hex().to_string(),
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
        idempotency_payload: Option<&str>,
    ) -> Result<SnapshotView, HostError> {
        let bundle = Bundle::at(root);
        let updated = match source_brep_sha256 {
            Some(source_brep_sha256) => bundle
                .replace_bracket_with_brep_if_revision_and_source_and_idempotency_payload(
                    feature_id,
                    kind,
                    expected_revision,
                    source_brep_sha256,
                    idempotency_key,
                    idempotency_payload,
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
                source_revision: draft.source_revision.clone(),
                current_revision: current_revision.to_string(),
                recovery: "reload_source_and_reopen",
            })?;
        if source_sha != draft.source_brep_sha256 {
            return Err(HostError::DraftSourceChanged {
                draft_id: draft.draft_id.clone(),
                source_feature_id: draft.source_feature_id.clone(),
                source_revision: draft.source_revision.clone(),
                current_revision: current_revision.to_string(),
                recovery: "reload_source_and_reopen",
            });
        }
        Ok(())
    }

    fn resolve_extrude_request(
        &self,
        root: &Path,
        mut request: ExtrudeRequest,
        expected_revision: Option<&str>,
    ) -> Result<ExtrudeRequest, HostError> {
        if request.mode == ExtrudeMode::Additive {
            if request.target_feature_id.is_some() || request.target_path.is_some() {
                return Err(HostError::Validation {
                    detail: "additive extrude must not have a target".to_string(),
                });
            }
            if let Some(expected) = expected_revision
                && Bundle::at(root).open()?.manifest.revision_hash != expected
            {
                return Err(HostError::Validation {
                    detail: "extrude source Revision Snapshot changed; retry the command"
                        .to_string(),
                });
            }
            return Ok(request);
        }
        let target_feature_id =
            request
                .target_feature_id
                .clone()
                .ok_or_else(|| HostError::Validation {
                    detail: "subtractive extrude requires target_feature_id".to_string(),
                })?;
        let loaded = Bundle::at(root).open()?;
        if expected_revision.is_some_and(|expected| loaded.manifest.revision_hash != expected) {
            return Err(HostError::Validation {
                detail: "extrude source Revision Snapshot changed; retry the command".to_string(),
            });
        }
        if !loaded.graph.contains_feature(&target_feature_id) {
            return Err(HostError::Validation {
                detail: format!(
                    "subtractive extrude target feature is missing: {target_feature_id}"
                ),
            });
        }
        let target_path = root
            .join(BREP_SUBDIR)
            .join(format!("{target_feature_id}.brep"));
        if !target_path.is_file() {
            self.load_with_extrude_replay(root)?;
        }
        if !target_path.is_file() {
            return Err(HostError::BrepFileMissing { path: target_path });
        }
        let loaded = Bundle::at(root).open()?;
        if expected_revision.is_some_and(|expected| loaded.manifest.revision_hash != expected) {
            return Err(HostError::Validation {
                detail: "extrude source Revision Snapshot changed; retry the command".to_string(),
            });
        }
        let target_entry = loaded
            .log
            .entries()
            .iter()
            .rev()
            .find(|entry| {
                entry.feature_id == target_feature_id
                    && entry.brep_byte_count.is_some()
                    && entry.brep_sha256.is_some()
            })
            .ok_or_else(|| HostError::Validation {
                detail: format!(
                    "subtractive extrude target has no authenticated geometry: {target_feature_id}"
                ),
            })?;
        let expected_bytes = target_entry
            .brep_byte_count
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| HostError::BrepIo {
                detail: "subtractive target BREP byte count is invalid".to_string(),
            })?;
        let expected_sha256 =
            target_entry
                .brep_sha256
                .as_deref()
                .ok_or_else(|| HostError::BrepIo {
                    detail: "subtractive target BREP digest is missing".to_string(),
                })?;
        read_brep_verified(&target_path, Some((expected_bytes, expected_sha256)))
            .map_err(|detail| HostError::BrepIo { detail })?;
        request.target_path = Some(target_path);
        if let Err(detail) = request.validate()
            && detail != "output_filename must be a non-empty plain filename"
        {
            return Err(HostError::Validation { detail });
        }
        Ok(request)
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
        worker.verify_identity().map_err(HostError::from)?;
        let request =
            self.resolve_extrude_request(root, request, Some(&source_snapshot.revision_hash))?;

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

    /// Remove a staged extrude after a read-only preview.
    pub fn discard_extrude_derived(&self, derived: ExtrudeDerivedResult) {
        self.layer1_results
            .borrow_mut()
            .remove(&derived.artifact.cache_key);
        if let Some(stage_root) = derived.artifact.path.parent() {
            let _ = fs::remove_dir_all(stage_root);
            if let Some(derived_root) = stage_root.parent()
                && derived_root
                    .file_name()
                    .is_some_and(|name| name == ".derived")
            {
                let _ = fs::remove_dir(derived_root);
            }
        }
    }

    fn discard_staged_occt_result<R>(&self, derived: &StagedOcctResult<R>) {
        self.layer1_results
            .borrow_mut()
            .remove(&derived.artifact.cache_key);
        if let Some(stage_root) = derived.artifact.path.parent() {
            let _ = fs::remove_dir_all(stage_root);
        }
    }

    /// Promote one Host-validated extrude result into the next canonical
    /// Project Generation. The request stage is consumed before persistence
    /// copies the bundle, so transient worker artifacts cannot become part of
    /// the new generation.
    pub fn promote_extrude_derived(
        &self,
        root: impl AsRef<Path>,
        derived: ExtrudeDerivedResult,
    ) -> Result<ExtrudeCommitView, HostError> {
        let root = Bundle::at(root.as_ref()).canonical_root().to_path_buf();
        let derived_root = root.join(".derived");
        let stage_root = derived
            .artifact
            .path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(
                    "derived_artifact_stage_missing",
                ),
            })?;
        if !stage_root.starts_with(&derived_root) {
            return Err(HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(
                    "derived_artifact_stage_not_owned",
                ),
            });
        }
        let stage =
            Stage::open_existing(&stage_root).map_err(|error| HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(&error.to_string()),
            })?;
        let source_snapshot = derived.source_snapshot.clone();
        let worker_fingerprint = derived.artifact.worker_fingerprint.clone();
        let mut artifact = derived.artifact.clone();
        let feature_id = derived.artifact.feature_id.clone();
        let cache_key = derived.artifact.cache_key.clone();
        let final_name = derived
            .artifact
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure("derived_artifact_name_missing"),
            })?
            .to_string();
        if stage_root.join(&final_name) != derived.artifact.path {
            let _ = stage.discard();
            self.layer1_results.borrow_mut().remove(&cache_key);
            let _ = fs::remove_dir(&derived_root);
            return Err(HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(
                    "derived_artifact_path_mismatch",
                ),
            });
        }

        let current = match Bundle::at(&root).open() {
            Ok(current) => current,
            Err(error) => {
                let _ = stage.discard();
                self.layer1_results.borrow_mut().remove(&cache_key);
                let _ = fs::remove_dir(&derived_root);
                return Err(error.into());
            }
        };
        if derived.result.source_revision_id.as_deref()
            != Some(source_snapshot.revision_hash.as_str())
            || derived.result.request_id.as_str() != derived.artifact.request_id
            || derived.result.feature_id.as_str() != feature_id
        {
            let _ = stage.discard();
            self.layer1_results.borrow_mut().remove(&cache_key);
            let _ = fs::remove_dir(&derived_root);
            return Err(HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_revision_mismatch(
                    "derived_result_identity_mismatch",
                ),
            });
        }

        let bytes = match stage.read_published(
            &final_name,
            derived.artifact.byte_count,
            &derived.artifact.sha256,
        ) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = stage.discard();
                self.layer1_results.borrow_mut().remove(&cache_key);
                let _ = fs::remove_dir(&derived_root);
                return Err(HostError::DerivedResult {
                    diagnostic: artifact_error_diagnostic(&error),
                });
            }
        };
        if source_snapshot.revision_hash != current.manifest.revision_hash
            || derived.artifact.source_revision_id != current.manifest.revision_hash
        {
            let diagnostic =
                Diagnostic::artifact_revision_mismatch("derived_artifact_source_revision_mismatch");
            self.retain_stale_last_valid_geometry(
                &source_snapshot,
                &current,
                &feature_id,
                &bytes,
                &diagnostic,
            );
            let _ = stage.discard();
            self.layer1_results.borrow_mut().remove(&cache_key);
            let _ = fs::remove_dir(&derived_root);
            self.current.replace(Some(current));
            return Err(HostError::DerivedResult { diagnostic });
        }
        if let Err(error) = stage.discard() {
            return Err(HostError::DerivedResult {
                diagnostic: Diagnostic::artifact_promotion_failure(&error.to_string()),
            });
        }

        let bundle = Bundle::at(&root);
        let kind = format!("brep:{feature_id}");
        let updated = match bundle.append_feature_with_brep_if_revision(
            &feature_id,
            &kind,
            &current.manifest.revision_hash,
            &bytes,
        ) {
            Ok(updated) => updated,
            Err(error) => {
                if matches!(&error, BundleError::Invalid(detail) if detail.starts_with("worker result belongs to revision"))
                    && let Ok(reconciled) = bundle.open()
                {
                    let current_view = SnapshotView::from(&reconciled);
                    if current_view.revision_hash != source_snapshot.revision_hash {
                        let diagnostic = Diagnostic::artifact_revision_mismatch(
                            "derived_artifact_source_revision_mismatch",
                        );
                        self.retain_stale_last_valid_geometry(
                            &source_snapshot,
                            &reconciled,
                            &feature_id,
                            &bytes,
                            &diagnostic,
                        );
                        self.layer1_results.borrow_mut().remove(&cache_key);
                        let _ = fs::remove_dir(&derived_root);
                        self.current.replace(Some(reconciled));
                        return Err(HostError::DerivedResult { diagnostic });
                    }
                }
                self.layer1_results.borrow_mut().remove(&cache_key);
                let _ = fs::remove_dir(&derived_root);
                if let Ok(reconciled) = bundle.open() {
                    self.current.replace(Some(reconciled));
                }
                return Err(error.into());
            }
        };
        self.layer1_results.borrow_mut().remove(&cache_key);
        let _ = fs::remove_dir_all(&derived_root);
        let snapshot = SnapshotView::from(&updated);
        self.current.replace(Some(updated));
        let mut result = derived.result;
        result.brep_path = root.join(BREP_SUBDIR).join(format!("{feature_id}.brep"));
        result.brep_bytes = bytes.len();
        result.brep_sha256 = sha256_hex(&bytes);
        artifact.path = result.brep_path.clone();
        Ok(ExtrudeCommitView {
            source_snapshot,
            snapshot,
            result,
            worker_fingerprint,
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
        let SupervisorOutcome::Completed {
            request_id,
            result,
            mut artifact_headers,
        } = outcome
        else {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("worker_result_not_completed"),
            ));
        };
        if binding.request_id.is_empty() {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_request_mismatch("empty_artifact_request_id"),
            ));
        }
        if binding.operation != "extrude" {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("artifact_request_operation_mismatch"),
            ));
        }
        if binding.feature_id.is_empty() {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("empty_artifact_feature_id"),
            ));
        }
        if !is_sha256_hex(&binding.source_revision_id) {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_revision_mismatch("invalid_artifact_source_revision"),
            ));
        }
        if binding.artifact_kind != "brep" {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("artifact_request_kind_mismatch"),
            ));
        }
        if binding.staging_name.is_empty()
            || binding.staging_name.contains('/')
            || binding.staging_name.contains('\\')
            || binding.staging_name.contains('\0')
        {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("artifact_request_staging_name_invalid"),
            ));
        }
        if !is_sha256_hex(&binding.semantic_input_sha256)
            || !is_sha256_hex(&binding.deterministic_settings_sha256)
        {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_cache_key_mismatch("invalid_artifact_cache_identity"),
            ));
        }
        if request_id != binding.request_id {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_request_mismatch("completed_request_id_mismatch"),
            ));
        }
        let outcome_result = match serde_json::from_value::<ExtrudeResult>(result) {
            Ok(result) => result,
            Err(error) => {
                return Err(discard_stage(
                    stage,
                    Diagnostic::artifact_promotion_failure(&format!(
                        "typed_result_schema_mismatch:{error}"
                    )),
                ));
            }
        };
        if outcome_result != *typed_result {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_does_not_match_completion"),
            ));
        }
        if typed_result.schema_version != threeterm_occt_worker::SCHEMA_VERSION {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_schema_mismatch"),
            ));
        }
        if !typed_result.is_success() {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_not_ok"),
            ));
        }
        if typed_result.request_id != binding.request_id {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_request_mismatch("typed_result_request_id_mismatch"),
            ));
        }
        if typed_result.source_revision_id.as_deref() != Some(binding.source_revision_id.as_str()) {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_revision_mismatch("typed_result_source_revision_mismatch"),
            ));
        }
        if typed_result.operation != threeterm_occt_worker::Operation::Extrude
            || binding.operation != "extrude"
        {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_operation_mismatch"),
            ));
        }
        if typed_result.feature_id != binding.feature_id {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_feature_id_mismatch"),
            ));
        }
        let expected_path = stage_root.join(format!("{}.partial", binding.staging_name));
        if typed_result.brep_path != expected_path {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_path_mismatch"),
            ));
        }
        if artifact_headers.len() != 1 {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("expected_exactly_one_artifact"),
            ));
        }
        let artifact = artifact_headers
            .pop()
            .expect("checked exactly one artifact");
        if artifact.schema_version != threeterm_protocol::schema_version() {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("artifact_schema_mismatch"),
            ));
        }
        if typed_result.brep_bytes as u64 != artifact.header.byte_count
            || typed_result.brep_sha256 != artifact.header.sha256
        {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("typed_result_artifact_metadata_mismatch"),
            ));
        }
        let expected_worker = expected_occt_worker_fingerprint();
        if artifact.header.worker_fingerprint != expected_worker {
            return Err(discard_stage(
                stage,
                Diagnostic::artifact_promotion_failure("artifact_worker_fingerprint_mismatch"),
            ));
        }

        self.accept_staged_artifact(stage, binding, &expected_worker, artifact.header, true)
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
        let stage = match Stage::open(root) {
            Ok(stage) => stage,
            Err(error) => {
                cleanup_staged_artifact(root, &request.staging_name);
                return Err(Diagnostic::artifact_promotion_failure(&error.to_string()));
            }
        };
        self.accept_staged_artifact(stage, request, expected_worker, artifact.header, false)
    }

    fn accept_staged_artifact(
        &self,
        stage: Stage,
        request: &Layer1ArtifactRequest,
        expected_worker: &WorkerFingerprint,
        header: threeterm_protocol::artifact::ArtifactHeader,
        discard_stage_on_error: bool,
    ) -> Result<Layer1DerivedResult, Diagnostic> {
        let header_staging_name = header.staging_name.clone();
        let current = match self.current() {
            Some(current) => current,
            None => {
                return Err(reject_staged_artifact(
                    stage,
                    &request.staging_name,
                    &header_staging_name,
                    Diagnostic::artifact_promotion_failure("canonical_snapshot_missing"),
                    discard_stage_on_error,
                ));
            }
        };
        // Exclusion policy: never cache Command Drafts, hover/pointer/candidate,
        // stale last-valid geometry, preview-only beyond session, worker internals.
        if is_layer1_excluded(request) {
            return Err(reject_staged_artifact(
                stage,
                &request.staging_name,
                &header_staging_name,
                Diagnostic::artifact_promotion_failure(
                    "excluded_layer1_artifact: draft/hover/candidate/pointer/stale/preview-only/worker-internal/tmp/stderr",
                ),
                discard_stage_on_error,
            ));
        }
        let expected_cache_key = Layer1CacheKey::issue(request, expected_worker);
        if request.source_revision_id != current.revision_hash
            || header.source_revision_id != request.source_revision_id
            || header.cache_key.source_revision_id != request.source_revision_id
        {
            return Err(reject_staged_artifact(
                stage,
                &request.staging_name,
                &header_staging_name,
                Diagnostic::artifact_revision_mismatch("artifact_source_revision_mismatch"),
                discard_stage_on_error,
            ));
        }
        if header.request_id != request.request_id {
            return Err(reject_staged_artifact(
                stage,
                &request.staging_name,
                &header_staging_name,
                Diagnostic::artifact_request_mismatch("artifact_request_id_mismatch"),
                discard_stage_on_error,
            ));
        }
        if header.operation != request.operation || header.feature_id != request.feature_id {
            return Err(reject_staged_artifact(
                stage,
                &request.staging_name,
                &header_staging_name,
                Diagnostic::artifact_promotion_failure("artifact_operation_or_feature_id_mismatch"),
                discard_stage_on_error,
            ));
        }
        if header.cache_key != expected_cache_key {
            return Err(reject_staged_artifact(
                stage,
                &request.staging_name,
                &header_staging_name,
                Diagnostic::artifact_cache_key_mismatch("artifact_cache_key_mismatch"),
                discard_stage_on_error,
            ));
        }
        if header.artifact_kind != request.artifact_kind
            || header.staging_name != request.staging_name
            || header.worker_fingerprint != *expected_worker
        {
            return Err(reject_staged_artifact(
                stage,
                &request.staging_name,
                &header_staging_name,
                Diagnostic::artifact_promotion_failure("artifact_header_mismatch"),
                discard_stage_on_error,
            ));
        }

        if let Err(error) = stage.verify(&header) {
            return Err(reject_staged_artifact(
                stage,
                &request.staging_name,
                &header_staging_name,
                artifact_error_diagnostic(&error),
                discard_stage_on_error,
            ));
        }
        let final_name = header.cache_key.final_artifact_name();
        if let Some(existing) = self.layer1_result(&header.cache_key) {
            let Some(existing_root) = existing.path.parent() else {
                return Err(reject_staged_artifact(
                    stage,
                    &request.staging_name,
                    &header_staging_name,
                    Diagnostic::artifact_promotion_failure("cached_artifact_root_missing"),
                    discard_stage_on_error,
                ));
            };
            let cached_stage = match Stage::open_existing(existing_root) {
                Ok(stage) => stage,
                Err(error) => {
                    return Err(reject_staged_artifact(
                        stage,
                        &request.staging_name,
                        &header_staging_name,
                        artifact_error_diagnostic(&error),
                        discard_stage_on_error,
                    ));
                }
            };
            let published_matches = match cached_stage.published_matches(
                &final_name,
                existing.byte_count,
                &existing.sha256,
            ) {
                Ok(matches) => matches,
                Err(error) => {
                    return Err(reject_staged_artifact(
                        stage,
                        &request.staging_name,
                        &header_staging_name,
                        artifact_error_diagnostic(&error),
                        discard_stage_on_error,
                    ));
                }
            };
            if published_matches {
                if discard_stage_on_error {
                    if let Err(error) = stage.discard() {
                        return Err(Diagnostic::artifact_promotion_failure(&error.to_string()));
                    }
                } else {
                    stage.discard_staged(&header.staging_name);
                }
                return Ok(existing);
            }
            if let Err(error) = cached_stage.discard_final(&final_name) {
                return Err(reject_staged_artifact(
                    stage,
                    &request.staging_name,
                    &header_staging_name,
                    artifact_error_diagnostic(&error),
                    discard_stage_on_error,
                ));
            }
        }
        let path = match stage.publish_verified(&header.staging_name, &final_name) {
            Ok(path) => path,
            Err(error) => {
                return Err(reject_staged_artifact(
                    stage,
                    &request.staging_name,
                    &header_staging_name,
                    artifact_error_diagnostic(&error),
                    discard_stage_on_error,
                ));
            }
        };
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

    fn retain_stale_last_valid_geometry(
        &self,
        source_snapshot: &SnapshotView,
        current: &LoadedBundle,
        feature_id: &str,
        bytes: &[u8],
        diagnostic: &Diagnostic,
    ) {
        self.stale_last_valid_geometry.borrow_mut().insert(
            feature_id.to_string(),
            RetainedStaleLastValidGeometry {
                source_revision_id: source_snapshot.revision_hash.clone(),
                current_revision_id: current.manifest.revision_hash.clone(),
                feature_id: feature_id.to_string(),
                bytes: bytes.to_vec(),
                sha256: sha256_hex(bytes),
                authoritative: false,
                diagnostic_code: diagnostic.code,
            },
        );
    }

    /// Return retained stale geometry without making it part of canonical
    /// state. Callers must explicitly choose a recovery action to use it.
    pub fn retained_stale_last_valid_geometry(
        &self,
        feature_id: &str,
    ) -> Option<RetainedStaleLastValidGeometry> {
        self.stale_last_valid_geometry
            .borrow()
            .get(feature_id)
            .cloned()
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
        if loaded
            .graph
            .features()
            .any(|feature| feature.id.as_str() == feature_id)
        {
            cleanup_worker_stage(root, brep_path);
            self.current.replace(Some(loaded));
            return Err(HostError::Validation {
                detail: format!("feature ID {feature_id:?} already exists"),
            });
        }

        let brep_bytes = match read_brep_verified(brep_path, expected) {
            Ok(bytes) => bytes,
            Err(detail) => {
                self.current.replace(Some(loaded));
                return Err(HostError::BrepIo { detail });
            }
        };

        let expected_revision = expected_revision.unwrap_or(prior_view.revision_hash.as_str());
        let kind = format!("brep:{feature_id}");
        let updated_result = bundle.append_new_feature_with_brep_if_revision(
            feature_id,
            &kind,
            expected_revision,
            &brep_bytes,
        );
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
                } else {
                    self.current.replace(Some(loaded));
                }
                cleanup_worker_stage(root, brep_path);
                return Err(HostError::from(error));
            }
        };
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
        let source_snapshot = self.load(root)?;
        let request =
            self.resolve_extrude_request(root, request, Some(&source_snapshot.revision_hash))?;
        let derived = self.stage_occt_result::<ExtrudeResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Extrude,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(ExtrudeCommitView {
            source_snapshot,
            snapshot,
            result,
            worker_fingerprint: expected_occt_worker_fingerprint(),
            artifact,
        })
    }

    /// Commit an extrude only if the worker was staged from the caller's
    /// expected source revision. The final promotion still performs its own
    /// persistence-lock comparison.
    pub fn extrude_at_revision(
        &self,
        root: impl AsRef<Path>,
        request: ExtrudeRequest,
        worker: &OcctWorker,
        expected_revision: &str,
    ) -> Result<ExtrudeCommitView, HostError> {
        let root = root.as_ref();
        let derived = self.stage_occt_result::<ExtrudeResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Extrude,
            worker,
        )?;
        if derived.source_snapshot.revision_hash != expected_revision {
            let current_revision = derived.source_snapshot.revision_hash.clone();
            self.discard_staged_occt_result(&derived);
            return Err(HostError::Validation {
                detail: format!(
                    "extrude source revision {expected_revision:?} changed to {current_revision:?}"
                ),
            });
        }
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(ExtrudeCommitView {
            source_snapshot,
            snapshot,
            result,
            worker_fingerprint: expected_occt_worker_fingerprint(),
            artifact,
        })
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
        let source_snapshot = self.load(root)?;
        let request =
            self.resolve_extrude_request(root, request, Some(&source_snapshot.revision_hash))?;
        let mut on_progress = |_progress: &threeterm_protocol::supervisor::Progress| {};
        let derived = self.stage_occt_result_inner::<ExtrudeResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Extrude,
            worker,
            Some(cancel),
            Some(&mut on_progress),
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(ExtrudeCommitView {
            source_snapshot,
            snapshot,
            result,
            worker_fingerprint: expected_occt_worker_fingerprint(),
            artifact,
        })
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
        let derived = self.stage_occt_result::<BooleanFuseResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::BooleanFuse,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(BooleanFuseCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
    }

    pub fn boolean_cut(
        &self,
        root: impl AsRef<Path>,
        request: BooleanCutRequest,
        worker: &OcctWorker,
    ) -> Result<BooleanCutCommitView, HostError> {
        let root = root.as_ref();
        let derived = self.stage_occt_result::<BooleanCutResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::BooleanCut,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(BooleanCutCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
    }

    pub fn boolean_common(
        &self,
        root: impl AsRef<Path>,
        request: BooleanCommonRequest,
        worker: &OcctWorker,
    ) -> Result<BooleanCommonCommitView, HostError> {
        let root = root.as_ref();
        let derived = self.stage_occt_result::<BooleanCommonResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::BooleanCommon,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(BooleanCommonCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        let derived = self.stage_occt_result::<FilletResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Fillet,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(FilletCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        let derived = self.stage_occt_result::<ChamferResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Chamfer,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(ChamferCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
    }

    /// Hole `request` against the disposable OCCT worker and, on
    /// success, commit the holed BREP into a new revision.
    ///
    /// The request must carry its semantic support (`base_feature_id`);
    /// unknown supports and invalid hole geometry are rejected before the
    /// worker runs so the prior Revision Snapshot is preserved.
    pub fn hole(
        &self,
        root: impl AsRef<Path>,
        request: HoleRequest,
        worker: &OcctWorker,
    ) -> Result<HoleCommitView, HostError> {
        request.validate().map_err(|detail| HostError::Validation {
            detail: format!("hole request is invalid: {detail}"),
        })?;
        let root = root.as_ref();
        let base_feature_id =
            request
                .base_feature_id
                .clone()
                .ok_or_else(|| HostError::Validation {
                    detail: "hole request is missing its semantic support".to_string(),
                })?;
        if base_feature_id.is_empty() {
            return Err(HostError::Validation {
                detail: "hole request is missing its semantic support".to_string(),
            });
        }
        let loaded = Bundle::at(root).open()?;
        if !loaded.graph.contains_feature(&base_feature_id) {
            return Err(HostError::Validation {
                detail: format!("hole base feature is missing: {base_feature_id}"),
            });
        }
        let derived = self.stage_occt_result::<HoleResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Hole,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(HoleCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        let derived = self.stage_occt_result::<RevolveResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Revolve,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(RevolveCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        let derived = self.stage_occt_result::<MirrorResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Mirror,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(MirrorCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        let derived = self.stage_occt_result::<LinearPatternResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::LinearPattern,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(LinearPatternCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        let derived = self.stage_occt_result::<CircularPatternResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::CircularPattern,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(CircularPatternCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
    }

    /// Run the real sequential Boolean-cut pattern with cooperative
    /// cancellation. A successful result is the only path that can advance
    /// the canonical Revision Snapshot.
    pub fn boolean_pattern(
        &self,
        root: impl AsRef<Path>,
        request: BooleanPatternRequest,
        worker: &OcctWorker,
    ) -> Result<BooleanPatternCommitView, HostError> {
        let cancel = AtomicBool::new(false);
        let mut ignore_progress = |_progress: &threeterm_protocol::supervisor::Progress| {};
        self.boolean_pattern_with_cancel_and_progress(
            root,
            request,
            worker,
            &cancel,
            &mut ignore_progress,
        )
    }

    pub fn boolean_pattern_with_cancel(
        &self,
        root: impl AsRef<Path>,
        request: BooleanPatternRequest,
        worker: &OcctWorker,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<BooleanPatternCommitView, HostError> {
        let mut ignore_progress = |_progress: &threeterm_protocol::supervisor::Progress| {};
        self.boolean_pattern_with_cancel_and_progress(
            root,
            request,
            worker,
            cancel,
            &mut ignore_progress,
        )
    }

    pub fn boolean_pattern_with_cancel_and_progress(
        &self,
        root: impl AsRef<Path>,
        request: BooleanPatternRequest,
        worker: &OcctWorker,
        cancel: &std::sync::atomic::AtomicBool,
        on_progress: &mut dyn FnMut(&threeterm_protocol::supervisor::Progress),
    ) -> Result<BooleanPatternCommitView, HostError> {
        let root = root.as_ref();
        let derived = self.stage_occt_result_with_cancel_and_progress::<BooleanPatternResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::BooleanPattern,
            worker,
            cancel,
            on_progress,
        )?;
        if cancel.load(Ordering::SeqCst) {
            let stage_root = derived.artifact.path.parent().map(Path::to_path_buf);
            self.layer1_results
                .borrow_mut()
                .remove(&derived.artifact.cache_key);
            if let Some(stage_root) = stage_root {
                let _ = fs::remove_dir_all(stage_root);
            }
            return Err(HostError::WorkerTerminated {
                record: Box::new(threeterm_protocol::supervisor::TerminationRecord {
                    request_id: derived.artifact.request_id,
                    stage: "cancelled_before_promotion".to_string(),
                    cancel_reason: Some("cancelled by host".to_string()),
                    elapsed: Duration::ZERO,
                    last_progress: None,
                    last_artifact_error: None,
                    exit_signal: None,
                    exit_code: Some(0),
                    stderr_tail: String::new(),
                    failed_code: None,
                    failed_detail: None,
                    protocol_diagnostic: None,
                    termination_error: None,
                    exit_kind: threeterm_protocol::supervisor::ExitKind::Cooperative,
                }),
            });
        }
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        let identity = ProjectIdentity::from(&Bundle::at(root).open()?);
        Ok(BooleanPatternCommitView {
            source_snapshot,
            snapshot,
            artifact,
            result,
            identity,
        })
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
        let derived = self.stage_occt_result::<ShellResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Shell,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(ShellCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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
        let derived = self.stage_occt_result::<DraftResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Draft,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(DraftCommitView {
            source_snapshot: Some(source_snapshot),
            snapshot,
            result,
            artifact: Some(artifact),
        })
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
        let derived = self.stage_occt_result::<LoftResult>(
            root,
            &request,
            threeterm_occt_worker::Operation::Loft,
            worker,
        )?;
        let source_snapshot = derived.source_snapshot.clone();
        let (snapshot, result, artifact) = self.promote_occt_result(root, derived)?;
        Ok(LoftCommitView {
            source_snapshot,
            snapshot,
            result,
            artifact,
        })
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

fn canonical_selected_feature_ids(feature_ids: &[String]) -> Result<Vec<String>, HostError> {
    let mut selected = BTreeSet::new();
    for feature_id in feature_ids {
        if feature_id.is_empty() || !selected.insert(feature_id.clone()) {
            return Err(HostError::Validation {
                detail: "selected feature IDs must be non-empty and unique".to_string(),
            });
        }
    }
    if selected.is_empty() {
        return Err(HostError::Validation {
            detail: "selected feature IDs must not be empty".to_string(),
        });
    }
    Ok(selected.into_iter().collect())
}

fn descriptor_for_selected_l_bracket(
    definition_id: &str,
    selected: &[String],
    snapshot: &threeterm_domain::history::HistorySnapshot,
) -> Result<threeterm_domain::LBracketDescriptor, HostError> {
    let mut family: Option<&str> = None;
    let mut values = [None; 4];
    for feature_id in selected {
        let Some((candidate_family, role)) = l_bracket_feature_role(feature_id) else {
            return Err(HostError::Validation {
                detail: format!("selected feature is not an L-bracket feature: {feature_id}"),
            });
        };
        if family.is_some_and(|existing| existing != candidate_family) {
            return Err(HostError::Validation {
                detail: "selected feature subset mixes L-bracket families".to_string(),
            });
        }
        family = Some(candidate_family);
        let value = snapshot
            .features
            .get(feature_id)
            .map(|feature| feature.input_value)
            .ok_or_else(|| HostError::Validation {
                detail: format!("selected feature reference is lost: {feature_id}"),
            })?;
        match role {
            "base" => values[0] = Some(value),
            "bend" => values[1] = Some(value),
            "finish" => values[2] = Some(value),
            "independent-base" => values[3] = Some(value),
            "independent-finish" => {}
            _ => unreachable!(),
        }
    }
    let [Some(length), Some(width), Some(height), Some(thickness)] = values else {
        return Err(HostError::Validation {
            detail: "selected feature subset does not contain all L-bracket parameters".to_string(),
        });
    };
    Ok(threeterm_domain::LBracketDescriptor {
        feature_id: format!("{definition_id}-feature"),
        length,
        width,
        height,
        thickness,
    })
}

fn l_bracket_feature_role(feature_id: &str) -> Option<(&str, &str)> {
    for (suffix, role) in [
        ("-independent-base", "independent-base"),
        ("-independent-finish", "independent-finish"),
        ("-base", "base"),
        ("-bend", "bend"),
        ("-finish", "finish"),
    ] {
        if let Some(family) = feature_id
            .strip_suffix(suffix)
            .filter(|family| !family.is_empty())
        {
            return Some((family, role));
        }
    }
    None
}

fn is_geometric_feature_kind(kind: &str) -> bool {
    kind.starts_with("brep:") || kind.starts_with("bracket:")
}

/// L-bracket family parameters read from one history snapshot. The role
/// mapping mirrors the canonical descriptor derivation: base carries length,
/// bend carries width, finish carries height, and independent-base carries
/// thickness.
#[derive(Debug, Clone, PartialEq)]
struct BracketFamilyParams {
    length: f64,
    width: f64,
    height: f64,
    thickness: f64,
}

fn bracket_family_params(snapshot: &HistorySnapshot, family: &str) -> Option<BracketFamilyParams> {
    let value = |suffix: &str| {
        snapshot
            .features
            .get(&format!("{family}{suffix}"))
            .map(|feature| feature.input_value)
    };
    Some(BracketFamilyParams {
        length: value("-base")?,
        width: value("-bend")?,
        height: value("-finish")?,
        thickness: value("-independent-base")?,
    })
}

fn bracket_family_ids(snapshot: &HistorySnapshot) -> BTreeSet<String> {
    snapshot
        .features
        .keys()
        .filter_map(|id| l_bracket_feature_role(id).map(|(family, _)| family.to_string()))
        .collect()
}

fn bracket_family_is_current(snapshot: &HistorySnapshot, family: &str) -> bool {
    ["-base", "-bend", "-finish", "-independent-base"]
        .into_iter()
        .all(|suffix| {
            snapshot
                .features
                .get(&format!("{family}{suffix}"))
                .is_none_or(|feature| feature.status == HistoryStatus::CurrentValid)
        })
}

struct StagedBracketFamily {
    family: String,
    request: BracketRequest,
    bytes: Vec<u8>,
    result_sha256: String,
    source_brep_sha256: String,
    source_revision: String,
}

fn history_recompute_idempotency_payload(
    family: &str,
    request: &BracketRequest,
    source_brep_sha256: &str,
    source_revision: &str,
    result_sha256: &str,
) -> String {
    let semantic = serde_json::json!({
        "bracket_id": family,
        "height": format!("{:.17}", request.height),
        "length": format!("{:.17}", request.length),
        "thickness": format!("{:.17}", request.thickness),
        "width": format!("{:.17}", request.width),
    });
    let semantic_bytes =
        serde_json::to_vec(&semantic).expect("history recompute semantic serializes");
    let semantic_fingerprint = format!("{:x}", Sha256::digest(semantic_bytes));
    let input = serde_json::json!({
        "bracket_id": family,
        "height": format!("{:.17}", request.height),
        "length": format!("{:.17}", request.length),
        "result_sha256": result_sha256,
        "source_brep_sha256": source_brep_sha256,
        "source_revision": source_revision,
        "thickness": format!("{:.17}", request.thickness),
        "width": format!("{:.17}", request.width),
    });
    let input_bytes = serde_json::to_vec(&input).expect("history recompute input serializes");
    let input_fingerprint = format!("{:x}", Sha256::digest(input_bytes));
    serde_json::json!({
        "input_fingerprint": input_fingerprint,
        "result_sha256": result_sha256,
        "semantic_fingerprint": semantic_fingerprint,
        "source_revision": source_revision,
    })
    .to_string()
}

fn parse_ascii_stl(path: &Path, feature_id: &str) -> Result<Vec<SceneTriangle>, HostError> {
    let metadata = fs::metadata(path).map_err(|error| HostError::BrepIo {
        detail: format!("read viewport tessellation metadata failed: {error}"),
    })?;
    if metadata.len() > MAX_VIEWPORT_TESSELLATION_BYTES {
        return Err(HostError::BrepInvalid {
            request_id: Some(format!("viewport-tessellation-{feature_id}")),
            detail: "viewport tessellation exceeds the bounded ASCII STL size".to_string(),
        });
    }
    let bytes = fs::read(path).map_err(|error| HostError::BrepIo {
        detail: format!("read viewport tessellation failed: {error}"),
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| HostError::BrepInvalid {
        request_id: Some(format!("viewport-tessellation-{feature_id}")),
        detail: format!("viewport tessellation is not ASCII STL: {error}"),
    })?;
    let mut vertices = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(values) = trimmed.strip_prefix("vertex") else {
            continue;
        };
        let values: Vec<_> = values.split_whitespace().collect();
        if values.len() != 3 {
            return Err(HostError::BrepInvalid {
                request_id: Some(format!("viewport-tessellation-{feature_id}")),
                detail: "viewport tessellation vertex must contain three coordinates".to_string(),
            });
        }
        let mut vertex = [0.0_f64; 3];
        for (index, value) in values.iter().enumerate() {
            vertex[index] = value.parse().map_err(|error| HostError::BrepInvalid {
                request_id: Some(format!("viewport-tessellation-{feature_id}")),
                detail: format!("viewport tessellation coordinate is invalid: {error}"),
            })?;
        }
        if vertex.iter().any(|coordinate| !coordinate.is_finite()) {
            return Err(HostError::BrepInvalid {
                request_id: Some(format!("viewport-tessellation-{feature_id}")),
                detail: "viewport tessellation contains a non-finite coordinate".to_string(),
            });
        }
        vertices.push(vertex);
    }
    if vertices.is_empty() || !vertices.len().is_multiple_of(3) {
        return Err(HostError::BrepInvalid {
            request_id: Some(format!("viewport-tessellation-{feature_id}")),
            detail: "viewport tessellation must contain complete non-empty triangles".to_string(),
        });
    }
    Ok(vertices
        .chunks_exact(3)
        .map(|vertices| SceneTriangle {
            vertices: [vertices[0], vertices[1], vertices[2]],
        })
        .collect())
}

pub fn stale_last_valid_geometry_for_export(
    history: &HistoryState,
    export_feature_id: &str,
) -> Vec<StaleLastValidGeometryEntry> {
    let snapshot = history.active_snapshot();
    let mut candidates = BTreeSet::new();
    for suffix in ["-base", "-bend", "-finish"] {
        let candidate = format!("{export_feature_id}{suffix}");
        if snapshot.features.contains_key(&candidate) {
            candidates.insert(candidate);
        }
    }
    if candidates.is_empty() && snapshot.features.contains_key(export_feature_id) {
        candidates.insert(export_feature_id.to_string());
    }
    candidates
        .into_iter()
        .filter_map(|feature_id| {
            let feature = snapshot.features.get(&feature_id)?;
            let status = match feature.status {
                HistoryStatus::Broken => "broken",
                HistoryStatus::BlockedByFailure => "blocked-by-failure",
                _ => return None,
            };
            Some(StaleLastValidGeometryEntry {
                feature_id,
                status: status.to_string(),
                last_valid_geometry_fingerprint: feature
                    .last_valid_geometry_fingerprint
                    .clone()
                    .filter(|fingerprint| !fingerprint.is_empty())?,
            })
        })
        .collect()
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

fn canonical_model_fingerprint(bundle: &LoadedBundle) -> String {
    canonical_model_fingerprint_parts(
        &bundle.graph,
        &bundle.components,
        &bundle.history,
        &bundle.generation.revisions,
    )
}

fn canonical_model_fingerprint_from_state(
    bundle: &LoadedBundle,
    state: &threeterm_persistence::CanonicalState,
) -> String {
    let mut revisions = bundle.generation.revisions.clone();
    if let Some(revision) = revisions.first_mut() {
        revision.features = state.feature_ids.clone();
    }
    canonical_model_fingerprint_parts(&state.graph, &state.components, &state.history, &revisions)
}

fn canonical_model_fingerprint_parts(
    graph: &FeatureGraph,
    components: &ComponentGraph,
    history: &HistoryState,
    revisions: &[threeterm_domain::Revision],
) -> String {
    let bytes = serde_json::to_vec(&(graph, components, history, revisions))
        .expect("canonical model state serializes");
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_geometry_fingerprints(bundle: &LoadedBundle) -> Vec<String> {
    let mut latest = std::collections::BTreeMap::new();
    for entry in bundle.log.entries() {
        if bundle.graph.contains_feature(&entry.feature_id)
            && let Some(digest) = &entry.brep_sha256
        {
            latest.insert(entry.feature_id.as_str(), digest.clone());
        }
    }
    latest.into_values().collect()
}

fn geometry_fingerprint_map_from_loaded(
    bundle: &LoadedBundle,
) -> std::collections::BTreeMap<String, String> {
    let mut fingerprints = std::collections::BTreeMap::new();
    for entry in bundle.log.entries() {
        if bundle.graph.contains_feature(&entry.feature_id)
            && let Some(digest) = &entry.brep_sha256
        {
            fingerprints.insert(entry.feature_id.clone(), digest.clone());
        }
    }
    fingerprints
}

fn geometry_fingerprint_map(
    feature_ids: &[String],
    fingerprints: &[String],
) -> std::collections::BTreeMap<String, String> {
    feature_ids
        .iter()
        .cloned()
        .zip(fingerprints.iter().cloned())
        .collect()
}

fn authenticated_geometry_fingerprints(
    root: &Path,
    bundle: &LoadedBundle,
) -> Result<(Vec<String>, Option<String>), HostError> {
    let mut latest = std::collections::BTreeMap::new();
    for entry in bundle.log.entries() {
        if bundle.graph.contains_feature(&entry.feature_id) && entry.brep_sha256.is_some() {
            latest.insert(entry.feature_id.as_str(), entry);
        }
    }
    let mut fingerprints = Vec::with_capacity(latest.len());
    for (feature_id, entry) in latest {
        let path = committed_brep_path(root, feature_id);
        let Ok(bytes) = fs::read(&path) else {
            return Ok((
                fingerprints,
                Some(format!(
                    "geometry result is missing for canonical feature {feature_id}"
                )),
            ));
        };
        let actual = format!("{:x}", Sha256::digest(bytes));
        if entry.brep_sha256.as_deref() != Some(actual.as_str()) {
            return Ok((
                fingerprints,
                Some(format!(
                    "geometry result differs for canonical feature {feature_id}"
                )),
            ));
        }
        fingerprints.push(actual);
    }
    Ok((fingerprints, None))
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
    format!(
        "bracket:length={:.17};width={:.17};height={:.17};thickness={:.17}",
        request.length, request.width, request.height, request.thickness,
    )
}

fn bracket_draft_fingerprint(draft: &BracketParameterDraft) -> String {
    let semantic = serde_json::json!({
        "bracket_id": draft.bracket_id,
        "height": format!("{:.17}", draft.request.height),
        "length": format!("{:.17}", draft.request.length),
        "source_brep_sha256": draft.source_brep_sha256,
        "source_revision": draft.source_revision,
        "thickness": format!("{:.17}", draft.request.thickness),
        "width": format!("{:.17}", draft.request.width),
    });
    let bytes = serde_json::to_vec(&semantic).expect("bracket draft fingerprint serializes");
    format!("{:x}", Sha256::digest(bytes))
}

fn bracket_semantic_fingerprint(draft: &BracketParameterDraft) -> String {
    let semantic = serde_json::json!({
        "bracket_id": draft.bracket_id,
        "height": format!("{:.17}", draft.request.height),
        "length": format!("{:.17}", draft.request.length),
        "thickness": format!("{:.17}", draft.request.thickness),
        "width": format!("{:.17}", draft.request.width),
    });
    let bytes = serde_json::to_vec(&semantic).expect("bracket semantic fingerprint serializes");
    format!("{:x}", Sha256::digest(bytes))
}

fn bracket_idempotency_payload(
    draft: &BracketParameterDraft,
    result_sha256: &str,
    input_fingerprint: &str,
) -> String {
    serde_json::json!({
        "input_fingerprint": input_fingerprint,
        "result_sha256": result_sha256,
        "semantic_fingerprint": bracket_semantic_fingerprint(draft),
        "source_revision": draft.source_revision,
    })
    .to_string()
}

fn bracket_input_fingerprint(draft: &BracketParameterDraft, result_sha256: &str) -> String {
    let semantic = serde_json::json!({
        "bracket_id": draft.bracket_id,
        "height": format!("{:.17}", draft.request.height),
        "length": format!("{:.17}", draft.request.length),
        "result_sha256": result_sha256,
        "source_brep_sha256": draft.source_brep_sha256,
        "source_revision": draft.source_revision,
        "thickness": format!("{:.17}", draft.request.thickness),
        "width": format!("{:.17}", draft.request.width),
    });
    let bytes = serde_json::to_vec(&semantic).expect("bracket fingerprint serializes");
    format!("{:x}", Sha256::digest(bytes))
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

fn discard_stage(stage: Stage, diagnostic: Diagnostic) -> Diagnostic {
    let _ = stage.discard();
    diagnostic
}

fn reject_staged_artifact(
    stage: Stage,
    request_staging_name: &str,
    header_staging_name: &str,
    diagnostic: Diagnostic,
    discard_stage_on_error: bool,
) -> Diagnostic {
    if discard_stage_on_error {
        return discard_stage(stage, diagnostic);
    }
    stage.discard_staged(request_staging_name);
    if header_staging_name != request_staging_name {
        stage.discard_staged(header_staging_name);
    }
    diagnostic
}
fn expected_occt_worker_fingerprint() -> WorkerFingerprint {
    WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: threeterm_occt_worker::SCHEMA_VERSION.to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    }
}

fn validate_layer1_cache(root: &Path, loaded: &LoadedBundle) -> Result<(), HostError> {
    let cache = root.join(LAYER1_CACHE_DIR);
    let record_path = cache.join(LAYER1_CACHE_RECORD);
    if !record_path.exists() {
        return Ok(());
    }
    let record: Layer1CacheRecord =
        serde_json::from_slice(&fs::read(&record_path).map_err(|error| HostError::BrepIo {
            detail: format!("read Layer 1 cache record failed: {error}"),
        })?)
        .map_err(|error| HostError::Validation {
            detail: format!("Layer 1 cache record is invalid: {error}"),
        })?;
    let expected = expected_occt_worker_fingerprint();
    if record.worker_fingerprint != expected {
        let _ = fs::remove_file(&record_path);
        // The record is untrusted at this point. Only remove the fixed cache
        // artifact owned by this boundary; never join an attacker-controlled
        // filename before validating it.
        let _ = fs::remove_file(cache.join("l-bracket.brep"));
        return Err(HostError::Layer1FingerprintMismatch {
            expected: Box::new(expected),
            found: Box::new(record.worker_fingerprint),
        });
    }
    if record.schema_version != LAYER1_CACHE_SCHEMA
        || record.source_revision != loaded.revision_hash_hex()
        || record.operation != "bracket"
        || record.feature_id != "l-bracket"
        || record.artifact_name.contains('/')
        || record.artifact_name.contains('\\')
    {
        return Err(HostError::Validation {
            detail: "Layer 1 cache record does not match the canonical revision".to_string(),
        });
    }
    let artifact = cache.join(&record.artifact_name);
    let metadata = fs::metadata(&artifact).map_err(|error| HostError::BrepIo {
        detail: format!("read Layer 1 cache artifact failed: {error}"),
    })?;
    let actual = sha256_path(&artifact).map_err(|error| HostError::BrepIo {
        detail: error.to_string(),
    })?;
    if !metadata.is_file() || metadata.len() != record.byte_count || actual != record.sha256 {
        return Err(HostError::Validation {
            detail: "Layer 1 cache artifact failed integrity validation".to_string(),
        });
    }
    Ok(())
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
            mode: request.mode.as_str(),
            target_feature_id: request.target_feature_id.as_deref(),
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

fn boolean_derived_result_json(artifact: &Layer1DerivedResult) -> serde_json::Value {
    serde_json::json!({
        "request_id": artifact.request_id,
        "operation": artifact.operation,
        "feature_id": artifact.feature_id,
        "source_revision_id": artifact.source_revision_id,
        "worker_fingerprint": serde_json::to_value(&artifact.worker_fingerprint).unwrap_or(serde_json::Value::Null),
        "artifact_kind": artifact.artifact_kind,
        "artifact_name": artifact.artifact_name,
        "byte_count": artifact.byte_count,
        "sha256": artifact.sha256,
    })
}

fn canonical_boolean_intent(
    request: &serde_json::Value,
    source_snapshot: &SnapshotView,
    artifact: &Layer1DerivedResult,
) -> Result<CanonicalBooleanIntent, HostError> {
    // The staged request carries disposable absolute paths; the canonical
    // operand identity is the BREP file stem (`<feature-id>.brep`).
    let stem_of = |field: &str| {
        request[field]
            .as_str()
            .and_then(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".brep").map(str::to_string))
            })
            .unwrap_or_default()
    };
    let intent = CanonicalBooleanIntent {
        schema_version: BOOLEAN_INTENT_SCHEMA_VERSION.to_string(),
        command: "boolean".to_string(),
        operation: boolean_operation_name(&artifact.operation).to_string(),
        base_feature_id: stem_of("base_path"),
        tool_feature_id: stem_of("tool_path"),
        request_id: artifact.request_id.clone(),
        affected_semantic_ids: vec![artifact.feature_id.clone()],
        source_revision: source_snapshot.revision_hash.clone(),
        worker_requirements: artifact.worker_fingerprint.clone(),
    };
    intent
        .validate(&artifact.feature_id)
        .map_err(|error| HostError::Validation {
            detail: error.to_string(),
        })?;
    Ok(intent)
}

fn boolean_operation_name(operation: &str) -> &str {
    match operation {
        "boolean_fuse" | "boolean-fuse" | "fuse" => "fuse",
        "boolean_cut" | "boolean-cut" | "cut" => "cut",
        "boolean_common" | "boolean-common" | "common" => "common",
        other => other,
    }
}

fn canonical_extrude_intent(
    request: &serde_json::Value,
    source_snapshot: &SnapshotView,
    artifact: &Layer1DerivedResult,
) -> Result<CanonicalExtrudeIntent, HostError> {
    let profile = serde_json::from_value(request["profile"].clone()).map_err(|error| {
        HostError::Validation {
            detail: format!("canonical extrude profile is invalid: {error}"),
        }
    })?;
    let height = request["height"]
        .as_f64()
        .ok_or_else(|| HostError::Validation {
            detail: "canonical extrude height is missing or invalid".to_string(),
        })?;
    let mode = parse_extrude_mode(request.get("mode"))?;
    let target_feature_id = request
        .get("target_feature_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let intent = CanonicalExtrudeIntent {
        schema_version: EXTRUDE_INTENT_SCHEMA_VERSION.to_string(),
        command: "extrude".to_string(),
        operation: mode.as_str().to_string(),
        mode: mode.as_str().to_string(),
        target_feature_id,
        request_id: artifact.request_id.clone(),
        deterministic_inputs: ExtrudeDeterministicInputs { profile, height },
        affected_semantic_ids: vec![artifact.feature_id.clone()],
        source_revision: source_snapshot.revision_hash.clone(),
        worker_requirements: artifact.worker_fingerprint.clone(),
    };
    intent
        .validate(&artifact.feature_id)
        .map_err(|error| HostError::Validation {
            detail: error.to_string(),
        })?;
    Ok(intent)
}

fn parse_hole_kind(value: Option<&serde_json::Value>) -> Result<String, HostError> {
    match value.and_then(serde_json::Value::as_str) {
        None => Ok("drilled".to_string()),
        Some("drilled") => Ok("drilled".to_string()),
        Some("tapped") => Ok("tapped".to_string()),
        Some(other) => Err(HostError::Validation {
            detail: format!("hole kind must be drilled or tapped, got {other:?}"),
        }),
    }
}

fn canonical_hole_intent(
    request: &serde_json::Value,
    base_feature_id: &str,
    source_snapshot: &SnapshotView,
    artifact: &Layer1DerivedResult,
) -> Result<CanonicalHoleIntent, HostError> {
    let position: [f64; 3] =
        serde_json::from_value(request["position"].clone()).map_err(|error| {
            HostError::Validation {
                detail: format!("canonical hole position is invalid: {error}"),
            }
        })?;
    let direction: [f64; 3] =
        serde_json::from_value(request["direction"].clone()).map_err(|error| {
            HostError::Validation {
                detail: format!("canonical hole direction is invalid: {error}"),
            }
        })?;
    let diameter = request["diameter"]
        .as_f64()
        .ok_or_else(|| HostError::Validation {
            detail: "canonical hole diameter is missing or invalid".to_string(),
        })?;
    let hole_kind = parse_hole_kind(request.get("hole_kind"))?;
    let thread_designation = request
        .get("thread_designation")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let thread_pitch = request
        .get("thread_pitch")
        .and_then(serde_json::Value::as_f64);
    let thread_depth = request
        .get("thread_depth")
        .and_then(serde_json::Value::as_f64);
    if base_feature_id.is_empty() {
        return Err(HostError::Validation {
            detail: "canonical hole base feature is missing".to_string(),
        });
    }
    let intent = CanonicalHoleIntent {
        schema_version: HOLE_INTENT_SCHEMA_VERSION.to_string(),
        command: "hole".to_string(),
        hole_kind,
        base_feature_id: base_feature_id.to_string(),
        request_id: artifact.request_id.clone(),
        deterministic_inputs: HoleDeterministicInputs {
            position,
            direction,
            diameter,
            thread_designation,
            thread_pitch,
            thread_depth,
        },
        affected_semantic_ids: vec![artifact.feature_id.clone()],
        source_revision: source_snapshot.revision_hash.clone(),
        worker_requirements: artifact.worker_fingerprint.clone(),
    };
    intent
        .validate(&artifact.feature_id)
        .map_err(|error| HostError::Validation {
            detail: error.to_string(),
        })?;
    Ok(intent)
}

fn occt_artifact_request(
    request: &serde_json::Value,
    operation: threeterm_occt_worker::Operation,
    source_snapshot: &SnapshotView,
    request_id: &str,
    feature_id: &str,
) -> Result<Layer1ArtifactRequest, HostError> {
    let mut semantic = request.clone();
    if let Some(object) = semantic.as_object_mut() {
        for field in [
            "output_dir",
            "output_filename",
            "artifact_request",
            "target_path",
        ] {
            object.remove(field);
        }
        for field in ["base_path", "tool_path"] {
            if object.contains_key(field) {
                object.insert(
                    field.to_string(),
                    serde_json::Value::String("<canonical-source>".to_string()),
                );
            }
        }
    }
    let semantic_input = threeterm_protocol::worker::serialize_capped(
        &semantic,
        threeterm_protocol::frame::MAX_FRAME_BUFFER,
    )
    .map_err(|error| HostError::Validation {
        detail: format!("OCCT semantic input serialization failed: {error}"),
    })?;
    Ok(Layer1ArtifactRequest {
        request_id: request_id.to_string(),
        source_revision_id: source_snapshot.revision_hash.clone(),
        operation: operation.as_str().to_string(),
        feature_id: feature_id.to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: format!("{}-{}.brep", operation.as_str(), request_id),
        semantic_input_sha256: sha256_hex(&semantic_input),
        deterministic_settings_sha256: sha256_hex(b"threeterm.occt.derived-settings/1"),
    })
}

#[derive(Debug, Serialize)]
struct ExtrudeSemanticInput<'a> {
    operation: &'static str,
    feature_id: &'a str,
    profile: &'a [[f64; 2]],
    height: f64,
    mode: &'static str,
    target_feature_id: Option<&'a str>,
}

fn parse_extrude_mode(value: Option<&serde_json::Value>) -> Result<ExtrudeMode, HostError> {
    let value = value
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| HostError::Validation {
            detail: "missing extrude mode".to_string(),
        })?;
    parse_extrude_mode_value(value)
}

fn parse_extrude_mode_value(value: &str) -> Result<ExtrudeMode, HostError> {
    match value {
        "additive" => Ok(ExtrudeMode::Additive),
        "subtractive" => Ok(ExtrudeMode::Subtractive),
        _ => Err(HostError::Validation {
            detail: format!("invalid extrude mode: {value}"),
        }),
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
#[cfg(test)]
fn copy_brep(source: &Path, target: &Path) -> Result<(), String> {
    copy_brep_verified(source, target, None)
}

fn read_brep_verified(source: &Path, expected: Option<(usize, &str)>) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    const O_NOFOLLOW: i32 = 0o400000;
    const O_NONBLOCK: i32 = 0o4000;
    let mut options = fs::OpenOptions::new();
    options.read(true).custom_flags(O_NOFOLLOW | O_NONBLOCK);
    let mut reader = options
        .open(source)
        .map_err(|error| format!("open source BREP {} failed: {error}", source.display()))?;
    let opened_metadata = reader
        .metadata()
        .map_err(|error| format!("stat opened BREP {} failed: {error}", source.display()))?;
    if !opened_metadata.is_file() {
        return Err(format!(
            "source BREP {} is not a regular file",
            source.display()
        ));
    }
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
    let mut buffer = [0u8; 8 * 1024];
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
    Ok(content)
}

#[cfg(test)]
fn copy_brep_verified(
    source: &Path,
    target: &Path,
    expected: Option<(usize, &str)>,
) -> Result<(), String> {
    let content = read_brep_verified(source, expected)?;
    write_brep_bytes(target, &content)
}

#[cfg(test)]
fn write_brep_bytes(target: &Path, content: &[u8]) -> Result<(), String> {
    use std::io::Write;

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
    if let Err(error) = writer.write_all(content) {
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

fn json_values_match_worker_result(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    match (left, right) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| json_values_match_worker_result(value, other))
                })
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(value, other)| json_values_match_worker_result(value, other))
        }
        (serde_json::Value::Number(left), serde_json::Value::Number(right)) => {
            left == right
                || match (left.as_i64(), left.as_u64(), right.as_i64(), right.as_u64()) {
                    (Some(left), _, Some(right), _) => left == right,
                    (_, Some(left), _, Some(right)) => left == right,
                    (Some(left), _, _, Some(right)) => left >= 0 && left as u64 == right,
                    (_, Some(left), Some(right), _) => right >= 0 && left == right as u64,
                    _ => left
                        .as_f64()
                        .is_some_and(|left| right.as_f64().is_some_and(|right| left == right)),
                }
        }
        (left, right) => left == right,
    }
}

impl Drop for WorkerStageCleanup<'_> {
    fn drop(&mut self) {
        cleanup_worker_stage(self.root, self.path);
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
    fn worker_result_comparison_accepts_equivalent_float_and_integer_numbers() {
        assert!(json_values_match_worker_result(
            &serde_json::json!({"edge_candidates": [{"length": 4}]}),
            &serde_json::json!({"edge_candidates": [{"length": 4.0}]}),
        ));
        assert!(!json_values_match_worker_result(
            &serde_json::json!({"edge_candidates": [{"length": 4}]}),
            &serde_json::json!({"edge_candidates": [{"length": 5.0}]}),
        ));
    }

    #[test]
    fn viewport_scene_requires_the_committed_brep_for_geometric_records() {
        let root = temp_root("viewport-missing-brep");
        Bundle::create(&root)
            .expect("bundle creates")
            .append_feature("lofted", "brep:lofted")
            .expect("geometric feature appends");
        let host = Host::new();
        host.load(&root).expect("bundle loads");

        assert!(matches!(
            host.presentation_viewport_scene(),
            Err(HostError::BrepFileMissing { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn viewport_stl_parser_rejects_incomplete_triangle_data() {
        let root = temp_root("viewport-malformed-stl");
        fs::create_dir_all(&root).expect("stage creates");
        let path = root.join("malformed.stl");
        fs::write(
            &path,
            "solid malformed\n  facet normal 0 0 1\n    outer loop\n      vertex 0 0 0\n      vertex 1 0 0\n    endloop\n  endfacet\nendsolid malformed\n",
        )
        .expect("malformed STL writes");

        assert!(matches!(
            parse_ascii_stl(&path, "lofted"),
            Err(HostError::BrepInvalid { .. })
        ));
        let _ = fs::remove_dir_all(root);
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
            manifest["transaction_count"], 3,
            "save_bracket must record two features and one history transaction"
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
    fn undo_and_redo_move_the_active_revision_snapshot() {
        let root = temp_root("undo-redo-movement");
        let host = Host::new();
        host.save_bracket(&root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
            .expect("history initializes");
        host.historical_edit(&root, "l-bracket-base", "length", 61.0)
            .expect("historical edit commits");
        assert_eq!(
            host.history(&root)
                .expect("history reloads")
                .active_snapshot()
                .revision_id,
            "history-revision-2"
        );

        let undone = host.undo(&root).expect("undo moves back");
        assert_eq!(
            undone.history.active_snapshot().revision_id,
            "history-revision-1"
        );
        assert_eq!(
            undone.history.active_snapshot().features["l-bracket-base"].input_value,
            60.0
        );
        assert_eq!(
            host.history(&root)
                .expect("history reloads after undo")
                .active_snapshot()
                .revision_id,
            "history-revision-1",
            "undo survives bundle reload"
        );

        let redone = host.redo(&root).expect("redo moves forward");
        assert_eq!(
            redone.history.active_snapshot().revision_id,
            "history-revision-2"
        );
        assert_eq!(
            redone.history.active_snapshot().features["l-bracket-base"].input_value,
            61.0
        );

        assert!(
            host.redo(&root).is_err(),
            "redo with no undone work is rejected atomically"
        );
        assert_eq!(
            host.history(&root)
                .expect("history reloads after rejected redo")
                .active_snapshot()
                .revision_id,
            "history-revision-2"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn successful_historical_edit_without_a_worker_is_rejected_atomically() {
        if threeterm_occt_worker::OcctWorker::locate().is_ok() {
            eprintln!("successful_historical_edit_without_a_worker: OCCT worker present");
            return;
        }
        let root = temp_root("edit-without-worker");
        let host = Host::new();
        host.save_bracket(&root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
            .expect("history initializes");
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).expect("staging dir creates");
        let seed = staging.join("seed.brep");
        std::fs::write(&seed, [0x62, 0x72, 0x65, 0x70]).expect("seed writes");
        host.commit_brep_feature(&root, "l-bracket", &seed)
            .expect("seed BREP commits");
        let before_bytes =
            std::fs::read(root.join("brep/l-bracket.brep")).expect("seeded BREP reads");
        let before_ordinal = host
            .history(&root)
            .expect("history reloads")
            .event_ordinal();

        let error = host
            .historical_edit(&root, "l-bracket-base", "length", 61.0)
            .expect_err("committed geometry must follow history, never go stale silently");
        assert!(
            matches!(error, HostError::WorkerFailure { .. }),
            "missing worker surfaces a worker failure, got {error:?}"
        );
        assert_eq!(
            host.history(&root)
                .expect("history reloads")
                .event_ordinal(),
            before_ordinal,
            "rejected recompute commits no history transaction"
        );
        assert_eq!(
            std::fs::read(root.join("brep/l-bracket.brep")).expect("BREP reads"),
            before_bytes,
            "rejected recompute leaves committed bytes untouched"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_historical_edit_preserves_committed_brep_under_the_stale_gate() {
        let root = temp_root("failed-edit-brep");
        let output = temp_root("failed-edit-brep-output");
        let host = Host::new();
        host.save_bracket(&root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
            .expect("history initializes");
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).expect("staging dir creates");
        let seed = staging.join("seed.brep");
        std::fs::write(&seed, [0x62, 0x72, 0x65, 0x70]).expect("seed writes");
        host.commit_brep_feature(&root, "l-bracket", &seed)
            .expect("seed BREP commits");
        let before_bytes =
            std::fs::read(root.join("brep/l-bracket.brep")).expect("seeded BREP reads");

        host.historical_edit(&root, "l-bracket-base", "length", 0.0)
            .expect("failing historical edit is committed");
        assert_eq!(
            std::fs::read(root.join("brep/l-bracket.brep")).expect("BREP reads"),
            before_bytes,
            "last-valid bytes stay on disk as recovery state"
        );
        let error = host
            .export(
                &root,
                "l-bracket",
                &["stl".to_string()],
                &output,
                0.5,
                false,
                false,
                &[],
            )
            .expect_err("broken family stays gated");
        assert!(
            matches!(error, HostError::StaleLastValidGeometry { .. }),
            "failed edit keeps the stale gate, got {error:?}"
        );

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(output);
    }

    #[test]
    fn successful_historical_edit_recomputes_only_the_dirty_family() {
        let Ok(worker) = threeterm_occt_worker::OcctWorker::locate() else {
            eprintln!(
                "successful_historical_edit_recomputes_only_the_dirty_family: OCCT worker unavailable"
            );
            return;
        };
        let root = temp_root("edit-dirty-family");
        let host = Host::new();
        for bracket_id in ["family-a", "family-b"] {
            let request = threeterm_occt_worker::BracketRequest::new(
                format!("test-{bracket_id}"),
                60.0,
                30.0,
                40.0,
                3.0,
            )
            .with_feature_id(bracket_id);
            host.create_bracket(&root, request, &worker)
                .expect("production bracket commits");
        }
        let before_a = std::fs::read(root.join("brep/family-a.brep")).expect("family A BREP reads");
        let before_b = std::fs::read(root.join("brep/family-b.brep")).expect("family B BREP reads");

        let edited = host
            .historical_edit(&root, "family-a-base", "length", 61.0)
            .expect("successful edit recomputes");
        assert_eq!(
            edited.history.active_snapshot().revision_id,
            "history-revision-3"
        );
        assert!(edited.history.active_snapshot().features.values().all(
            |feature| feature.status == threeterm_domain::history::HistoryStatus::CurrentValid
        ));
        assert_ne!(
            std::fs::read(root.join("brep/family-a.brep")).expect("family A BREP re-reads"),
            before_a,
            "the dirty family recomputes real geometry"
        );
        assert_eq!(
            std::fs::read(root.join("brep/family-b.brep")).expect("family B BREP re-reads"),
            before_b,
            "the clean family keeps byte-identical results"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn undo_restores_the_recomputed_family_geometry() {
        let Ok(worker) = threeterm_occt_worker::OcctWorker::locate() else {
            eprintln!("undo_restores_the_recomputed_family_geometry: OCCT worker unavailable");
            return;
        };
        let root = temp_root("undo-family-geometry");
        let host = Host::new();
        let request =
            threeterm_occt_worker::BracketRequest::new("test-undo", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("family-a");
        host.create_bracket(&root, request, &worker)
            .expect("production bracket commits");
        let initial = std::fs::read(root.join("brep/family-a.brep")).expect("initial BREP reads");
        host.historical_edit(&root, "family-a-base", "length", 61.0)
            .expect("edit recomputes");
        assert_ne!(
            std::fs::read(root.join("brep/family-a.brep")).expect("edited BREP reads"),
            initial
        );

        let undone = host.undo(&root).expect("undo recomputes back");
        assert_eq!(
            undone.history.active_snapshot().revision_id,
            "history-revision-1"
        );
        assert_eq!(
            std::fs::read(root.join("brep/family-a.brep")).expect("undone BREP reads"),
            initial,
            "undo restores the prior committed geometry bytes"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn export_rejects_stale_history_before_brep_or_staging_side_effects() {
        let root = temp_root("stale-export-gate");
        let output = temp_root("stale-export-output");
        let host = Host::new();
        host.save_bracket(&root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
            .expect("history initializes");
        host.historical_edit(&root, "l-bracket-base", "length", 0.0)
            .expect("failing historical edit is committed");
        let manifest = std::fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
        let log = std::fs::read(root.join(threeterm_persistence::TRANSACTIONS_LOG_FILENAME))
            .expect("log reads");

        let error = host
            .export(
                &root,
                "l-bracket",
                &["stl".to_string()],
                &output,
                0.5,
                false,
                false,
                &[],
            )
            .expect_err("stale family must require explicit acceptance");
        match error {
            HostError::StaleLastValidGeometry {
                feature_id,
                active_revision,
                stale_features,
            } => {
                assert_eq!(feature_id, "l-bracket");
                assert!(active_revision.starts_with("history-revision-"));
                assert_eq!(
                    stale_features
                        .iter()
                        .map(|feature| feature.feature_id.as_str())
                        .collect::<Vec<_>>(),
                    ["l-bracket-base", "l-bracket-bend", "l-bracket-finish"]
                );
                assert!(
                    stale_features
                        .iter()
                        .all(|feature| !feature.last_valid_geometry_fingerprint.is_empty())
                );
            }
            other => panic!("unexpected export error: {other:?}"),
        }
        assert_eq!(
            std::fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
            manifest
        );
        assert_eq!(
            std::fs::read(root.join(threeterm_persistence::TRANSACTIONS_LOG_FILENAME)).unwrap(),
            log
        );
        assert!(!output.exists());
        assert_eq!(
            host.history(&root)
                .expect("history reloads")
                .active_snapshot()
                .revision_id,
            "history-revision-2"
        );

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(output);
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
    fn commit_brep_feature_rejects_existing_feature_id_without_replacing_bytes() {
        let root = temp_root("brep-replace");
        let bundle = Bundle::create(&root).expect("bundle creates");
        let revision = bundle
            .open()
            .expect("bundle opens")
            .revision_hash_hex()
            .to_string();
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).expect("staging dir creates");
        let brep_dir = root.join("brep");
        let prior_bytes: Vec<u8> = (0..128u8).collect();
        bundle
            .append_feature_with_brep_if_revision("box-1", "brep:box-1", &revision, &prior_bytes)
            .expect("prior BREP publishes");

        let new_source = staging.join("new.brep");
        let new_bytes: Vec<u8> = (128..=255u8).cycle().take(128).collect();
        std::fs::write(&new_source, &new_bytes).expect("new BREP writes");

        let host = Host::new();
        let prior = host.load(&root).expect("host loads prior");
        let result = host.commit_brep_feature(&root, "box-1", &new_source);
        assert!(matches!(result, Err(HostError::Validation { .. })));

        let committed = std::fs::read(brep_dir.join("box-1.brep")).expect("reads");
        assert_eq!(committed, prior_bytes, "BREP bytes remain unchanged");
        let reloaded = host.load(&root).expect("reloads");
        assert_eq!(reloaded, prior);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn three_mf_writer_emits_named_objects_and_recoverable_generation_metadata() {
        let root = temp_root("3mf-writer");
        std::fs::create_dir_all(&root).expect("writer root creates");
        let stl = root.join("body.stl");
        std::fs::write(
            &stl,
            "solid body\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid body\n",
        )
        .expect("fixture STL writes");
        let destination = root.join("body.3mf");
        write_3mf(
            &[
                ThreeMfBody {
                    label: "first<&".to_string(),
                    stl: stl.clone(),
                },
                ThreeMfBody {
                    label: "second".to_string(),
                    stl,
                },
            ],
            "generation-1",
            "revision-2",
            &["first".to_string(), "second".to_string()],
            &"a".repeat(64),
            &"b".repeat(64),
            &destination,
        )
        .expect("3MF writes");
        let bytes = std::fs::read(&destination).expect("3MF reads");
        let archive = String::from_utf8_lossy(&bytes);
        assert_eq!(archive.matches("<object ").count(), 2);
        assert_eq!(archive.matches("<item ").count(), 2);
        assert!(archive.contains("name=\"first&lt;&amp;\""));
        assert!(archive.contains("threeterm.generation_id"));
        assert!(archive.contains("generation-1"));
        assert!(archive.contains("revision-2"));
        assert!(archive.contains("threeterm.feature_ids\">[&quot;first&quot;,&quot;second&quot;]"));
        assert!(archive.contains(&"a".repeat(64)));
        assert!(archive.contains(&"b".repeat(64)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extrude_artifact_request_uses_the_pinned_semantic_input_order() {
        let request =
            ExtrudeRequest::new("request-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 2.0)
                .with_feature_id("feature-1");
        let snapshot = SnapshotView {
            generation_id: "generation-1".to_string(),
            feature_graph_hash: "a".repeat(64),
            revision_hash: "b".repeat(64),
            recovered_from_previous: false,
        };
        let binding = extrude_artifact_request(&request, &snapshot).expect("binding derives");
        let expected = br#"{"operation":"extrude","feature_id":"feature-1","profile":[[0.0,0.0],[1.0,0.0],[1.0,1.0]],"height":2.0,"mode":"additive","target_feature_id":null}"#;

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
            generation_id: "generation-1".to_string(),
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

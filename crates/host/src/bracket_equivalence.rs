//! Cross-surface geometric comparison for the retained L-bracket journeys.
//!
//! This module compares measurements derived from independently parsed meshes.
//! It intentionally has no byte, triangle-order, or application-identity
//! equality path.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use threeterm_protocol::artifact::sha256_hex;

use crate::bracket_oracle::{assert_bracket_mesh, cross, dot, point_inside_mesh, subtract};
use crate::stl_integrity::{StlIntegrityReport, StlMeshObservation, observe_path, verify_path};

pub const JOURNEY_EVIDENCE_REPORT_SCHEMA_VERSION: &str = "threeterm.evidence.bracket-journey/1";
pub const AGGREGATE_REPORT_SCHEMA_VERSION: &str = "threeterm.evidence.bracket-equivalence/1";
pub const API_SURFACE: &str = "api";
pub const MCP_SURFACE: &str = "mcp";
pub const TUI_SURFACE: &str = "tui";
pub const PRODUCER_SURFACES: [&str; 3] = [API_SURFACE, MCP_SURFACE, TUI_SURFACE];
pub const REQUIRED_JOURNEY_COMMANDS: [&str; 18] = [
    "list",
    "new-project",
    "extrude",
    "boolean-fuse",
    "fillet",
    "chamfer",
    "hole",
    "revolve",
    "mirror",
    "linear-pattern",
    "circular-pattern",
    "shell",
    "draft",
    "loft",
    "save",
    "load",
    "validate",
    "export",
];

const JOURNEY_TESTS: [(&str, &str); 3] = [
    (API_SURFACE, "e2e_stl_api_all_tools_l_bracket"),
    (MCP_SURFACE, "e2e_stl_mcp_all_tools_l_bracket"),
    (TUI_SURFACE, "production_tui_all_tools_stl_journey"),
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TolerancePolicy {
    pub linear_mm: f64,
    pub placement_mm: f64,
    pub volume_fraction: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometrySignature {
    pub dimensions: [f64; 3],
    pub material_volume: f64,
    pub topology: TopologySignature,
    pub voids: Vec<ProbeSample>,
    pub landmarks: Vec<ProbeSample>,
    pub surface_samples: Vec<SurfaceSample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologySignature {
    pub shell_count: usize,
    pub material_shell_count: usize,
    pub cavity_shell_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeSample {
    pub name: String,
    pub occupied: bool,
    pub intersections: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceSample {
    pub name: String,
    pub intersections: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeIdentity {
    pub schema_version: String,
    pub digest: String,
    pub units: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub commit: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub revision_snapshot_hash: String,
    pub integrity: Option<StlIntegrityReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JourneyEvidenceReport {
    pub schema_version: String,
    pub run_id: String,
    pub producer_surface: String,
    pub test: String,
    pub result: String,
    pub recipe: RecipeIdentity,
    pub source: SourceIdentity,
    pub schemas: Value,
    pub workers: Value,
    pub runtime: Value,
    pub artifact: ArtifactRecord,
    pub evidence: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JourneyMetadata {
    pub source: SourceIdentity,
    pub schemas: Value,
    pub workers: Value,
    pub runtime: Value,
    pub evidence: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProducerSurfaceSignature {
    pub producer_surface: String,
    pub artifact: ArtifactRecord,
    pub geometry: GeometrySignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairComparison {
    pub left_producer_surface: String,
    pub right_producer_surface: String,
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateReport {
    pub schema_version: String,
    pub run_id: String,
    pub recipe: RecipeIdentity,
    pub source: SourceIdentity,
    pub producer_surfaces: Vec<ProducerSurfaceSignature>,
    pub comparisons: Vec<PairComparison>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EquivalenceError {
    Io(String),
    Report(String),
    IndependentExpectation {
        producer_surface: String,
        detail: String,
    },
    Comparison(ComparisonFailure),
}

impl std::fmt::Display for EquivalenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(detail) | Self::Report(detail) => formatter.write_str(detail),
            Self::IndependentExpectation {
                producer_surface,
                detail,
            } => {
                write!(
                    formatter,
                    "{producer_surface} independent expectation failed: {detail}"
                )
            }
            Self::Comparison(failure) => failure.fmt(formatter),
        }
    }
}

impl std::error::Error for EquivalenceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MismatchKind {
    Dimensions,
    Volume,
    Topology,
    Voids,
    Landmarks,
    SurfaceSamples,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonFailure {
    pub left_producer_surface: String,
    pub right_producer_surface: String,
    pub kind: MismatchKind,
    pub detail: String,
}

impl std::fmt::Display for ComparisonFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} vs {} {:?}: {}",
            self.left_producer_surface, self.right_producer_surface, self.kind, self.detail
        )
    }
}

impl std::error::Error for ComparisonFailure {}

pub fn compare_signatures(
    left_name: &str,
    left: &GeometrySignature,
    right_name: &str,
    right: &GeometrySignature,
    tolerances: TolerancePolicy,
    reference_volume: f64,
) -> Result<(), ComparisonFailure> {
    for (axis, (left_dimension, right_dimension)) in left
        .dimensions
        .into_iter()
        .zip(right.dimensions)
        .enumerate()
    {
        if (left_dimension - right_dimension).abs() > tolerances.linear_mm {
            return Err(failure(
                left_name,
                right_name,
                MismatchKind::Dimensions,
                format!("axis {axis} differs: {left_dimension} vs {right_dimension}"),
            ));
        }
    }

    if !reference_volume.is_finite()
        || reference_volume <= f64::EPSILON
        || !left.material_volume.is_finite()
        || !right.material_volume.is_finite()
    {
        return Err(failure(
            left_name,
            right_name,
            MismatchKind::Volume,
            "volume reference must be finite and positive",
        ));
    }
    let volume_delta = (left.material_volume - right.material_volume).abs() / reference_volume;
    if volume_delta > tolerances.volume_fraction {
        return Err(failure(
            left_name,
            right_name,
            MismatchKind::Volume,
            format!(
                "relative difference {volume_delta:.6} exceeds {}",
                tolerances.volume_fraction
            ),
        ));
    }

    if left.topology != right.topology {
        return Err(failure(
            left_name,
            right_name,
            MismatchKind::Topology,
            format!(
                "topology differs: {:?} vs {:?}",
                left.topology, right.topology
            ),
        ));
    }
    compare_probes(
        left_name,
        &left.voids,
        right_name,
        &right.voids,
        MismatchKind::Voids,
        tolerances.linear_mm,
    )?;
    compare_probes(
        left_name,
        &left.landmarks,
        right_name,
        &right.landmarks,
        MismatchKind::Landmarks,
        tolerances.placement_mm,
    )?;

    if left.surface_samples.len() != right.surface_samples.len() {
        return Err(failure(
            left_name,
            right_name,
            MismatchKind::SurfaceSamples,
            "surface sample counts differ",
        ));
    }
    for (left_sample, right_sample) in left.surface_samples.iter().zip(&right.surface_samples) {
        if left_sample.name != right_sample.name {
            return Err(failure(
                left_name,
                right_name,
                MismatchKind::SurfaceSamples,
                format!(
                    "sample names differ: {} vs {}",
                    left_sample.name, right_sample.name
                ),
            ));
        }
        if !intersections_match(
            &left_sample.intersections,
            &right_sample.intersections,
            tolerances.linear_mm,
        ) {
            return Err(failure(
                left_name,
                right_name,
                MismatchKind::SurfaceSamples,
                format!(
                    "sample {} differs: {:?} vs {:?}",
                    left_sample.name,
                    sorted_intersections(&left_sample.intersections),
                    sorted_intersections(&right_sample.intersections)
                ),
            ));
        }
    }
    Ok(())
}

fn compare_probes(
    left_name: &str,
    left: &[ProbeSample],
    right_name: &str,
    right: &[ProbeSample],
    kind: MismatchKind,
    tolerance: f64,
) -> Result<(), ComparisonFailure> {
    if left.len() != right.len() {
        return Err(failure(left_name, right_name, kind, "probe counts differ"));
    }
    for (left_probe, right_probe) in left.iter().zip(right) {
        if left_probe.name != right_probe.name
            || left_probe.occupied != right_probe.occupied
            || !intersections_match(
                &left_probe.intersections,
                &right_probe.intersections,
                tolerance,
            )
        {
            return Err(failure(
                left_name,
                right_name,
                kind,
                format!("probe results differ: {left_probe:?} vs {right_probe:?}"),
            ));
        }
    }
    Ok(())
}

fn intersections_match(left: &[f64], right: &[f64], tolerance: f64) -> bool {
    let left = sorted_intersections(left);
    let right = sorted_intersections(right);
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| (left - right).abs() <= tolerance)
}

fn sorted_intersections(intersections: &[f64]) -> Vec<f64> {
    let mut sorted = intersections.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted
}

fn failure(
    left: &str,
    right: &str,
    kind: MismatchKind,
    detail: impl Into<String>,
) -> ComparisonFailure {
    ComparisonFailure {
        left_producer_surface: left.to_string(),
        right_producer_surface: right.to_string(),
        kind,
        detail: detail.into(),
    }
}

pub fn recipe_digest(recipe: &Value) -> String {
    let bytes = serde_json::to_vec(recipe).expect("recipe JSON serializes");
    sha256_hex(&bytes)
}

pub fn evidence_root() -> PathBuf {
    if let Some(root) = std::env::var_os("THREETERM_JOURNEY_EVIDENCE_ROOT") {
        return PathBuf::from(root);
    }
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"))
        .join("journey-equivalence")
}

pub fn configured_run_id() -> Option<String> {
    std::env::var("THREETERM_JOURNEY_RUN_ID")
        .ok()
        .filter(|run_id| !run_id.is_empty())
}

fn report_root(root: &Path, run_id: &str) -> Result<PathBuf, EquivalenceError> {
    if run_id.is_empty()
        || run_id == "."
        || run_id == ".."
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(EquivalenceError::Report(
            "journey run ID is not a safe path component".to_string(),
        ));
    }
    Ok(root.join(run_id))
}

pub fn current_source_identity() -> Result<SourceIdentity, EquivalenceError> {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|commit| !commit.is_empty())
        .ok_or_else(|| {
            EquivalenceError::Report("application source commit is unavailable".to_string())
        })?;
    let worktree_dirty = std::process::Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .map(|status| !status.success())
        .unwrap_or(true);
    let index_dirty = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map(|status| !status.success())
        .unwrap_or(true);
    let untracked_dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(true);
    let dirty = worktree_dirty || index_dirty || untracked_dirty;
    Ok(SourceIdentity { commit, dirty })
}

pub fn new_journey_evidence_report(
    run_id: impl Into<String>,
    surface: impl Into<String>,
    test: impl Into<String>,
    recipe: &Value,
    metadata: JourneyMetadata,
    revision_snapshot_hash: impl Into<String>,
) -> JourneyEvidenceReport {
    JourneyEvidenceReport {
        schema_version: JOURNEY_EVIDENCE_REPORT_SCHEMA_VERSION.to_string(),
        run_id: run_id.into(),
        producer_surface: surface.into(),
        test: test.into(),
        result: "passed".to_string(),
        recipe: RecipeIdentity {
            schema_version: recipe["schema_version"]
                .as_str()
                .expect("recipe schema version is a string")
                .to_string(),
            digest: recipe_digest(recipe),
            units: recipe["units"].as_str().unwrap_or("mm").to_string(),
        },
        source: metadata.source,
        schemas: metadata.schemas,
        workers: metadata.workers,
        runtime: metadata.runtime,
        artifact: ArtifactRecord {
            path: String::new(),
            bytes: 0,
            sha256: String::new(),
            revision_snapshot_hash: revision_snapshot_hash.into(),
            integrity: None,
        },
        evidence: metadata.evidence,
    }
}

pub fn publish_journey_evidence_report(
    root: &Path,
    mut report: JourneyEvidenceReport,
    source_stl: &Path,
) -> Result<PathBuf, EquivalenceError> {
    validate_producer_surface(&report.producer_surface)?;
    if report.schema_version != JOURNEY_EVIDENCE_REPORT_SCHEMA_VERSION {
        return Err(EquivalenceError::Report(
            "journey report has an unsupported schema".to_string(),
        ));
    }
    if report.result != "passed" {
        return Err(EquivalenceError::Report(
            "only passed journey reports can be retained".to_string(),
        ));
    }
    if !source_stl.is_file() {
        return Err(EquivalenceError::Io(format!(
            "journey STL does not exist: {}",
            source_stl.display()
        )));
    }
    let run_root = report_root(root, &report.run_id)?;
    let surface_root = run_root.join(&report.producer_surface);
    ensure_directory_tree(&surface_root, "journey evidence")?;
    let artifact_path = surface_root.join("complete-bracket.stl");
    let artifact_tmp =
        surface_root.join(format!(".complete-bracket.stl.{}.tmp", std::process::id()));
    copy_file_exclusive(source_stl, &artifact_tmp)?;
    let publish_result = fs::hard_link(&artifact_tmp, &artifact_path).map_err(io_error);
    let cleanup_result = fs::remove_file(&artifact_tmp).map_err(io_error);
    publish_result.and(cleanup_result)?;

    let integrity = verify_path(&artifact_path).map_err(|error| {
        EquivalenceError::Report(format!(
            "retained {} STL failed independent verification: {error}",
            report.producer_surface
        ))
    })?;
    let bytes = fs::read(&artifact_path).map_err(io_error)?;
    report.artifact.path = format!("{}/complete-bracket.stl", report.producer_surface);
    report.artifact.bytes = bytes.len() as u64;
    report.artifact.sha256 = sha256_hex(&bytes);
    report.artifact.integrity = Some(integrity);

    let report_path = surface_root.join("journey.json");
    write_json_atomically(&report_path, &report)?;
    Ok(report_path)
}

pub fn aggregate_report_path(root: &Path) -> PathBuf {
    root.join("geometric-equivalence.json")
}

pub fn write_aggregate_report(
    root: &Path,
    report: &AggregateReport,
) -> Result<(), EquivalenceError> {
    let run_root = report_root(root, &report.run_id)?;
    write_json_atomically(&aggregate_report_path(&run_root), report)
}

pub fn write_aggregate_failure(
    root: &Path,
    run_id: &str,
    detail: &str,
) -> Result<(), EquivalenceError> {
    let run_root = report_root(root, run_id)?;
    let failure = json!({
        "schema_version": AGGREGATE_REPORT_SCHEMA_VERSION,
        "run_id": run_id,
        "result": "failed",
        "error": detail,
    });
    write_json_atomically(&aggregate_report_path(&run_root), &failure)
}

pub fn compare_three_reports(
    root: &Path,
    run_id: &str,
    recipe: &Value,
) -> Result<AggregateReport, EquivalenceError> {
    let run_root = report_root(root, run_id)?;
    let reports = load_reports(&run_root, run_id, recipe)?;
    let source = reports
        .first()
        .map(|report| report.source.clone())
        .ok_or_else(|| EquivalenceError::Report("no journey reports were loaded".to_string()))?;
    let mut producer_surfaces = Vec::with_capacity(reports.len());
    for report in reports {
        let artifact_path = safe_regular_file(
            &run_root,
            Path::new(&report.artifact.path),
            &format!("{} retained STL", report.producer_surface),
        )?;
        let integrity = verify_path(&artifact_path).map_err(|error| {
            EquivalenceError::IndependentExpectation {
                producer_surface: report.producer_surface.clone(),
                detail: error.to_string(),
            }
        })?;
        if report.artifact.integrity.as_ref() != Some(&integrity) {
            return Err(EquivalenceError::Report(format!(
                "{} retained integrity report does not match the STL",
                report.producer_surface
            )));
        }
        let mesh = observe_path(&artifact_path).map_err(|error| {
            EquivalenceError::IndependentExpectation {
                producer_surface: report.producer_surface.clone(),
                detail: error.to_string(),
            }
        })?;
        assert_bracket_mesh(recipe, &integrity, &mesh).map_err(|failure| {
            EquivalenceError::IndependentExpectation {
                producer_surface: report.producer_surface.clone(),
                detail: failure.to_string(),
            }
        })?;
        let geometry = signature_from_mesh(recipe, &integrity, &mesh, &report.producer_surface)?;
        producer_surfaces.push(ProducerSurfaceSignature {
            producer_surface: report.producer_surface,
            artifact: report.artifact,
            geometry,
        });
    }

    let tolerances = tolerance_policy(recipe)?;
    let reference_volume = producer_surfaces
        .iter()
        .find(|surface| surface.producer_surface == API_SURFACE)
        .and_then(|surface| surface.artifact.integrity.as_ref())
        .map(|integrity| integrity.material_volume)
        .ok_or_else(|| EquivalenceError::Report("API reference volume is missing".to_string()))?;
    let mut comparisons = Vec::new();
    for left in 0..producer_surfaces.len() {
        for right in (left + 1)..producer_surfaces.len() {
            compare_signatures(
                &producer_surfaces[left].producer_surface,
                &producer_surfaces[left].geometry,
                &producer_surfaces[right].producer_surface,
                &producer_surfaces[right].geometry,
                tolerances,
                reference_volume,
            )
            .map_err(EquivalenceError::Comparison)?;
            comparisons.push(PairComparison {
                left_producer_surface: producer_surfaces[left].producer_surface.clone(),
                right_producer_surface: producer_surfaces[right].producer_surface.clone(),
                result: "passed".to_string(),
            });
        }
    }

    Ok(AggregateReport {
        schema_version: AGGREGATE_REPORT_SCHEMA_VERSION.to_string(),
        run_id: run_id.to_string(),
        recipe: RecipeIdentity {
            schema_version: recipe["schema_version"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            digest: recipe_digest(recipe),
            units: recipe["units"].as_str().unwrap_or("mm").to_string(),
        },
        source,
        producer_surfaces,
        comparisons,
    })
}

fn load_reports(
    root: &Path,
    run_id: &str,
    recipe: &Value,
) -> Result<Vec<JourneyEvidenceReport>, EquivalenceError> {
    if run_id.is_empty() {
        return Err(EquivalenceError::Report(
            "journey run ID is required".to_string(),
        ));
    }
    let expected_recipe_digest = recipe_digest(recipe);
    let mut reports = Vec::with_capacity(PRODUCER_SURFACES.len());
    let mut source_commit = None;
    let mut expected_stable_provenance = None;
    for surface in PRODUCER_SURFACES {
        let path = safe_regular_file(
            root,
            Path::new(&format!("{surface}/journey.json")),
            &format!("{surface} journey report"),
        )?;
        let report: JourneyEvidenceReport =
            serde_json::from_slice(&fs::read(&path).map_err(io_error)?).map_err(|error| {
                EquivalenceError::Report(format!("{surface} report is malformed: {error}"))
            })?;
        validate_report(&report, surface, run_id, &expected_recipe_digest)?;
        if let Some(expected) = &source_commit {
            if expected != &report.source.commit {
                return Err(EquivalenceError::Report(
                    "journey reports were produced from different application commits".to_string(),
                ));
            }
        } else {
            source_commit = Some(report.source.commit.clone());
        }
        let report_provenance = stable_provenance(&report);
        if let Some(expected) = &expected_stable_provenance {
            if expected != &report_provenance {
                return Err(EquivalenceError::Report(
                    "journey reports have inconsistent schema, worker, kernel, or host runtime identities"
                        .to_string(),
                ));
            }
        } else {
            expected_stable_provenance = Some(report_provenance);
        }
        validate_artifact(root, &report)?;
        reports.push(report);
    }
    Ok(reports)
}

fn validate_report(
    report: &JourneyEvidenceReport,
    expected_surface: &str,
    run_id: &str,
    expected_recipe_digest: &str,
) -> Result<(), EquivalenceError> {
    if report.schema_version != JOURNEY_EVIDENCE_REPORT_SCHEMA_VERSION
        || report.producer_surface != expected_surface
        || report.run_id != run_id
        || report.result != "passed"
        || !is_hex_identity(&report.source.commit, &[40, 64])
        || report.source.dirty
        || report.recipe.schema_version.is_empty()
        || report.recipe.units != "mm"
        || report.recipe.digest != expected_recipe_digest
    {
        return Err(EquivalenceError::Report(format!(
            "{expected_surface} journey report identity is incomplete or stale"
        )));
    }
    let expected_test = JOURNEY_TESTS
        .iter()
        .find(|(surface, _)| *surface == expected_surface)
        .map(|(_, test)| *test)
        .expect("every expected surface has a test");
    if report.test != expected_test {
        return Err(EquivalenceError::Report(format!(
            "{expected_surface} report names test {} instead of {expected_test}",
            report.test
        )));
    }
    if report.recipe.schema_version != "threeterm.recipe.bracket-complete/1"
        || !report.schemas.is_object()
        || !has_nonempty_string(&report.schemas, "command_registry")
        || !has_nonempty_string(&report.schemas, "feature_schema")
        || !has_nonempty_string(&report.schemas, "protocol_schema")
        || !has_nonempty_string(&report.schemas, "project_manifest_schema_version")
        || !report.workers.is_object()
        || !report.workers["project_manifest"].is_object()
        || !report.workers["occt_fingerprint"].is_object()
        || !report.runtime.is_object()
        || report.runtime["producer_surface"] != expected_surface
        || !has_nonempty_strings(
            &report.workers["project_manifest"],
            &[
                "schema_version",
                "revision_hash",
                "command_registry_hash",
                "feature_schema_version",
                "protocol_schema_version",
                "occt_kernel_version",
            ],
        )
        || !has_nonempty_strings(
            &report.workers["occt_fingerprint"],
            &[
                "worker_kind",
                "worker_schema_version",
                "protocol_schema_version",
            ],
        )
        || !has_nonempty_strings(&report.runtime, &["os", "arch", "adapter"])
        || !report.evidence.is_object()
        || !is_hex_identity(&report.artifact.revision_snapshot_hash, &[64])
        || !provenance_matches(report)
    {
        return Err(EquivalenceError::Report(format!(
            "{expected_surface} report is missing provenance or evidence"
        )));
    }
    match expected_surface {
        API_SURFACE => require_command_evidence(&report.evidence)?,
        MCP_SURFACE => {
            if report.evidence["restart"] != true
                || !report.evidence["protocol"].is_array()
                || report.evidence["protocol_errors"] != json!([])
                || report.evidence["domain_errors"] != json!([])
            {
                return Err(EquivalenceError::Report(
                    "MCP report lacks successful restart and correlated protocol evidence"
                        .to_string(),
                ));
            }
        }
        TUI_SURFACE => {
            if report.evidence["manifest"]["result"] != "passed"
                || !report.evidence["transcript"].is_array()
            {
                return Err(EquivalenceError::Report(
                    "TUI report lacks a passed manifest and retained transcript".to_string(),
                ));
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn has_nonempty_string(value: &Value, field: &str) -> bool {
    value[field].as_str().is_some_and(|value| !value.is_empty())
}

fn has_nonempty_strings(value: &Value, fields: &[&str]) -> bool {
    fields.iter().all(|field| has_nonempty_string(value, field))
}

fn is_hex_identity(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn provenance_matches(report: &JourneyEvidenceReport) -> bool {
    let manifest = &report.workers["project_manifest"];
    let worker = &report.workers["occt_fingerprint"];
    report.artifact.revision_snapshot_hash == manifest["revision_hash"]
        && report.schemas["command_registry"] == manifest["command_registry_hash"]
        && report.schemas["feature_schema"] == manifest["feature_schema_version"]
        && report.schemas["protocol_schema"] == manifest["protocol_schema_version"]
        && report.schemas["project_manifest_schema_version"] == manifest["schema_version"]
        && worker["worker_kind"] == manifest["occt_worker"]["worker_kind"]
        && worker["worker_schema_version"] == manifest["occt_worker"]["worker_schema_version"]
        && worker["protocol_schema_version"] == manifest["occt_worker"]["protocol_schema_version"]
}

fn stable_provenance(report: &JourneyEvidenceReport) -> Value {
    // Each producer has an independent Project Generation, so its Revision
    // Snapshot hash is validated against its own manifest but is not a
    // cross-surface identity key.
    let manifest = &report.workers["project_manifest"];
    json!({
        "schemas": {
            "command_registry": report.schemas["command_registry"],
            "feature_schema": report.schemas["feature_schema"],
            "protocol_schema": report.schemas["protocol_schema"],
            "project_manifest_schema_version": report.schemas["project_manifest_schema_version"],
        },
        "occt_fingerprint": report.workers["occt_fingerprint"],
        "occt_kernel_version": manifest["occt_kernel_version"],
        "runtime": {
            "os": report.runtime["os"],
            "arch": report.runtime["arch"],
        },
    })
}

fn require_command_evidence(evidence: &Value) -> Result<(), EquivalenceError> {
    let executions = evidence["executions"].as_array().ok_or_else(|| {
        EquivalenceError::Report("API report lacks execution evidence".to_string())
    })?;
    for command in REQUIRED_JOURNEY_COMMANDS {
        if !executions
            .iter()
            .any(|entry| entry["command"] == command && entry["outcome"] == "ok")
        {
            return Err(EquivalenceError::Report(format!(
                "API report lacks successful execution evidence for {command}"
            )));
        }
    }
    Ok(())
}

fn validate_artifact(root: &Path, report: &JourneyEvidenceReport) -> Result<(), EquivalenceError> {
    let expected_path = format!("{}/complete-bracket.stl", report.producer_surface);
    if report.artifact.path != expected_path {
        return Err(EquivalenceError::Report(format!(
            "{} artifact path is not the canonical retained STL",
            report.producer_surface
        )));
    }
    let path = safe_regular_file(
        root,
        Path::new(&report.artifact.path),
        &format!("{} retained STL", report.producer_surface),
    )?;
    let bytes = fs::read(&path).map_err(io_error)?;
    if report.artifact.bytes != bytes.len() as u64
        || report.artifact.sha256 != sha256_hex(&bytes)
        || report.artifact.integrity.is_none()
    {
        return Err(EquivalenceError::Report(format!(
            "{} retained STL digest or integrity record does not match",
            report.producer_surface
        )));
    }
    Ok(())
}

fn ensure_directory_tree(path: &Path, label: &str) -> Result<(), EquivalenceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(EquivalenceError::Report(format!(
                    "{label} directory contains a symlink or non-directory"
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            ensure_directory_tree(parent, label)?;
            fs::create_dir(path).map_err(io_error)?;
            let metadata = fs::symlink_metadata(path).map_err(io_error)?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(EquivalenceError::Report(format!(
                    "{label} directory was replaced during creation"
                )));
            }
        }
        Err(error) => return Err(io_error(error)),
    }
    Ok(())
}

fn copy_file_exclusive(source: &Path, destination: &Path) -> Result<(), EquivalenceError> {
    let mut created = false;
    let result = (|| {
        let mut source = fs::File::open(source).map_err(io_error)?;
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(io_error)?;
        created = true;
        std::io::copy(&mut source, &mut destination).map_err(io_error)?;
        destination.sync_all().map_err(io_error)
    })();
    if result.is_err() && created {
        let _ = fs::remove_file(destination);
    }
    result
}

fn safe_regular_file(
    root: &Path,
    relative: &Path,
    label: &str,
) -> Result<PathBuf, EquivalenceError> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(EquivalenceError::Report(format!(
            "{label} path escapes the evidence root"
        )));
    }

    let root_metadata = fs::symlink_metadata(root).map_err(|error| {
        EquivalenceError::Report(format!("evidence root is unavailable: {error}"))
    })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Err(EquivalenceError::Report(
            "evidence root is not a regular directory".to_string(),
        ));
    }

    let mut path = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        if let Component::CurDir = component {
            continue;
        }
        let Component::Normal(name) = component else {
            return Err(EquivalenceError::Report(format!(
                "{label} path is not a safe relative path"
            )));
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            EquivalenceError::Report(format!("{label} is unavailable: {error}"))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(EquivalenceError::Report(format!(
                "{label} path contains a symlink"
            )));
        }
        if components.peek().is_some() {
            if !metadata.file_type().is_dir() {
                return Err(EquivalenceError::Report(format!(
                    "{label} path has a non-directory parent"
                )));
            }
        } else if !metadata.file_type().is_file() {
            return Err(EquivalenceError::Report(format!(
                "{label} is not a regular file"
            )));
        }
    }
    Ok(path)
}

fn tolerance_policy(recipe: &Value) -> Result<TolerancePolicy, EquivalenceError> {
    let tolerances = &recipe["frozen"]["tolerances"];
    Ok(TolerancePolicy {
        linear_mm: tolerances["linear_mm"]
            .as_f64()
            .ok_or_else(|| EquivalenceError::Report("linear tolerance is missing".to_string()))?,
        placement_mm: tolerances["placement_mm"].as_f64().ok_or_else(|| {
            EquivalenceError::Report("placement tolerance is missing".to_string())
        })?,
        volume_fraction: tolerances["volume_fraction"]
            .as_f64()
            .ok_or_else(|| EquivalenceError::Report("volume tolerance is missing".to_string()))?,
    })
}

fn signature_from_mesh(
    recipe: &Value,
    integrity: &StlIntegrityReport,
    mesh: &StlMeshObservation,
    surface: &str,
) -> Result<GeometrySignature, EquivalenceError> {
    let equivalence = recipe["frozen"]["equivalence"].as_object().ok_or_else(|| {
        EquivalenceError::Report("recipe lacks frozen equivalence probes".to_string())
    })?;
    let tolerances = tolerance_policy(recipe)?;
    let mut voids = Vec::new();
    for probe in equivalence["void_probes"]
        .as_array()
        .ok_or_else(|| EquivalenceError::Report("recipe lacks frozen void probes".to_string()))?
    {
        voids.push(probe_value(
            mesh,
            probe,
            false,
            surface,
            "void",
            tolerances.linear_mm,
        )?);
    }
    let mut landmarks = Vec::new();
    for probe in equivalence["landmark_probes"].as_array().ok_or_else(|| {
        EquivalenceError::Report("recipe lacks frozen landmark probes".to_string())
    })? {
        landmarks.push(probe_value(
            mesh,
            probe,
            true,
            surface,
            "landmark",
            tolerances.placement_mm,
        )?);
    }
    let mut surface_samples = Vec::new();
    for sample in equivalence["surface_samples"].as_array().ok_or_else(|| {
        EquivalenceError::Report("recipe lacks frozen surface samples".to_string())
    })? {
        let name = sample["name"]
            .as_str()
            .ok_or_else(|| EquivalenceError::Report("surface sample lacks a name".to_string()))?;
        let x = sample["x"]
            .as_f64()
            .ok_or_else(|| EquivalenceError::Report(format!("surface sample {name} lacks x")))?;
        let y = sample["y"]
            .as_f64()
            .ok_or_else(|| EquivalenceError::Report(format!("surface sample {name} lacks y")))?;
        surface_samples.push(SurfaceSample {
            name: name.to_string(),
            intersections: vertical_surface_intersections(mesh, x, y, tolerances.linear_mm),
        });
    }
    let dimensions = [
        mesh.bounds_max[0] - mesh.bounds_min[0],
        mesh.bounds_max[1] - mesh.bounds_min[1],
        mesh.bounds_max[2] - mesh.bounds_min[2],
    ];
    Ok(GeometrySignature {
        dimensions,
        material_volume: integrity.material_volume,
        topology: TopologySignature {
            shell_count: integrity.shell_count,
            material_shell_count: integrity
                .shell_count
                .saturating_sub(integrity.cavity_shell_count),
            cavity_shell_count: integrity.cavity_shell_count,
        },
        voids,
        landmarks,
        surface_samples,
    })
}

fn probe_value(
    mesh: &StlMeshObservation,
    probe: &Value,
    expected: bool,
    surface: &str,
    category: &str,
    linear_tolerance: f64,
) -> Result<ProbeSample, EquivalenceError> {
    let name = probe["name"]
        .as_str()
        .ok_or_else(|| EquivalenceError::Report(format!("{category} probe lacks a name")))?;
    let point: [f64; 3] = serde_json::from_value(probe["point"].clone()).map_err(|error| {
        EquivalenceError::Report(format!(
            "{category} probe {name} has invalid point: {error}"
        ))
    })?;
    let occupied = point_inside_mesh(mesh, point);
    if probe["occupied"].as_bool() != Some(expected) || occupied != expected {
        return Err(EquivalenceError::IndependentExpectation {
            producer_surface: surface.to_string(),
            detail: format!("{category} probe {name} expected occupied={expected}, got {occupied}"),
        });
    }
    Ok(ProbeSample {
        name: name.to_string(),
        occupied,
        intersections: vertical_surface_intersections(mesh, point[0], point[1], linear_tolerance),
    })
}

fn vertical_surface_intersections(
    mesh: &StlMeshObservation,
    x: f64,
    y: f64,
    tolerance: f64,
) -> Vec<f64> {
    let origin_z = mesh.bounds_min[2] - 1.0;
    let mut intersections = Vec::new();
    for facet in &mesh.facets {
        let [a, b, c] = facet.vertices;
        let edge1 = subtract(b, a);
        let edge2 = subtract(c, a);
        let direction = [0.0, 0.0, 1.0];
        let pvec = cross(direction, edge2);
        let determinant = dot(edge1, pvec);
        if determinant.abs() <= 1.0e-12 {
            continue;
        }
        let inverse = 1.0 / determinant;
        let tvec = [x, y, origin_z];
        let tvec = subtract(tvec, a);
        let u = dot(tvec, pvec) * inverse;
        let qvec = cross(tvec, edge1);
        let v = dot(direction, qvec) * inverse;
        let t = dot(edge2, qvec) * inverse;
        if t <= 0.0 || u <= tolerance * 1.0e-2 || v <= tolerance * 1.0e-2 {
            continue;
        }
        let third = 1.0 - u - v;
        if third <= tolerance * 1.0e-2 {
            continue;
        }
        intersections.push(origin_z + t);
    }
    intersections.sort_by(f64::total_cmp);
    intersections.dedup_by(|left, right| (*left - *right).abs() <= tolerance);
    intersections
}

fn validate_producer_surface(surface: &str) -> Result<(), EquivalenceError> {
    PRODUCER_SURFACES
        .contains(&surface)
        .then_some(())
        .ok_or_else(|| EquivalenceError::Report(format!("unknown producer surface {surface}")))
}

fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<(), EquivalenceError> {
    let parent = path
        .parent()
        .ok_or_else(|| EquivalenceError::Io(format!("path has no parent: {}", path.display())))?;
    ensure_directory_tree(parent, "evidence")?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| EquivalenceError::Report(error.to_string()))?;
    let mut created = false;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        created = true;
        file.write_all(&bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::hard_link(&temporary, path).map_err(io_error)?;
        fs::remove_file(&temporary).map_err(io_error)
    })();
    if result.is_err() && created {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn io_error(error: std::io::Error) -> EquivalenceError {
    EquivalenceError::Io(error.to_string())
}

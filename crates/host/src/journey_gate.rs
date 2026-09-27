//! Aggregate acceptance evidence for the API, MCP, and graphical TUI journeys.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bracket_equivalence::{
    AggregateReport, ArtifactRecord, GeometrySignature, compare_three_reports, recipe_digest,
    write_aggregate_failure, write_aggregate_report,
};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::coverage::{
    JOURNEY_REPORT_SCHEMA_VERSION, SourceIdentity, all_surfaces_tool_coverage_matrix,
};

pub const RUN_MANIFEST_SCHEMA_VERSION: &str = "threeterm.acceptance.run/1";
pub const ATTEMPT_SCHEMA_VERSION: &str = "threeterm.acceptance.attempt/1";
pub const CATALOG_SCHEMA_VERSION: &str = "threeterm.acceptance.three-journey/1";

pub const API_SURFACE: &str = "api";
pub const MCP_SURFACE: &str = "mcp";
pub const TUI_SURFACE: &str = "tui";
pub const SURFACES: [&str; 3] = [API_SURFACE, MCP_SURFACE, TUI_SURFACE];

pub const API_TEST: &str = "e2e_stl_api_all_tools_l_bracket";
pub const MCP_TEST: &str = "e2e_stl_mcp_all_tools_l_bracket";
pub const TUI_TEST: &str = "production_tui_all_tools_stl_journey";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecipeBinding {
    pub schema_version: String,
    pub canonical_json_sha256: String,
    pub file_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct JourneyBinding {
    pub surface: String,
    pub test: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunManifest {
    pub schema_version: String,
    pub run_id: String,
    pub source: SourceIdentity,
    pub recipe: RecipeBinding,
    pub journeys: Vec<JourneyBinding>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EvidenceFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PrerequisiteResult {
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AttemptRecord {
    pub schema_version: String,
    pub run_id: String,
    pub surface: String,
    pub test: String,
    pub status: String,
    pub command: String,
    pub exit_status: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: Option<u64>,
    pub prerequisite: PrerequisiteResult,
    pub stdout: EvidenceFile,
    pub stderr: EvidenceFile,
    pub failure: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CoverageEvidence {
    pub run_id: Option<String>,
    pub result: String,
    pub recipe_schema_version: String,
    pub source: Option<SourceIdentity>,
    pub required_commands: Vec<String>,
    pub command_inventory: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct AggregatePaths {
    pub manifest: PathBuf,
    pub attempts: PathBuf,
    pub coverage: PathBuf,
    pub evidence: PathBuf,
    pub recipe: PathBuf,
    pub run_id: String,
    pub catalog: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogJourney {
    pub surface: String,
    pub test: String,
    pub status: String,
    pub command: String,
    pub exit_status: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: Option<u64>,
    pub prerequisite: PrerequisiteResult,
    pub stdout: EvidenceFile,
    pub stderr: EvidenceFile,
    pub failure: Option<String>,
    pub export: Option<ArtifactRecord>,
    pub mesh_metrics: Option<GeometrySignature>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogCoverage {
    pub run_id: Option<String>,
    pub result: String,
    pub required_commands: Vec<String>,
    pub command_inventory: Vec<Value>,
    pub command_inventory_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregateCatalog {
    pub schema_version: &'static str,
    pub result: String,
    pub run: RunManifest,
    pub journeys: Vec<CatalogJourney>,
    pub coverage: CatalogCoverage,
    pub geometric_equivalence: Option<AggregateReport>,
    pub errors: Vec<String>,
}

pub fn expected_test(surface: &str) -> Option<&'static str> {
    match surface {
        API_SURFACE => Some(API_TEST),
        MCP_SURFACE => Some(MCP_TEST),
        TUI_SURFACE => Some(TUI_TEST),
        _ => None,
    }
}

pub fn build_catalog(
    manifest: RunManifest,
    attempts: Vec<AttemptRecord>,
    coverage: Option<CoverageEvidence>,
    equivalence: Option<AggregateReport>,
    mut errors: Vec<String>,
) -> AggregateCatalog {
    validate_manifest(&manifest, &mut errors);
    let mut surfaces = std::collections::BTreeSet::new();
    for attempt in &attempts {
        if !surfaces.insert(attempt.surface.clone()) {
            errors.push(format!("{}: duplicate attempt record", attempt.surface));
        }
        if expected_test(&attempt.surface).is_none() {
            errors.push(format!("{}: unknown journey surface", attempt.surface));
        }
    }
    let mut journeys = Vec::with_capacity(SURFACES.len());
    for surface in SURFACES {
        let attempt = attempts.iter().find(|attempt| attempt.surface == surface);
        if attempt.is_none() {
            errors.push(format!("{surface}: attempt record is missing"));
        }
        let attempt = attempt
            .cloned()
            .unwrap_or_else(|| missing_attempt(&manifest, surface));
        let (export, mesh_metrics) = equivalence
            .as_ref()
            .and_then(|report| {
                report
                    .producer_surfaces
                    .iter()
                    .find(|producer| producer.producer_surface == surface)
            })
            .map(|producer| {
                (
                    Some(producer.artifact.clone()),
                    Some(producer.geometry.clone()),
                )
            })
            .unwrap_or((None, None));
        journeys.push(CatalogJourney {
            surface: surface.to_string(),
            test: attempt.test.clone(),
            status: attempt.status.clone(),
            command: attempt.command.clone(),
            exit_status: attempt.exit_status,
            timed_out: attempt.timed_out,
            duration_ms: attempt.duration_ms,
            prerequisite: attempt.prerequisite.clone(),
            stdout: attempt.stdout.clone(),
            stderr: attempt.stderr.clone(),
            failure: attempt.failure.clone(),
            export,
            mesh_metrics,
        });
        validate_attempt(&manifest, &attempt, &mut errors);
    }

    let coverage_value = coverage.as_ref();
    let mut inventory = coverage_value
        .map(|matrix| matrix.command_inventory.clone())
        .unwrap_or_default();
    inventory.sort_by_key(|cell| {
        (
            cell.get("surface")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            cell.get("command_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    });
    let inventory_digest = sha256_json(&inventory);
    let required_commands = coverage_value
        .map(|matrix| matrix.required_commands.clone())
        .unwrap_or_default();
    if coverage_value.is_none() {
        errors.push("coverage matrix is missing".to_string());
    } else if let Some(matrix) = coverage_value {
        if matrix.result != "passed" {
            errors.push("coverage matrix did not pass".to_string());
        }
        if matrix.required_commands.is_empty() || matrix.command_inventory.is_empty() {
            errors.push("coverage matrix command inventory is incomplete".to_string());
        }
        if matrix.run_id.as_deref() != Some(manifest.run_id.as_str()) {
            errors.push("coverage evidence run ID does not match the run manifest".to_string());
        }
        if matrix
            .command_inventory
            .iter()
            .any(|cell| cell.get("status").and_then(Value::as_str) != Some("passed"))
        {
            errors.push("coverage matrix contains a non-passing command cell".to_string());
        }
    }
    if let Some(report) = &equivalence {
        if report.schema_version != "threeterm.evidence.bracket-equivalence/1" {
            errors.push("geometric evidence schema is invalid".to_string());
        }
        if report.run_id != manifest.run_id {
            errors.push("geometric evidence run ID does not match the run manifest".to_string());
        }
        if report.source.commit != manifest.source.commit
            || report.source.dirty != manifest.source.dirty
        {
            errors.push("geometric evidence source does not match the run manifest".to_string());
        }
        if report.recipe.schema_version != manifest.recipe.schema_version
            || report.recipe.digest != manifest.recipe.canonical_json_sha256
        {
            errors.push("geometric evidence recipe does not match the run manifest".to_string());
        }
        let producer_surfaces = report
            .producer_surfaces
            .iter()
            .map(|producer| producer.producer_surface.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if report.producer_surfaces.len() != SURFACES.len()
            || producer_surfaces.len() != SURFACES.len()
            || !SURFACES
                .iter()
                .all(|surface| producer_surfaces.contains(surface))
        {
            errors.push("geometric evidence does not contain all three surfaces".to_string());
        }
    } else {
        errors.push("geometric equivalence evidence is missing".to_string());
    }
    if let Some(matrix) = coverage_value
        && matrix.source.as_ref() != Some(&manifest.source)
    {
        errors.push("coverage evidence source does not match the run manifest".to_string());
    }
    if let Some(matrix) = coverage_value
        && matrix.recipe_schema_version != manifest.recipe.schema_version
    {
        errors.push("coverage evidence recipe does not match the run manifest".to_string());
    }

    errors.sort();
    errors.dedup();
    AggregateCatalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        result: if errors.is_empty() {
            "passed"
        } else {
            "failed"
        }
        .to_string(),
        run: manifest,
        journeys,
        coverage: CatalogCoverage {
            run_id: coverage_value.and_then(|matrix| matrix.run_id.clone()),
            result: coverage_value
                .map(|matrix| matrix.result.clone())
                .unwrap_or_else(|| "missing".to_string()),
            required_commands,
            command_inventory: inventory,
            command_inventory_sha256: inventory_digest,
        },
        geometric_equivalence: equivalence,
        errors,
    }
}

pub fn aggregate_files(paths: &AggregatePaths) -> Result<AggregateCatalog, String> {
    if !is_safe_run_id(&paths.run_id) {
        return Err("run ID is not a safe path component".to_string());
    }
    let mut errors = Vec::new();
    let manifest = read_manifest(&paths.manifest, &paths.run_id, &mut errors);
    if manifest.run_id != paths.run_id {
        errors.push("run manifest ID does not match the aggregate input".to_string());
    }
    let attempts = read_attempts(&paths.attempts, &mut errors);
    let run_root = paths.attempts.parent().unwrap_or_else(|| Path::new("."));
    for attempt in &attempts {
        validate_retained_attempt_evidence(run_root, attempt, &mut errors);
    }
    let recipe_bytes = fs::read(&paths.recipe).map_err(|error| {
        errors.push(format!("recipe: {error}"));
        error.to_string()
    });
    let recipe = recipe_bytes
        .as_ref()
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
    if let Some(recipe) = recipe.as_ref() {
        let canonical = recipe_digest(recipe);
        let raw = sha256_hex(recipe_bytes.as_ref().unwrap());
        if manifest.recipe.canonical_json_sha256 != canonical {
            errors
                .push("run manifest canonical recipe digest does not match the recipe".to_string());
        }
        if manifest.recipe.file_sha256 != raw {
            errors.push("run manifest recipe file digest does not match the recipe".to_string());
        }
        if recipe.get("schema_version").and_then(Value::as_str)
            != Some(manifest.recipe.schema_version.as_str())
        {
            errors.push("run manifest recipe schema does not match the recipe".to_string());
        }
    } else {
        errors.push("recipe: malformed JSON".to_string());
    }

    if let Err(error) = all_surfaces_tool_coverage_matrix(&paths.coverage) {
        errors.push(format!("coverage evaluation: {error}"));
    }
    let coverage_run_id = read_coverage_bindings(
        &paths.coverage,
        &paths.run_id,
        &manifest.source,
        &manifest.recipe.schema_version,
        &mut errors,
    );
    let coverage = match read_coverage(&paths.coverage.join("journey-coverage-matrix.json")) {
        Ok(mut coverage) => {
            coverage.run_id = coverage_run_id;
            Some(coverage)
        }
        Err(error) => {
            errors.push(error);
            None
        }
    };
    let equivalence = if let Some(recipe) = recipe.as_ref() {
        match compare_three_reports(&paths.evidence, &paths.run_id, recipe) {
            Ok(report) => {
                if let Err(error) = write_aggregate_report(&paths.evidence, &report) {
                    errors.push(format!("aggregate report: {error}"));
                }
                Some(report)
            }
            Err(error) => {
                let detail = error.to_string();
                errors.push(format!("geometric equivalence: {detail}"));
                if let Err(write_error) =
                    write_aggregate_failure(&paths.evidence, &paths.run_id, &detail)
                {
                    errors.push(format!("aggregate failure report: {write_error}"));
                }
                None
            }
        }
    } else {
        None
    };
    let catalog = build_catalog(manifest, attempts, coverage, equivalence, errors);
    write_catalog(&paths.catalog, &catalog)?;
    if catalog.result == "failed" {
        write_failure_report(&paths.evidence, &paths.run_id, &catalog.errors)?;
    }
    Ok(catalog)
}

pub fn write_catalog(path: &Path, catalog: &AggregateCatalog) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("catalog path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(catalog).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn write_failure_report(root: &Path, run_id: &str, errors: &[String]) -> Result<(), String> {
    let parent = root.join(run_id);
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let path = parent.join("aggregate-failure.json");
    let temporary = parent.join(format!(
        ".aggregate-failure.json.tmp-{}",
        std::process::id()
    ));
    let value = serde_json::json!({
        "schema_version": "threeterm.evidence.bracket-equivalence/1",
        "run_id": run_id,
        "result": "failed",
        "errors": errors,
    });
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn read_manifest(path: &Path, run_id: &str, errors: &mut Vec<String>) -> RunManifest {
    match fs::read(path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
    {
        Ok(manifest) => manifest,
        Err(error) => {
            errors.push(format!("run manifest: {error}"));
            RunManifest {
                schema_version: String::new(),
                run_id: run_id.to_string(),
                source: SourceIdentity {
                    commit: String::new(),
                    dirty: true,
                },
                recipe: RecipeBinding {
                    schema_version: String::new(),
                    canonical_json_sha256: String::new(),
                    file_sha256: String::new(),
                },
                journeys: Vec::new(),
            }
        }
    }
}

fn read_attempts(root: &Path, errors: &mut Vec<String>) -> Vec<AttemptRecord> {
    SURFACES
        .iter()
        .filter_map(|surface| {
            let path = root.join(format!("{surface}.json"));
            match fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(attempt) => Some(attempt),
                    Err(error) => {
                        errors.push(format!("{surface}: malformed attempt: {error}"));
                        None
                    }
                },
                Err(error) => {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        errors.push(format!("{surface}: attempt: {error}"));
                    }
                    None
                }
            }
        })
        .collect()
}

fn read_coverage_bindings(
    root: &Path,
    expected_run_id: &str,
    expected_source: &SourceIdentity,
    expected_recipe_schema: &str,
    errors: &mut Vec<String>,
) -> Option<String> {
    let mut run_ids = std::collections::BTreeSet::new();
    for surface in SURFACES {
        let path = root.join(format!("{surface}-journey-coverage.binding.json"));
        let binding: Value = match fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        {
            Ok(binding) => binding,
            Err(error) => {
                errors.push(format!("coverage {surface} binding: {error}"));
                continue;
            }
        };
        let run_id = binding["run_id"].as_str().unwrap_or_default();
        let binding_surface = binding["surface"].as_str().unwrap_or_default();
        if binding["schema_version"] != "threeterm.acceptance.coverage-binding/1"
            || run_id != expected_run_id
            || binding_surface != surface
        {
            errors.push(format!("coverage {surface} binding identity is invalid"));
            continue;
        }
        let report: EvidenceFile = match serde_json::from_value(binding["report"].clone()) {
            Ok(report) => report,
            Err(error) => {
                errors.push(format!(
                    "coverage {surface} binding report is invalid: {error}"
                ));
                continue;
            }
        };
        if report.path != format!("{surface}-journey-coverage.json") {
            errors.push(format!("coverage {surface} binding report path is invalid"));
            continue;
        }
        if let Err(error) = validate_evidence_file(root, &report) {
            errors.push(format!("coverage {surface} binding report: {error}"));
            continue;
        }
        let report_value: Value = match fs::read(root.join(&report.path))
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        {
            Ok(report) => report,
            Err(error) => {
                errors.push(format!("coverage {surface} journey report: {error}"));
                continue;
            }
        };
        let source = report_value
            .get("source")
            .cloned()
            .and_then(|source| serde_json::from_value::<SourceIdentity>(source).ok());
        if report_value["schema_version"] != JOURNEY_REPORT_SCHEMA_VERSION
            || report_value["surface"] != surface
            || report_value["test"] != expected_test(surface).unwrap_or_default()
            || report_value["result"] != "passed"
            || source.as_ref() != Some(expected_source)
            || report_value["recipe_schema_version"] != expected_recipe_schema
        {
            errors.push(format!(
                "coverage {surface} journey report identity is invalid"
            ));
            continue;
        }
        run_ids.insert(run_id.to_string());
    }
    (run_ids.len() == 1 && run_ids.contains(expected_run_id)).then(|| expected_run_id.to_string())
}

fn read_coverage(path: &Path) -> Result<CoverageEvidence, String> {
    let value: Value = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("coverage matrix: {error}"))?,
    )
    .map_err(|error| format!("coverage matrix: {error}"))?;
    let source = value
        .get("source")
        .filter(|source| !source.is_null())
        .map(|source| serde_json::from_value(source.clone()))
        .transpose()
        .map_err(|error| format!("coverage source: {error}"))?;
    let required_commands = value["required_commands"]
        .as_array()
        .ok_or_else(|| "coverage matrix has no required command inventory".to_string())?
        .iter()
        .map(|command| {
            command
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "coverage command inventory contains a non-string".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let command_inventory = value["cells"]
        .as_array()
        .ok_or_else(|| "coverage matrix has no cells".to_string())?
        .iter()
        .map(|cell| {
            let object = cell
                .as_object()
                .ok_or_else(|| "coverage cell is not an object".to_string())?;
            Ok(serde_json::json!({
                "surface": object.get("surface"),
                "command_id": object.get("command_id"),
                "command_name": object.get("command_name"),
                "command_schema_version": object.get("command_schema_version"),
                "request_schema_version": object.get("request_schema_version"),
                "response_schema_version": object.get("response_schema_version"),
                "request_schema_hash": object.get("request_schema_hash"),
                "response_schema_hash": object.get("response_schema_hash"),
                "status": object.get("status"),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(CoverageEvidence {
        run_id: None,
        result: value["result"].as_str().unwrap_or("missing").to_string(),
        recipe_schema_version: value["recipe_schema_version"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        source,
        required_commands,
        command_inventory,
    })
}

fn validate_manifest(manifest: &RunManifest, errors: &mut Vec<String>) {
    if manifest.schema_version != RUN_MANIFEST_SCHEMA_VERSION {
        errors.push("run manifest schema is invalid".to_string());
    }
    if manifest.run_id.is_empty() {
        errors.push("run manifest has no run ID".to_string());
    }
    if !is_lower_hex(&manifest.source.commit, 40) {
        errors.push("run manifest source commit is invalid".to_string());
    }
    if manifest.recipe.schema_version.is_empty()
        || !is_lower_hex(&manifest.recipe.canonical_json_sha256, 64)
        || !is_lower_hex(&manifest.recipe.file_sha256, 64)
    {
        errors.push("run manifest recipe binding is incomplete".to_string());
    }
    let journeys = manifest
        .journeys
        .iter()
        .map(|journey| (journey.surface.as_str(), journey.test.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    if manifest.journeys.len() != SURFACES.len()
        || journeys.len() != SURFACES.len()
        || !SURFACES.iter().all(|surface| {
            expected_test(surface)
                .map(|test| journeys.contains(&(*surface, test)))
                .unwrap_or(false)
        })
    {
        errors.push("run manifest journey bindings are invalid".to_string());
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_run_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_retained_attempt_evidence(
    run_root: &Path,
    attempt: &AttemptRecord,
    errors: &mut Vec<String>,
) {
    for (name, evidence) in [("stdout", &attempt.stdout), ("stderr", &attempt.stderr)] {
        if evidence.path.is_empty() {
            continue;
        }
        if let Err(error) = validate_evidence_file(run_root, evidence) {
            errors.push(format!(
                "{}: retained {name} evidence: {error}",
                attempt.surface
            ));
        }
    }
}

fn validate_evidence_file(root: &Path, evidence: &EvidenceFile) -> Result<(), String> {
    let relative = Path::new(&evidence.path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err("path escapes the evidence root".to_string());
    }
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("path is not a regular file".to_string());
    }
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let canonical_path = fs::canonicalize(&path).map_err(|error| error.to_string())?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("path resolves outside the evidence root".to_string());
    }
    let bytes = fs::read(canonical_path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 != evidence.bytes || sha256_hex(&bytes) != evidence.sha256 {
        return Err("hash or byte count is invalid".to_string());
    }
    Ok(())
}

fn missing_attempt(manifest: &RunManifest, surface: &str) -> AttemptRecord {
    AttemptRecord {
        schema_version: ATTEMPT_SCHEMA_VERSION.to_string(),
        run_id: manifest.run_id.clone(),
        surface: surface.to_string(),
        test: expected_test(surface).unwrap_or("unknown").to_string(),
        status: "unrun".to_string(),
        command: String::new(),
        exit_status: None,
        timed_out: false,
        duration_ms: None,
        prerequisite: PrerequisiteResult {
            status: "unknown".to_string(),
            reason: "attempt record was not produced".to_string(),
        },
        stdout: empty_evidence_file(),
        stderr: empty_evidence_file(),
        failure: Some("attempt record was not produced".to_string()),
    }
}

fn empty_evidence_file() -> EvidenceFile {
    EvidenceFile {
        path: String::new(),
        bytes: 0,
        sha256: String::new(),
    }
}

fn validate_attempt(manifest: &RunManifest, attempt: &AttemptRecord, errors: &mut Vec<String>) {
    let prefix = attempt.surface.as_str();
    if attempt.schema_version != ATTEMPT_SCHEMA_VERSION {
        errors.push(format!("{prefix}: attempt schema is invalid"));
    }
    if attempt.run_id != manifest.run_id {
        errors.push(format!(
            "{prefix}: attempt run ID does not match the run manifest"
        ));
    }
    if expected_test(prefix) != Some(attempt.test.as_str()) {
        errors.push(format!("{prefix}: attempt test ID is invalid"));
    }
    if attempt.status != "passed"
        || attempt.exit_status != Some(0)
        || attempt.timed_out
        || attempt.duration_ms.is_none()
        || attempt.prerequisite.status != "passed"
        || attempt.command.is_empty()
    {
        errors.push(format!("{prefix}: attempt did not pass all required gates"));
    }
    for evidence in [&attempt.stdout, &attempt.stderr] {
        if evidence.path.is_empty() || evidence.sha256.len() != 64 {
            errors.push(format!("{prefix}: retained process evidence is incomplete"));
        }
    }
}

fn sha256_json<T: Serialize>(value: &T) -> String {
    threeterm_protocol::artifact::sha256_hex(
        &serde_json::to_vec(value).expect("catalog inventory serializes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bracket_equivalence::{
        AggregateReport, PairComparison, ProducerSurfaceSignature, RecipeIdentity,
        SourceIdentity as EvidenceSourceIdentity, TopologySignature,
    };
    use crate::stl_integrity::{StlFormat, VALIDATION_POLICY};
    use threeterm_protocol::coverage::Surface;

    fn manifest() -> RunManifest {
        RunManifest {
            schema_version: RUN_MANIFEST_SCHEMA_VERSION.to_string(),
            run_id: "run-1".to_string(),
            source: SourceIdentity {
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
                dirty: false,
            },
            recipe: RecipeBinding {
                schema_version: "threeterm.recipe.bracket-complete/1".to_string(),
                canonical_json_sha256: "a".repeat(64),
                file_sha256: "b".repeat(64),
            },
            journeys: SURFACES
                .iter()
                .map(|surface| JourneyBinding {
                    surface: (*surface).to_string(),
                    test: expected_test(surface).unwrap().to_string(),
                })
                .collect(),
        }
    }

    fn attempt(surface: &str) -> AttemptRecord {
        AttemptRecord {
            schema_version: ATTEMPT_SCHEMA_VERSION.to_string(),
            run_id: "run-1".to_string(),
            surface: surface.to_string(),
            test: expected_test(surface).unwrap().to_string(),
            status: "passed".to_string(),
            command: format!("cargo test {surface}"),
            exit_status: Some(0),
            timed_out: false,
            duration_ms: Some(42),
            prerequisite: PrerequisiteResult {
                status: "passed".to_string(),
                reason: "qualified".to_string(),
            },
            stdout: EvidenceFile {
                path: format!("attempts/{surface}.stdout"),
                bytes: 1,
                sha256: "c".repeat(64),
            },
            stderr: EvidenceFile {
                path: format!("attempts/{surface}.stderr"),
                bytes: 1,
                sha256: "d".repeat(64),
            },
            failure: None,
        }
    }

    fn coverage() -> CoverageEvidence {
        CoverageEvidence {
            run_id: Some("run-1".to_string()),
            result: "passed".to_string(),
            recipe_schema_version: "threeterm.recipe.bracket-complete/1".to_string(),
            source: Some(manifest().source),
            required_commands: vec!["export".to_string()],
            command_inventory: vec![serde_json::json!({
                "surface": Surface::Api,
                "command_id": "export",
                "command_name": "export",
                "command_schema_version": "export/1",
                "request_schema_version": "request/1",
                "response_schema_version": "response/1",
                "request_schema_hash": "e".repeat(64),
                "response_schema_hash": "f".repeat(64),
                "status": "passed",
            })],
        }
    }

    fn equivalence() -> AggregateReport {
        let artifact = ArtifactRecord {
            path: "api/complete-bracket.stl".to_string(),
            bytes: 4,
            sha256: "1".repeat(64),
            revision_snapshot_hash: "2".repeat(64),
            integrity: Some(crate::stl_integrity::StlIntegrityReport {
                policy: VALIDATION_POLICY,
                format: StlFormat::Binary,
                triangle_count: 1,
                unique_vertex_count: 3,
                shell_count: 1,
                cavity_shell_count: 0,
                shell_signed_volumes: vec![1.0],
                signed_volume: 1.0,
                material_volume: 1.0,
            }),
        };
        let geometry = GeometrySignature {
            dimensions: [1.0, 2.0, 3.0],
            material_volume: 1.0,
            topology: TopologySignature {
                shell_count: 1,
                material_shell_count: 1,
                cavity_shell_count: 0,
            },
            voids: Vec::new(),
            landmarks: Vec::new(),
            surface_samples: Vec::new(),
        };
        AggregateReport {
            schema_version: "threeterm.evidence.bracket-equivalence/1".to_string(),
            run_id: "run-1".to_string(),
            recipe: RecipeIdentity {
                schema_version: "threeterm.recipe.bracket-complete/1".to_string(),
                digest: "a".repeat(64),
                units: "mm".to_string(),
            },
            source: EvidenceSourceIdentity {
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
                dirty: false,
            },
            producer_surfaces: SURFACES
                .iter()
                .map(|surface| ProducerSurfaceSignature {
                    producer_surface: (*surface).to_string(),
                    artifact: ArtifactRecord {
                        path: format!("{surface}/complete-bracket.stl"),
                        ..artifact.clone()
                    },
                    geometry: geometry.clone(),
                })
                .collect(),
            comparisons: vec![
                PairComparison {
                    left_producer_surface: "api".to_string(),
                    right_producer_surface: "mcp".to_string(),
                    result: "passed".to_string(),
                },
                PairComparison {
                    left_producer_surface: "api".to_string(),
                    right_producer_surface: "tui".to_string(),
                    result: "passed".to_string(),
                },
                PairComparison {
                    left_producer_surface: "mcp".to_string(),
                    right_producer_surface: "tui".to_string(),
                    result: "passed".to_string(),
                },
            ],
        }
    }

    #[test]
    fn passing_three_journeys_publish_bound_catalog_data() {
        let catalog = build_catalog(
            manifest(),
            SURFACES.iter().map(|surface| attempt(surface)).collect(),
            Some(coverage()),
            Some(equivalence()),
            Vec::new(),
        );

        assert_eq!(catalog.result, "passed");
        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        assert_eq!(catalog.journeys.len(), 3);
        assert_eq!(catalog.run.recipe.canonical_json_sha256, "a".repeat(64));
        assert_eq!(catalog.coverage.command_inventory_sha256.len(), 64);
        assert!(catalog.journeys.iter().all(|journey| {
            journey.duration_ms == Some(42)
                && journey
                    .export
                    .as_ref()
                    .is_some_and(|artifact| artifact.sha256.len() == 64)
                && journey
                    .mesh_metrics
                    .as_ref()
                    .is_some_and(|mesh| mesh.material_volume > 0.0)
        }));
    }

    #[test]
    fn missing_journey_attempt_fails_the_aggregate() {
        let catalog = build_catalog(
            manifest(),
            vec![attempt(API_SURFACE), attempt(MCP_SURFACE)],
            Some(coverage()),
            Some(equivalence()),
            Vec::new(),
        );

        assert_eq!(catalog.result, "failed");
        assert!(catalog.errors.iter().any(|error| error.contains("tui")));
        assert_eq!(catalog.journeys[2].status, "unrun");
    }

    #[test]
    fn failed_and_prerequisite_skipped_attempts_cannot_pass() {
        let mut failed = attempt(API_SURFACE);
        failed.status = "failed".to_string();
        failed.exit_status = Some(1);
        failed.failure = Some("worker failed".to_string());
        let mut skipped = attempt(TUI_SURFACE);
        skipped.status = "prerequisite_skipped".to_string();
        skipped.prerequisite.status = "skipped".to_string();
        skipped.prerequisite.reason = "qualified runner unavailable".to_string();
        let catalog = build_catalog(
            manifest(),
            vec![failed, attempt(MCP_SURFACE), skipped],
            Some(coverage()),
            Some(equivalence()),
            Vec::new(),
        );

        assert_eq!(catalog.result, "failed");
        assert!(catalog.errors.iter().any(|error| error.contains("api")));
        assert!(catalog.errors.iter().any(|error| error.contains("tui")));
    }

    #[test]
    fn stale_source_or_recipe_evidence_fails_the_aggregate() {
        let mut stale_source = equivalence();
        stale_source.source.commit = "fedcba9876543210fedcba9876543210fedcba98".to_string();
        let source_catalog = build_catalog(
            manifest(),
            SURFACES.iter().map(|surface| attempt(surface)).collect(),
            Some(coverage()),
            Some(stale_source),
            Vec::new(),
        );
        assert_eq!(source_catalog.result, "failed");
        assert!(
            source_catalog
                .errors
                .iter()
                .any(|error| error.contains("source"))
        );

        let mut stale_recipe = equivalence();
        stale_recipe.recipe.digest = "e".repeat(64);
        let recipe_catalog = build_catalog(
            manifest(),
            SURFACES.iter().map(|surface| attempt(surface)).collect(),
            Some(coverage()),
            Some(stale_recipe),
            Vec::new(),
        );
        assert_eq!(recipe_catalog.result, "failed");
        assert!(
            recipe_catalog
                .errors
                .iter()
                .any(|error| error.contains("recipe"))
        );
    }

    #[test]
    fn stale_coverage_binding_fails_the_aggregate() {
        let mut stale_coverage = coverage();
        stale_coverage.run_id = Some("different-run".to_string());
        let catalog = build_catalog(
            manifest(),
            SURFACES.iter().map(|surface| attempt(surface)).collect(),
            Some(stale_coverage),
            Some(equivalence()),
            Vec::new(),
        );

        assert_eq!(catalog.result, "failed");
        assert!(
            catalog
                .errors
                .iter()
                .any(|error| error.contains("coverage evidence run ID"))
        );
    }
}

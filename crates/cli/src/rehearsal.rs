use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use threeterm_domain::ProjectGeneration;
use threeterm_host::{Host, HostError};
use threeterm_persistence::{Bundle, BundleError, write_v0_fixture};
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, BRACKET_EDIT_COMMAND_ID, BRACKET_EDIT_RESPONSE_SCHEMA,
    BRACKET_RESPONSE_SCHEMA, EXPORT_COMMAND_ID, LOAD_COMMAND_ID, NEW_PROJECT_COMMAND_ID,
    NEW_PROJECT_RESPONSE_SCHEMA, REHEARSE_RESPONSE_SCHEMA, REHEARSE_RESPONSE_SCHEMA_VERSION,
    REHEARSE_RUN_RESPONSE_SCHEMA, REHEARSE_RUN_RESPONSE_SCHEMA_VERSION, find,
};
use threeterm_protocol::schema_validator::validate;
use threeterm_tui::TuiViewportSession;
use threeterm_viewport::{
    CapabilityProbe, CapabilityProbeIo, CapabilityState, GhosttyRenderer, TerminalCapabilityVector,
    TerminalEnvironment, ViewportDiagnosticCode, ViewportFrame,
};

const PROJECT_DIR: &str = "project";
const PREVIOUS_PROJECT_DIR: &str = "project.previous-generation";
const EXPORT_DIR: &str = "export";
const CATALOG_FILE: &str = "sha256-manifest.json";
const FIXTURE: &str = "l-bracket";
const DRAFT_ID: &str = "rehearsal-edit";

const TIMING_CLASSES: [&str; 9] = [
    "project_create",
    "bracket_create",
    "edit_open",
    "edit_update",
    "edit_preview",
    "edit_commit",
    "reload",
    "export",
    "catalog",
];

#[derive(Debug)]
pub struct RehearsalError {
    pub stage: String,
    pub detail: Value,
    pub current_revision: Option<String>,
}

impl RehearsalError {
    fn new(stage: &str, detail: Value, project: &Path) -> Self {
        let current_revision = threeterm_persistence::Bundle::at(project)
            .open()
            .ok()
            .map(|bundle| bundle.revision_hash_hex().to_string());
        Self {
            stage: stage.to_string(),
            detail,
            current_revision,
        }
    }

    pub fn diagnostic(&self) -> Value {
        json!({
            "schema_version": threeterm_protocol::schema_version(),
            "code": "rehearsal_failure",
            "stage": self.stage,
            "detail": self.detail,
            "current_revision": self.current_revision,
            "recovery": "canonical state remains at current_revision; inspect the failed stage and retry with a fresh output root"
        })
    }
}

#[derive(Debug, Serialize)]
struct TimingBand {
    class: String,
    unit: &'static str,
    sample_count: usize,
    samples_ms: [f64; 1],
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

#[derive(Debug, Serialize)]
struct Artifact {
    relative_path: String,
    bytes: u64,
    sha256: String,
}

fn elapsed_band(class: &str, started: Instant) -> TimingBand {
    let milliseconds = started.elapsed().as_secs_f64() * 1_000.0;
    TimingBand {
        class: class.to_string(),
        unit: "ms",
        sample_count: 1,
        samples_ms: [milliseconds],
        p50_ms: milliseconds,
        p95_ms: milliseconds,
        p99_ms: milliseconds,
    }
}

/// Run the canonical L-bracket rehearsal twice and write the comparison catalog.
pub fn run_l_bracket_rehearsal(
    output_dir: impl AsRef<Path>,
    release_candidate: &str,
) -> Result<Value, RehearsalError> {
    let output_dir = output_dir.as_ref();
    let project = output_dir.join("run-1").join(PROJECT_DIR);
    preflight_aggregate(output_dir, &project)?;
    let release_candidates = release_candidates(release_candidate);
    let mut runs = Vec::with_capacity(release_candidates.len());
    for (index, candidate) in release_candidates.iter().enumerate() {
        let run_root = output_dir.join(format!("run-{}", index + 1));
        let report =
            run_single_l_bracket_rehearsal(&run_root, candidate).map_err(|mut error| {
                error.stage = format!("run-{}:{}", index + 1, error.stage);
                error
            })?;
        runs.push(prefix_run_report(report, &format!("run-{}", index + 1)));
    }

    let comparisons =
        compare_rehearsal_runs(runs.as_slice(), output_dir.join("run-2").join(PROJECT_DIR))?;
    let report = json!({
        "schema_version": REHEARSE_RESPONSE_SCHEMA_VERSION,
        "release_candidates": release_candidates,
        "fixture": FIXTURE,
        "run_count": 2,
        "sample_policy": "nearest-rank",
        "promoted": false,
        "runs": runs,
        "comparisons": comparisons,
    });
    validate(&REHEARSE_RESPONSE_SCHEMA, &report).map_err(|error| {
        RehearsalError::new(
            "comparison",
            json!({"message": format!("comparison response failed schema validation: {error}")}),
            &output_dir.join("run-2").join(PROJECT_DIR),
        )
    })?;
    write_catalog(output_dir, &report).map_err(|error| {
        RehearsalError::new(
            "comparison",
            json!({"message": error.to_string()}),
            &project,
        )
    })?;
    Ok(report)
}

fn run_single_l_bracket_rehearsal(
    output_dir: &Path,
    release_candidate: &str,
) -> Result<Value, RehearsalError> {
    let project = output_dir.join(PROJECT_DIR);
    preflight(output_dir, &project, release_candidate)?;
    let mut timings = Vec::with_capacity(TIMING_CLASSES.len());

    let started = Instant::now();
    invoke_machine(
        "project_create",
        &project,
        &["--machine", "new-project"],
        &[project.to_string_lossy().as_ref()],
    )?;
    timings.push(elapsed_band("project_create", started));

    let started = Instant::now();
    invoke_machine(
        "bracket_create",
        &project,
        &["--machine", "bracket"],
        &[
            project.to_string_lossy().as_ref(),
            "--bracket-id",
            FIXTURE,
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    )?;
    timings.push(elapsed_band("bracket_create", started));

    let host = Host::new();
    let base_request = |phase: &str| {
        json!({
            "phase": phase,
            "bundle_path": project.to_string_lossy(),
            "draft_id": DRAFT_ID,
            "bracket_id": FIXTURE,
            "length": 65.0,
            "width": 30.0,
            "height": 40.0,
            "thickness": 3.0
        })
    };

    let started = Instant::now();
    let opened = invoke_registered(
        "edit_open",
        &project,
        &host,
        BRACKET_EDIT_COMMAND_ID,
        base_request("open"),
    )?;
    timings.push(elapsed_band("edit_open", started));
    let sequence = opened["draft_sequence"].as_u64().ok_or_else(|| {
        RehearsalError::new(
            "edit_open",
            json!({"message": "edit response omitted draft_sequence"}),
            &project,
        )
    })?;
    let fingerprint = opened["input_fingerprint"].as_str().ok_or_else(|| {
        RehearsalError::new(
            "edit_open",
            json!({"message": "edit response omitted input_fingerprint"}),
            &project,
        )
    })?;

    let mut update = base_request("update");
    update["draft_sequence"] = json!(sequence);
    update["input_fingerprint"] = json!(fingerprint);
    let started = Instant::now();
    invoke_registered(
        "edit_update",
        &project,
        &host,
        BRACKET_EDIT_COMMAND_ID,
        update,
    )?;
    timings.push(elapsed_band("edit_update", started));

    let started = Instant::now();
    invoke_registered(
        "edit_preview",
        &project,
        &host,
        BRACKET_EDIT_COMMAND_ID,
        base_request("preview"),
    )?;
    timings.push(elapsed_band("edit_preview", started));

    let started = Instant::now();
    let committed = invoke_registered(
        "edit_commit",
        &project,
        &host,
        BRACKET_EDIT_COMMAND_ID,
        base_request("commit"),
    )?;
    timings.push(elapsed_band("edit_commit", started));
    let committed_revision = committed["current_revision"].as_str().ok_or_else(|| {
        RehearsalError::new(
            "edit_commit",
            json!({"message": "edit response omitted current_revision"}),
            &project,
        )
    })?;
    let committed_graph = host
        .current()
        .map(|snapshot| snapshot.feature_graph_hash)
        .ok_or_else(|| {
            RehearsalError::new(
                "edit_commit",
                json!({"message": "host did not retain the committed snapshot"}),
                &project,
            )
        })?;

    let started = Instant::now();
    let reloaded = invoke_machine(
        "reload",
        &project,
        &["--machine", "load"],
        &[project.to_string_lossy().as_ref()],
    )?;
    timings.push(elapsed_band("reload", started));
    if reloaded["revision_hash"] != committed_revision {
        return Err(RehearsalError::new(
            "reload",
            json!({
                "message": "reload revision differs from edit commit",
                "committed_revision": committed_revision,
                "reloaded_revision": reloaded["revision_hash"]
            }),
            &project,
        ));
    }
    if reloaded["feature_graph_hash"] != committed_graph {
        return Err(RehearsalError::new(
            "reload",
            json!({"message": "reload feature graph differs from edit commit"}),
            &project,
        ));
    }

    let export = output_dir.join(EXPORT_DIR);
    let started = Instant::now();
    invoke_machine(
        "export",
        &project,
        &["--machine", "export"],
        &[
            "--bundle",
            project.to_string_lossy().as_ref(),
            "--feature-id",
            FIXTURE,
            "--formats",
            "stl,3mf,step",
            "--output-dir",
            export.to_string_lossy().as_ref(),
            "--tessellation-deflection",
            "0.5",
        ],
    )?;
    timings.push(elapsed_band("export", started));

    let started = Instant::now();
    let artifacts = collect_artifacts(&project, &export)?;
    let catalog_timing = elapsed_band("catalog", started);
    timings.push(catalog_timing);
    timings.sort_by_key(|timing| {
        TIMING_CLASSES
            .iter()
            .position(|class| *class == timing.class)
            .unwrap_or(usize::MAX)
    });
    let report = json!({
        "schema_version": REHEARSE_RUN_RESPONSE_SCHEMA_VERSION,
        "release_candidate": release_candidate,
        "project_path": PROJECT_DIR,
        "export_path": EXPORT_DIR,
        "catalog_path": CATALOG_FILE,
        "timings": timings,
        "artifacts": artifacts,
    });
    validate(&REHEARSE_RUN_RESPONSE_SCHEMA, &report).map_err(|error| {
        RehearsalError::new(
            "catalog",
            json!({"message": format!("catalog response failed schema validation: {error}")}),
            &project,
        )
    })?;
    write_catalog(output_dir, &report).map_err(|error| {
        RehearsalError::new("catalog", json!({"message": error.to_string()}), &project)
    })?;
    Ok(report)
}

fn release_candidates(release_candidate: &str) -> [String; 2] {
    let second = release_candidate.strip_suffix('1').map_or_else(
        || format!("{release_candidate}-2"),
        |prefix| format!("{prefix}2"),
    );
    [release_candidate.to_string(), second]
}

fn prefix_run_report(mut report: Value, prefix: &str) -> Value {
    for field in ["project_path", "export_path", "catalog_path"] {
        let path = report[field]
            .as_str()
            .expect("per-run report path is a string")
            .to_string();
        report[field] = json!(format!("{prefix}/{path}"));
    }
    if let Some(artifacts) = report["artifacts"].as_array_mut() {
        for artifact in artifacts {
            let path = artifact["relative_path"]
                .as_str()
                .expect("artifact path is a string")
                .to_string();
            artifact["relative_path"] = json!(format!("{prefix}/{path}"));
        }
    }
    report
}

/// Compare the published timing bands from two completed rehearsal runs.
pub fn compare_rehearsal_runs(
    runs: &[Value],
    project: impl AsRef<Path>,
) -> Result<Vec<Value>, RehearsalError> {
    let project = project.as_ref();
    let Some(first) = runs.first().and_then(|run| run["timings"].as_array()) else {
        return Err(RehearsalError::new(
            "comparison",
            json!({"message": "first run timings are missing"}),
            project,
        ));
    };
    let Some(second) = runs.get(1).and_then(|run| run["timings"].as_array()) else {
        return Err(RehearsalError::new(
            "comparison",
            json!({"message": "second run timings are missing"}),
            project,
        ));
    };
    if runs.len() != 2
        || first.len() != TIMING_CLASSES.len()
        || second.len() != TIMING_CLASSES.len()
    {
        return Err(RehearsalError::new(
            "comparison",
            json!({"message": "comparison requires two complete timing runs"}),
            project,
        ));
    }
    for class in TIMING_CLASSES {
        let first_count = first
            .iter()
            .filter(|timing| timing["class"] == class)
            .count();
        let second_count = second
            .iter()
            .filter(|timing| timing["class"] == class)
            .count();
        if first_count != 1 || second_count != 1 {
            return Err(RehearsalError::new(
                "comparison",
                json!({"message": "timing classes must match exactly once per run", "class": class}),
                project,
            ));
        }
    }
    let mut comparisons = Vec::with_capacity(first.len());
    for timing in first {
        let class = timing["class"].as_str().expect("timing class is a string");
        let other = second
            .iter()
            .find(|candidate| candidate["class"] == class)
            .ok_or_else(|| {
                RehearsalError::new(
                    "comparison",
                    json!({"message": "timing class is missing from the second run", "class": class}),
                    project,
                )
            })?;
        let run_1 = timing_band_values(timing);
        let run_2 = timing_band_values(other);
        let same = ["p50_ms", "p95_ms", "p99_ms"]
            .iter()
            .all(|field| same_order_of_magnitude(&run_1[field], &run_2[field]));
        if !same {
            return Err(RehearsalError::new(
                "comparison",
                json!({
                    "message": "release candidates differ in timing order of magnitude",
                    "class": class,
                    "run_1": run_1,
                    "run_2": run_2,
                }),
                project,
            ));
        }
        comparisons.push(json!({
            "class": class,
            "run_1": run_1,
            "run_2": run_2,
            "same_order_of_magnitude": true,
        }));
    }
    Ok(comparisons)
}

fn timing_band_values(timing: &Value) -> Value {
    json!({
        "p50_ms": timing["p50_ms"],
        "p95_ms": timing["p95_ms"],
        "p99_ms": timing["p99_ms"],
    })
}

fn same_order_of_magnitude(left: &Value, right: &Value) -> bool {
    let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) else {
        return false;
    };
    if !left.is_finite() || !right.is_finite() || left < 0.0 || right < 0.0 {
        return false;
    }
    if left == 0.0 || right == 0.0 {
        return left == right;
    }
    left.log10().floor() == right.log10().floor()
}

fn preflight_aggregate(output_dir: &Path, project: &Path) -> Result<(), RehearsalError> {
    if output_dir.exists() {
        let metadata = fs::symlink_metadata(output_dir).map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(RehearsalError::new(
                "preflight",
                json!({"message": "output_dir must be a real directory"}),
                project,
            ));
        }
        let entries = fs::read_dir(output_dir)
            .map_err(|error| {
                RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
            })?
            .collect::<Result<Vec<_>, io::Error>>()
            .map_err(|error| {
                RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
            })?;
        if entries.len() > 1
            || entries
                .first()
                .is_some_and(|entry| entry.file_name() != "run-2")
            || entries.first().is_some_and(|entry| {
                let metadata = fs::symlink_metadata(entry.path()).ok();
                metadata
                    .is_none_or(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
            })
        {
            return Err(RehearsalError::new(
                "preflight",
                json!({"message": "output_dir must be empty for a two-run rehearsal"}),
                project,
            ));
        }
    } else {
        fs::create_dir_all(output_dir).map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
        })?;
    }
    Ok(())
}

fn preflight(
    output_dir: &Path,
    project: &Path,
    release_candidate: &str,
) -> Result<(), RehearsalError> {
    if release_candidate.is_empty() {
        return Err(RehearsalError::new(
            "argument_parse",
            json!({"message": "release_candidate must not be empty"}),
            project,
        ));
    }
    if output_dir.exists() {
        let metadata = fs::symlink_metadata(output_dir).map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(RehearsalError::new(
                "preflight",
                json!({"message": "output_dir must be a real directory"}),
                project,
            ));
        }
    } else {
        fs::create_dir_all(output_dir).map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
        })?;
    }
    for entry in fs::read_dir(output_dir).map_err(|error| {
        RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
    })? {
        let entry = entry.map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            RehearsalError::new(
                "preflight",
                json!({"message": "output_dir contains a non-UTF-8 entry"}),
                project,
            )
        })?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), project)
        })?;
        if metadata.file_type().is_symlink() || !matches!(name.as_str(), EXPORT_DIR) {
            return Err(RehearsalError::new(
                "preflight",
                json!({"message": format!("unexpected output entry {name:?}")}),
                project,
            ));
        }
    }
    Ok(())
}

fn invoke_machine(
    stage: &str,
    project: &Path,
    prefix: &[&str],
    suffix: &[&str],
) -> Result<Value, RehearsalError> {
    let mut args: Vec<OsString> = prefix.iter().map(OsString::from).collect();
    args.extend(suffix.iter().map(OsString::from));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = crate::dispatch::dispatch(args, &mut stdout, &mut stderr);
    if exit != crate::dispatch::EXIT_OK {
        return Err(RehearsalError::new(
            stage,
            parse_detail(&stderr, &format!("production command exited with {exit}")),
            project,
        ));
    }
    if !stderr.is_empty() {
        return Err(RehearsalError::new(
            stage,
            parse_detail(&stderr, "production command wrote diagnostics on success"),
            project,
        ));
    }
    let response: Value = serde_json::from_slice(&stdout).map_err(|error| {
        RehearsalError::new(
            stage,
            json!({"message": format!("production command returned invalid JSON: {error}")}),
            project,
        )
    })?;
    let command = match stage {
        "project_create" => NEW_PROJECT_COMMAND_ID,
        "bracket_create" => BRACKET_COMMAND_ID,
        "reload" => LOAD_COMMAND_ID,
        "export" => EXPORT_COMMAND_ID,
        _ => return Ok(response),
    };
    let schema = find(command).expect("production command schema is registered");
    validate(&schema.response_schema, &response).map_err(|error| {
        RehearsalError::new(
            stage,
            json!({"message": format!("production response failed schema validation: {error}")}),
            project,
        )
    })?;
    Ok(response)
}

fn invoke_registered(
    stage: &str,
    project: &Path,
    host: &Host,
    command: threeterm_protocol::schema::CommandId,
    request: Value,
) -> Result<Value, RehearsalError> {
    let response =
        crate::dispatch::dispatch_registered_command(host, command, request).map_err(|error| {
            RehearsalError::new(
                stage,
                json!({"message": error.diagnostic_detail()}),
                project,
            )
        })?;
    let schema = match command {
        BRACKET_EDIT_COMMAND_ID => &BRACKET_EDIT_RESPONSE_SCHEMA,
        BRACKET_COMMAND_ID => &BRACKET_RESPONSE_SCHEMA,
        _ => &NEW_PROJECT_RESPONSE_SCHEMA,
    };
    validate(schema, &response).map_err(|error| {
        RehearsalError::new(
            stage,
            json!({"message": format!("production response failed schema validation: {error}")}),
            project,
        )
    })?;
    Ok(response)
}

fn parse_detail(bytes: &[u8], fallback: &str) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| json!({"message": fallback}))
}

fn collect_artifacts(project: &Path, export: &Path) -> Result<Vec<Artifact>, RehearsalError> {
    let mut artifacts = Vec::new();
    let output_dir = project
        .parent()
        .expect("project has an output directory parent");
    collect_tree(project, PROJECT_DIR, &mut artifacts)?;
    let previous = output_dir.join(PREVIOUS_PROJECT_DIR);
    if previous.exists() {
        collect_tree(&previous, PREVIOUS_PROJECT_DIR, &mut artifacts)?;
    }
    collect_tree(export, EXPORT_DIR, &mut artifacts)?;
    artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    if artifacts.is_empty() {
        return Err(RehearsalError::new(
            "catalog",
            json!({"message": "rehearsal produced no artifacts"}),
            project,
        ));
    }
    Ok(artifacts)
}

/// Verify a checked-in rehearsal output without rerunning native workers.
pub fn verify_rehearsal_evidence(root: impl AsRef<Path>) -> Result<(), RehearsalError> {
    let root = root.as_ref();
    let diagnostic_root = root.join("run-2").join(PROJECT_DIR);
    let mut allowed = vec!["run-1", "run-2", CATALOG_FILE];
    if root.join("adversarial").is_dir() {
        allowed.push("adversarial");
    }
    verify_directory_entries(root, &allowed, &diagnostic_root)?;
    if root.join("adversarial").is_dir() {
        verify_adversarial_evidence(root.join("adversarial"))?;
    }

    let aggregate = read_report(
        &root.join(CATALOG_FILE),
        &REHEARSE_RESPONSE_SCHEMA,
        &diagnostic_root,
    )?;
    let runs = aggregate["runs"].as_array().ok_or_else(|| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": "aggregate report omitted runs"}),
            &diagnostic_root,
        )
    })?;

    for (index, aggregate_run) in runs.iter().enumerate() {
        let run_root = root.join(format!("run-{}", index + 1));
        let run_project = run_root.join(PROJECT_DIR);
        let mut allowed = vec![PROJECT_DIR, EXPORT_DIR, CATALOG_FILE];
        if run_root.join(PREVIOUS_PROJECT_DIR).exists() {
            allowed.push(PREVIOUS_PROJECT_DIR);
        }
        verify_directory_entries(&run_root, &allowed, &run_project)?;
        let run_report = read_report(
            &run_root.join(CATALOG_FILE),
            &REHEARSE_RUN_RESPONSE_SCHEMA,
            &run_project,
        )?;
        if prefix_run_report(run_report.clone(), &format!("run-{}", index + 1)) != *aggregate_run {
            return Err(RehearsalError::new(
                "evidence_verification",
                json!({"message": "aggregate run does not match its per-run report", "run": index + 1}),
                &run_project,
            ));
        }

        let artifacts = collect_artifacts(&run_project, &run_root.join(EXPORT_DIR))?;
        let actual = serde_json::to_value(artifacts).expect("artifact catalog serializes");
        if actual != run_report["artifacts"] {
            return Err(RehearsalError::new(
                "evidence_verification",
                json!({"message": "artifact catalog does not match the committed files", "run": index + 1}),
                &run_project,
            ));
        }
    }
    Ok(())
}

fn read_report(
    path: &Path,
    schema: &Value,
    diagnostic_root: &Path,
) -> Result<Value, RehearsalError> {
    let bytes = fs::read(path).map_err(|error| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": error.to_string(), "path": path}),
            diagnostic_root,
        )
    })?;
    let report: Value = serde_json::from_slice(&bytes).map_err(|error| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": error.to_string(), "path": path}),
            diagnostic_root,
        )
    })?;
    validate(schema, &report).map_err(|error| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": format!("catalog report failed schema validation: {error}"), "path": path}),
            diagnostic_root,
        )
    })?;
    Ok(report)
}

fn verify_directory_entries(
    root: &Path,
    allowed: &[&str],
    diagnostic_root: &Path,
) -> Result<(), RehearsalError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": error.to_string(), "path": root}),
            diagnostic_root,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RehearsalError::new(
            "evidence_verification",
            json!({"message": "evidence root is not a real directory", "path": root}),
            diagnostic_root,
        ));
    }
    let mut names = fs::read_dir(root)
        .map_err(|error| {
            RehearsalError::new(
                "evidence_verification",
                json!({"message": error.to_string(), "path": root}),
                diagnostic_root,
            )
        })?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|error| {
                    RehearsalError::new(
                        "evidence_verification",
                        json!({"message": error.to_string(), "path": root}),
                        diagnostic_root,
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected = allowed
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    expected.sort();
    if names != expected {
        return Err(RehearsalError::new(
            "evidence_verification",
            json!({"message": "evidence directory has unexpected entries", "path": root, "actual": names, "expected": expected}),
            diagnostic_root,
        ));
    }
    for name in names {
        let path = root.join(name);
        if fs::symlink_metadata(&path)
            .map_err(|error| {
                RehearsalError::new(
                    "evidence_verification",
                    json!({"message": error.to_string(), "path": path}),
                    diagnostic_root,
                )
            })?
            .file_type()
            .is_symlink()
        {
            return Err(RehearsalError::new(
                "evidence_verification",
                json!({"message": "evidence contains a symlink", "path": path}),
                diagnostic_root,
            ));
        }
    }
    Ok(())
}

fn collect_tree(
    root: &Path,
    root_name: &str,
    artifacts: &mut Vec<Artifact>,
) -> Result<(), RehearsalError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        RehearsalError::new("catalog", json!({"message": error.to_string()}), root)
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RehearsalError::new(
            "catalog",
            json!({"message": format!("artifact root {root_name:?} is not a directory")}),
            root,
        ));
    }
    let mut entries = fs::read_dir(root)
        .map_err(|error| {
            RehearsalError::new("catalog", json!({"message": error.to_string()}), root)
        })?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| {
            RehearsalError::new("catalog", json!({"message": error.to_string()}), root)
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| {
            RehearsalError::new(
                "catalog",
                json!({"message": "artifact path is not valid UTF-8"}),
                root,
            )
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            RehearsalError::new("catalog", json!({"message": error.to_string()}), root)
        })?;
        let relative = format!("{root_name}/{name}");
        if metadata.file_type().is_symlink() {
            return Err(RehearsalError::new(
                "catalog",
                json!({"message": format!("symlink artifact {relative:?}")}),
                root,
            ));
        }
        if metadata.is_dir() {
            collect_tree(&path, &relative, artifacts)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&path).map_err(|error| {
                RehearsalError::new("catalog", json!({"message": error.to_string()}), root)
            })?;
            let sha256 = format!("{:x}", Sha256::digest(&bytes));
            artifacts.push(Artifact {
                relative_path: relative,
                bytes: bytes.len() as u64,
                sha256,
            });
        } else {
            return Err(RehearsalError::new(
                "catalog",
                json!({"message": format!("non-regular artifact {relative:?}")}),
                root,
            ));
        }
    }
    Ok(())
}

fn write_catalog(output_dir: &Path, report: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(report).expect("rehearsal report serializes");
    let temporary = output_dir.join(format!(".{CATALOG_FILE}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, output_dir.join(CATALOG_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

const ADVERSARIAL_SCHEMA: &str = "threeterm.rehearsal.adversarial/1";
pub const ADVERSARIAL_EVIDENCE_DIR: &str = "docs/research/rehearsal-evidence/l-bracket/adversarial";
const REFERENCE_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/research/rehearsal-evidence/l-bracket/run-2/project/manifest.json"
));
const REFERENCE_TRANSACTIONS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/research/rehearsal-evidence/l-bracket/run-2/project/transactions.log"
));
const REFERENCE_BREP: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/research/rehearsal-evidence/l-bracket/run-2/project/brep/l-bracket.brep"
));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdversarialCase {
    MismatchCache,
    SchemaV0,
    CapabilityLoss,
}

impl AdversarialCase {
    pub const ALL: [Self; 3] = [Self::MismatchCache, Self::SchemaV0, Self::CapabilityLoss];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MismatchCache => "mismatch-cache",
            Self::SchemaV0 => "schema-v0",
            Self::CapabilityLoss => "capability-loss",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|case| case.as_str() == value)
    }
}

/// Run one adversarial case and incrementally publish its evidence catalog.
pub fn run_adversarial_case(
    output_dir: impl AsRef<Path>,
    case: AdversarialCase,
) -> Result<Value, RehearsalError> {
    let output_dir = output_dir.as_ref();
    preflight_adversarial_root(output_dir)?;
    let staging = output_dir.join(format!(".{}-{}", case.as_str(), std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|error| {
        RehearsalError::new("evidence", json!({"message": error.to_string()}), &staging)
    })?;
    let report = match case {
        AdversarialCase::MismatchCache => run_mismatch_cache(&staging),
        AdversarialCase::SchemaV0 => run_schema_v0(&staging),
        AdversarialCase::CapabilityLoss => run_capability_loss(&staging),
    }?;
    write_json(&staging.join("report.json"), &report).map_err(|error| {
        RehearsalError::new("evidence", json!({"message": error.to_string()}), &staging)
    })?;
    let final_dir = output_dir.join(case.as_str());
    let _ = fs::remove_dir_all(&final_dir);
    fs::rename(&staging, &final_dir).map_err(|error| {
        RehearsalError::new(
            "evidence",
            json!({"message": error.to_string()}),
            output_dir,
        )
    })?;
    let manifest = write_adversarial_manifest(output_dir)?;
    Ok(json!({"case": case.as_str(), "report": report, "manifest": manifest}))
}

/// Run all three cases in the documented order.
pub fn run_all_adversarial_cases(output_dir: impl AsRef<Path>) -> Result<Value, RehearsalError> {
    let output_dir = output_dir.as_ref();
    for case in AdversarialCase::ALL {
        run_adversarial_case(output_dir, case)?;
    }
    let manifest = write_adversarial_manifest(output_dir)?;
    if manifest["cases"]
        != json!(
            AdversarialCase::ALL
                .iter()
                .map(|case| case.as_str())
                .collect::<Vec<_>>()
        )
    {
        return Err(RehearsalError::new(
            "evidence",
            json!({"message": "all adversarial cases did not publish"}),
            output_dir,
        ));
    }
    Ok(manifest)
}

/// Verify adversarial evidence without starting native workers.
pub fn verify_adversarial_evidence(root: impl AsRef<Path>) -> Result<(), RehearsalError> {
    let root = root.as_ref();
    let manifest_path = root.join(CATALOG_FILE);
    let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).map_err(|error| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": error.to_string()}),
            root,
        )
    })?)
    .map_err(|error| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": error.to_string()}),
            root,
        )
    })?;
    if manifest["schema_version"] != ADVERSARIAL_SCHEMA {
        return Err(RehearsalError::new(
            "evidence_verification",
            json!({"message": "adversarial manifest schema mismatch"}),
            root,
        ));
    }
    let expected = manifest["artifacts"].as_array().ok_or_else(|| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": "adversarial manifest omitted artifacts"}),
            root,
        )
    })?;
    let actual = collect_adversarial_artifacts(root).map_err(|error| {
        RehearsalError::new(
            "evidence_verification",
            json!({"message": error.to_string()}),
            root,
        )
    })?;
    let actual_value = serde_json::to_value(actual).expect("artifact list serializes");
    if actual_value != Value::Array(expected.clone()) {
        return Err(RehearsalError::new(
            "evidence_verification",
            json!({"message": "adversarial artifact catalog does not match files"}),
            root,
        ));
    }
    Ok(())
}

fn preflight_adversarial_root(root: &Path) -> Result<(), RehearsalError> {
    if root.exists() {
        let metadata = fs::symlink_metadata(root).map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), root)
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(RehearsalError::new(
                "preflight",
                json!({"message": "adversarial output root must be a real directory"}),
                root,
            ));
        }
    } else {
        fs::create_dir_all(root).map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), root)
        })?;
    }
    for entry in fs::read_dir(root).map_err(|error| {
        RehearsalError::new("preflight", json!({"message": error.to_string()}), root)
    })? {
        let entry = entry.map_err(|error| {
            RehearsalError::new("preflight", json!({"message": error.to_string()}), root)
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !matches!(
            name.as_str(),
            "mismatch-cache" | "schema-v0" | "capability-loss" | CATALOG_FILE
        ) && !name.starts_with('.')
        {
            return Err(RehearsalError::new(
                "preflight",
                json!({"message": format!("unexpected adversarial output entry {name:?}")}),
                root,
            ));
        }
    }
    Ok(())
}

fn write_adversarial_manifest(root: &Path) -> Result<Value, RehearsalError> {
    let mut cases = Vec::new();
    for case in AdversarialCase::ALL {
        if root.join(case.as_str()).is_dir() {
            cases.push(case.as_str());
        }
    }
    let artifacts = collect_adversarial_artifacts(root).map_err(|error| {
        RehearsalError::new("evidence", json!({"message": error.to_string()}), root)
    })?;
    let report = json!({
        "schema_version": ADVERSARIAL_SCHEMA,
        "fixture": FIXTURE,
        "cases": cases,
        "artifacts": artifacts,
    });
    write_catalog(root, &report).map_err(|error| {
        RehearsalError::new("evidence", json!({"message": error.to_string()}), root)
    })?;
    Ok(report)
}

fn collect_adversarial_artifacts(root: &Path) -> io::Result<Vec<Artifact>> {
    let mut artifacts = Vec::new();
    for case in AdversarialCase::ALL {
        let case_root = root.join(case.as_str());
        if case_root.is_dir() {
            collect_tree(&case_root, case.as_str(), &mut artifacts)
                .map_err(|error| io::Error::other(format!("{error:?}")))?;
        }
    }
    artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(artifacts)
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).expect("JSON report serializes");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

fn run_mismatch_cache(root: &Path) -> Result<Value, RehearsalError> {
    let project = root.join("working-project");
    invoke_machine(
        "project_create",
        &project,
        &["--machine", "new-project"],
        &[project.to_string_lossy().as_ref()],
    )?;
    invoke_machine(
        "bracket_create",
        &project,
        &["--machine", "bracket"],
        &[
            project.to_string_lossy().as_ref(),
            "--bracket-id",
            FIXTURE,
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    )?;
    let host = Host::new();
    host.load(&project)
        .map_err(|error| rehearsal_host_error("load", error, &project))?;
    let host_before = host.current();
    let before = canonical_state(&project)?;
    host.rebuild_lbracket_layer1_cache(&project)
        .map_err(|error| rehearsal_host_error("cache_rebuild", error, &project))?;
    let cache_record = project.join("cache/layer1.json");
    let mut tampered = fs::read(&cache_record)
        .map_err(|error| rehearsal_io_error("cache_tamper", error, &project))?;
    let position = tampered
        .windows(4)
        .position(|window| window == b"occt")
        .ok_or_else(|| {
            rehearsal_detail("cache_tamper", "worker fingerprint was not found", &project)
        })?;
    tampered[position] = b'p';
    fs::write(root.join("tampered-cache.json"), &tampered)
        .map_err(|error| rehearsal_io_error("evidence", error, root))?;
    let mismatch = host
        .load_with_layer1_cache(&project)
        .expect_err("tampered Layer 1 cache must be rejected");
    if !matches!(&mismatch, HostError::Layer1FingerprintMismatch { .. }) {
        return Err(rehearsal_detail(
            "cache_reload",
            format!("unexpected cache diagnostic: {mismatch}"),
            &project,
        ));
    }
    let host_snapshot_preserved = host.current() == host_before;
    let rebuilt = host
        .rebuild_lbracket_layer1_cache(&project)
        .map_err(|error| rehearsal_host_error("cache_rebuild", error, &project))?;
    fs::copy(&cache_record, root.join("repaired-cache.json"))
        .map_err(|error| rehearsal_io_error("evidence", error, root))?;
    fs::copy(
        project.join("cache/l-bracket.brep"),
        root.join("repaired-cache.brep"),
    )
    .map_err(|error| rehearsal_io_error("evidence", error, root))?;
    let after = canonical_state(&project)?;
    copy_canonical(&project, root)?;
    let canonical_byte_equal = before.files == after.files && before.revision == after.revision;
    fs::remove_dir_all(&project).map_err(|error| rehearsal_io_error("cleanup", error, root))?;
    Ok(json!({
        "schema_version": ADVERSARIAL_SCHEMA,
        "case": AdversarialCase::MismatchCache.as_str(),
        "diagnostic": {
            "code": "LAYER_1_FINGERPRINT_MISMATCH",
            "detail": mismatch.to_string(),
            "recovery": "discarded tampered cache and recomputed through the OCCT worker"
        },
        "canonical_revision_before": before.revision,
        "canonical_revision_after": after.revision,
        "canonical_byte_equal": canonical_byte_equal,
        "host_snapshot_preserved": host_snapshot_preserved,
        "recomputations": rebuilt.recomputations,
    }))
}

fn run_schema_v0(root: &Path) -> Result<Value, RehearsalError> {
    let project = root.join("working-project");
    materialize_reference_bundle(&project)?;
    let host = Host::new();
    host.load(&project)
        .map_err(|error| rehearsal_host_error("load", error, &project))?;
    let host_before = host.current();
    let before = canonical_state(&project)?;
    let v0 = root.join("input-v0");
    write_v0_fixture(&v0, ProjectGeneration::with_id("adversarial-v0"))
        .map_err(|error| rehearsal_detail("v0_fixture", error.to_string(), &v0))?;
    let v0_before = fingerprint_tree(&v0)?;
    let refusal = host
        .load_adversarial_v0(&v0)
        .expect_err("adversarial v0 load must fail closed");
    if !matches!(
        &refusal,
        HostError::Persistence(BundleError::SchemaEpochV0RequiresBackup)
    ) {
        return Err(rehearsal_detail("v0_load", refusal.to_string(), &v0));
    }
    let host_snapshot_preserved = host.current() == host_before;
    let after = canonical_state(&project)?;
    copy_canonical(&project, root)?;
    copy_file(&v0.join("manifest.json"), &root.join("v0-manifest.json"))?;
    copy_file(
        &v0.join("canonical/transactions.ndjson"),
        &root.join("v0-transactions.ndjson"),
    )?;
    let canonical_byte_equal = before.files == after.files && before.revision == after.revision;
    let backup = v0.with_file_name(format!(
        "{}{}",
        v0.file_name().unwrap().to_string_lossy(),
        threeterm_persistence::PRE_MIGRATION_BACKUP_SUFFIX
    ));
    let v0_unchanged = fingerprint_tree(&v0)? == v0_before;
    let no_backup = !backup.exists();
    fs::remove_dir_all(&project).map_err(|error| rehearsal_io_error("cleanup", error, root))?;
    fs::remove_dir_all(&v0).map_err(|error| rehearsal_io_error("cleanup", error, root))?;
    Ok(json!({
        "schema_version": ADVERSARIAL_SCHEMA,
        "case": AdversarialCase::SchemaV0.as_str(),
        "diagnostic": {
            "code": "SCHEMA_EPOCH_V0_REQUIRES_BACKUP",
            "detail": refusal.to_string(),
            "recovery": "refused the v0 bundle without changing the canonical snapshot"
        },
        "canonical_revision_before": before.revision,
        "canonical_revision_after": after.revision,
        "canonical_byte_equal": canonical_byte_equal,
        "host_snapshot_preserved": host_snapshot_preserved,
        "v0_byte_equal": v0_unchanged,
        "backup_created": !no_backup,
    }))
}

fn run_capability_loss(root: &Path) -> Result<Value, RehearsalError> {
    let project = root.join("working-project");
    materialize_reference_bundle(&project)?;
    let host = Host::new();
    host.load(&project)
        .map_err(|error| rehearsal_host_error("load", error, &project))?;
    let before = canonical_state(&project)?;
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let termios_calls = Rc::new(Cell::new(0));
    let probe = valid_capability_result();
    let renderer = GhosttyRenderer::with_termios_restorer(
        SharedWriter(Rc::clone(&bytes)),
        RecordingTermios(Rc::clone(&termios_calls)),
    );
    let mut session = TuiViewportSession::from_host_with_probe(&host, 80, 24, renderer, &probe)
        .map_err(|error| rehearsal_detail("viewport_start", error.to_string(), &project))?;
    session
        .process_terminal_input(b"\x1b[C")
        .map_err(|error| rehearsal_detail("frame_1", error.to_string(), &project))?;
    let mut failed_probe = FailedProbe::default();
    let probe_error = CapabilityProbe::new(77)
        .probe(&mut failed_probe, valid_terminal_environment())
        .expect_err("the second capability probe must fail");
    let before_cleanup = bytes.borrow().len();
    session
        .report_viewport_failure(&probe_error)
        .map_err(|error| rehearsal_detail("capability_loss", format!("{error:?}"), &project))?;
    let after_cleanup = bytes.borrow().len();
    let second_frame = ViewportFrame {
        revision: before.revision.clone(),
        generation: 2,
        width: 1,
        height: 1,
        rgb: vec![1, 2, 3],
        frame_token: None,
    };
    let second_frame_error = session
        .coordinator_mut()
        .submit(second_frame)
        .expect_err("invalidated renderer must reject the second frame");
    let after_second_frame = bytes.borrow().len();
    if second_frame_error.code != ViewportDiagnosticCode::CapabilityInvalidated {
        return Err(rehearsal_detail(
            "frame_2",
            format!("unexpected second-frame diagnostic: {second_frame_error}"),
            &project,
        ));
    }
    session
        .complete_viewport_restore()
        .map_err(|error| rehearsal_detail("headless_restore", format!("{error:?}"), &project))?;
    let lifecycle = format!("{:?}", session.state().lifecycle);
    if lifecycle != "HeadlessOnly" {
        return Err(rehearsal_detail(
            "headless_restore",
            format!("unexpected lifecycle state: {lifecycle}"),
            &project,
        ));
    }
    let terminal = bytes.borrow().clone();
    let after = canonical_state(&project)?;
    let terminal_state = json!({
        "alternate_screen_exited": terminal.windows(b"?1049l".len()).any(|w| w == b"?1049l"),
        "kitty_image_deleted": terminal.windows(b"a=d,d=I".len()).any(|w| w == b"a=d,d=I"),
        "kitty_transmission_disabled": !terminal[before_cleanup..].windows(3).any(|w| w == b"a=T"),
        "keyboard_mouse_focus_disabled": terminal.windows(b"?1016l".len()).any(|w| w == b"?1016l"),
        "cursor_and_attributes_restored": terminal.windows(b"?25h".len()).any(|w| w == b"?25h") && terminal.windows(b"0m".len()).any(|w| w == b"0m"),
        "termios_restored": termios_calls.get() == 1,
        "second_frame_bytes_added": after_second_frame > after_cleanup,
        "second_frame_rejected_code": second_frame_error.code.as_str(),
    });
    write_json(&root.join("terminal-state.json"), &terminal_state)
        .map_err(|error| rehearsal_io_error("evidence", error, root))?;
    fs::write(root.join("failed-probe-response.bin"), &failed_probe.bytes)
        .map_err(|error| rehearsal_io_error("evidence", error, root))?;
    copy_canonical(&project, root)?;
    let canonical_byte_equal = before.files == after.files && before.revision == after.revision;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = crate::dispatch::dispatch(
        [
            OsString::from("--machine"),
            OsString::from("load"),
            OsString::from(project.to_string_lossy().as_ref()),
        ],
        &mut stdout,
        &mut stderr,
    );
    if exit != crate::dispatch::EXIT_OK || !stderr.is_empty() {
        return Err(rehearsal_detail(
            "headless_route",
            "headless load failed",
            &project,
        ));
    }
    fs::remove_dir_all(&project).map_err(|error| rehearsal_io_error("cleanup", error, root))?;
    Ok(json!({
        "schema_version": ADVERSARIAL_SCHEMA,
        "case": AdversarialCase::CapabilityLoss.as_str(),
        "diagnostic": {
            "code": "CAPABILITY_LOSS",
            "detail": probe_error.to_string(),
            "recovery": "bounded cleanup completed and the next command routed to HeadlessOnly"
        },
        "canonical_revision_before": before.revision,
        "canonical_revision_after": after.revision,
        "canonical_byte_equal": canonical_byte_equal,
        "lifecycle": lifecycle,
        "next_command_route": "HeadlessOnly",
        "terminal_state": terminal_state,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonicalState {
    revision: String,
    files: Vec<(String, Vec<u8>)>,
}

fn canonical_state(project: &Path) -> Result<CanonicalState, RehearsalError> {
    let revision = Bundle::at(project)
        .open()
        .map_err(|error| rehearsal_detail("canonical_state", error.to_string(), project))?
        .revision_hash_hex()
        .to_string();
    let mut files = Vec::new();
    for relative in ["manifest.json", "transactions.log", "brep/l-bracket.brep"] {
        files.push((
            relative.to_string(),
            fs::read(project.join(relative))
                .map_err(|error| rehearsal_io_error("canonical_state", error, project))?,
        ));
    }
    Ok(CanonicalState { revision, files })
}

fn copy_canonical(project: &Path, root: &Path) -> Result<(), RehearsalError> {
    for (relative, _) in canonical_state(project)?.files {
        copy_file(
            &project.join(&relative),
            &root.join("canonical").join(relative),
        )?;
    }
    Ok(())
}

fn materialize_reference_bundle(project: &Path) -> Result<(), RehearsalError> {
    fs::create_dir_all(project.join("brep"))
        .map_err(|error| rehearsal_io_error("fixture", error, project))?;
    fs::write(project.join("manifest.json"), REFERENCE_MANIFEST)
        .map_err(|error| rehearsal_io_error("fixture", error, project))?;
    fs::write(project.join("transactions.log"), REFERENCE_TRANSACTIONS)
        .map_err(|error| rehearsal_io_error("fixture", error, project))?;
    fs::write(project.join("brep/l-bracket.brep"), REFERENCE_BREP)
        .map_err(|error| rehearsal_io_error("fixture", error, project))?;
    Bundle::at(project)
        .open()
        .map_err(|error| rehearsal_detail("fixture", error.to_string(), project))?;
    Ok(())
}

fn fingerprint_tree(root: &Path) -> Result<Vec<(String, String)>, RehearsalError> {
    let mut files = Vec::new();
    fingerprint_tree_inner(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn fingerprint_tree_inner(
    root: &Path,
    current: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), RehearsalError> {
    for entry in
        fs::read_dir(current).map_err(|error| rehearsal_io_error("fingerprint", error, current))?
    {
        let entry = entry.map_err(|error| rehearsal_io_error("fingerprint", error, current))?;
        let path = entry.path();
        if path.is_dir() {
            fingerprint_tree_inner(root, &path, files)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("tree entry has root prefix")
                .to_string_lossy()
                .into_owned();
            files.push((
                relative,
                format!(
                    "{:x}",
                    Sha256::digest(fs::read(&path).map_err(|error| rehearsal_io_error(
                        "fingerprint",
                        error,
                        &path
                    ))?)
                ),
            ));
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), RehearsalError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| rehearsal_io_error("evidence", error, parent))?;
    }
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| rehearsal_io_error("evidence", error, destination))
}

fn rehearsal_detail(stage: &str, detail: impl Into<String>, project: &Path) -> RehearsalError {
    RehearsalError::new(stage, json!({"message": detail.into()}), project)
}

fn rehearsal_io_error(stage: &str, error: io::Error, project: &Path) -> RehearsalError {
    rehearsal_detail(stage, error.to_string(), project)
}

fn rehearsal_host_error(stage: &str, error: HostError, project: &Path) -> RehearsalError {
    rehearsal_detail(stage, error.to_string(), project)
}

#[derive(Debug, Clone)]
struct SharedWriter(Rc<RefCell<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct RecordingTermios(Rc<Cell<u8>>);

impl threeterm_viewport::TermiosRestorer for RecordingTermios {
    fn restore(&mut self) -> Result<(), String> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct FailedProbe {
    bytes: Vec<u8>,
}

impl Write for FailedProbe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapabilityProbeIo for FailedProbe {
    fn read_probe_response(&mut self, _max_bytes: usize) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

fn valid_capability_result() -> threeterm_viewport::CapabilityProbeResult {
    threeterm_viewport::CapabilityProbeResult {
        capabilities: TerminalCapabilityVector {
            state: CapabilityState::Valid,
            direct_ghostty: true,
            kitty_rgb_zlib: true,
            kitty_acknowledgements: true,
            kitty_keyboard: true,
            sgr_mouse_cell: true,
            sgr_mouse_pixel: true,
            focus_reporting: true,
            alternate_screen: true,
            resize_events: true,
        },
        unrelated_input: Vec::new(),
        response_evidence: "injected-valid-probe".to_string(),
    }
}

fn valid_terminal_environment() -> TerminalEnvironment {
    TerminalEnvironment {
        term: Some("xterm-ghostty".to_string()),
        term_program: Some("ghostty".to_string()),
        in_tmux: false,
        over_ssh: false,
        foreground_tty: true,
        utf8: true,
        width: 80,
        height: 24,
    }
}

#[cfg(test)]
mod tests {
    use super::same_order_of_magnitude;
    use serde_json::json;

    #[test]
    fn equal_zero_bands_match_but_zero_and_positive_do_not() {
        assert!(same_order_of_magnitude(&json!(0.0), &json!(0.0)));
        assert!(!same_order_of_magnitude(&json!(0.0), &json!(0.1)));
    }

    #[test]
    fn positive_bands_match_within_an_exponent_and_fail_at_a_boundary() {
        assert!(same_order_of_magnitude(&json!(0.1), &json!(0.99)));
        assert!(same_order_of_magnitude(&json!(1.0), &json!(9.99)));
        assert!(!same_order_of_magnitude(&json!(9.99), &json!(10.0)));
        assert!(!same_order_of_magnitude(&json!(1.0), &json!(100.0)));
    }
}

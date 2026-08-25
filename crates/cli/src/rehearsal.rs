use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use threeterm_host::Host;
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, BRACKET_EDIT_COMMAND_ID, BRACKET_EDIT_RESPONSE_SCHEMA,
    BRACKET_RESPONSE_SCHEMA, EXPORT_COMMAND_ID, LOAD_COMMAND_ID, NEW_PROJECT_COMMAND_ID,
    NEW_PROJECT_RESPONSE_SCHEMA, REHEARSE_RESPONSE_SCHEMA_VERSION, find,
};
use threeterm_protocol::schema_validator::validate;

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

/// Run the canonical L-bracket rehearsal once and write its artifact catalog.
pub fn run_l_bracket_rehearsal(
    output_dir: impl AsRef<Path>,
    release_candidate: &str,
) -> Result<Value, RehearsalError> {
    let output_dir = output_dir.as_ref();
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
        "schema_version": REHEARSE_RESPONSE_SCHEMA_VERSION,
        "release_candidate": release_candidate,
        "fixture": FIXTURE,
        "run_count": 1,
        "sample_policy": "nearest-rank",
        "promoted": false,
        "project_path": PROJECT_DIR,
        "export_path": EXPORT_DIR,
        "catalog_path": CATALOG_FILE,
        "timings": timings,
        "artifacts": artifacts,
    });
    validate(
        &find(threeterm_protocol::schema::REHEARSE_COMMAND_ID)
            .expect("rehearse schema is registered")
            .response_schema,
        &report,
    )
    .map_err(|error| {
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

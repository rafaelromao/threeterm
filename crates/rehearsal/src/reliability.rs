use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, Operation};
use threeterm_protocol::schema::NEW_PROJECT_COMMAND_ID;

pub const RELIABILITY_SCHEMA_VERSION: &str = "threeterm.reliability.failure-drill/1";
pub const RELIABILITY_TEST_ID: &str =
    "reliability.failure_drill_isolation_bounded_cleanup_and_retained_evidence";

const SURFACE: &str = "api";
const TOOL: &str = "extrude";
const RECIPE_STEP: &str = "injected-worker-failure";
const SEMANTIC_FEATURE: &str = "reliability-failure-feature";
const CANCELLATION_GRACE: Duration = Duration::from_millis(150);
const FORCED_CLEANUP_BOUND: Duration = Duration::from_secs(2);
const TOTAL_BOUND: Duration = Duration::from_secs(4);
const READY_BOUND: Duration = Duration::from_secs(2);

type DrillResult<T = Value> = Result<T, String>;

/// Run the named failure drill through the production Host/worker boundary.
/// The output root is intentionally retained; callers can inspect a failed
/// journey without rerunning the fault.
pub fn run_reliability_failure_drill(root: impl AsRef<Path>) -> DrillResult {
    let root = root.as_ref();
    prepare_root(root)?;
    for directory in [
        "config",
        "export",
        "logs",
        "evidence",
        "artifacts",
        "screenshots",
    ] {
        fs::create_dir_all(root.join(directory)).map_err(io_detail)?;
    }

    let project = root.join("project");
    let config = root.join("config");
    let export = root.join("export");
    let logs = root.join("logs");
    let evidence = root.join("evidence");
    let artifacts = root.join("artifacts");
    let screenshots = root.join("screenshots");
    let process_log = config.join("processes.jsonl");
    let ready_marker = config.join("ready");
    let fixture = config.join("worker.sh");

    let mut transcript = Vec::new();
    transcript.push(json!({
        "event": "journey_started",
        "test": RELIABILITY_TEST_ID,
        "surface": SURFACE,
        "tool": TOOL,
        "recipe_step": RECIPE_STEP,
        "semantic_feature": SEMANTIC_FEATURE,
    }));
    write_json_lines(&evidence.join("transcript.jsonl"), &transcript)?;

    let fixture_text = fixture_script(&process_log, &ready_marker);
    fs::write(&fixture, fixture_text.as_bytes()).map_err(io_detail)?;
    let mut permissions = fs::metadata(&fixture).map_err(io_detail)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fixture, permissions).map_err(io_detail)?;

    let host = Host::new();
    host.execute_domain_command(
        NEW_PROJECT_COMMAND_ID,
        json!({"destination": project.to_string_lossy()}),
    )
    .map_err(|error| format!("new-project failed: {error:?}"))?;
    let before_manifest = fs::read(project.join("manifest.json")).map_err(io_detail)?;
    let before_log = fs::read(project.join("transactions.log")).map_err(io_detail)?;
    transcript.push(json!({"event": "project_created", "path": "project"}));
    write_json_lines(&evidence.join("transcript.jsonl"), &transcript)?;

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_watcher = Arc::clone(&cancel);
    let ready_for_watcher = ready_marker.clone();
    let watcher = thread::spawn(move || {
        let deadline = Instant::now() + READY_BOUND;
        while Instant::now() < deadline {
            if ready_for_watcher.is_file() {
                cancel_for_watcher.store(true, Ordering::SeqCst);
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    });

    let worker = OcctWorker::with_binary_path(fixture.clone())
        .with_expected_worker_id("reliability-fixture")
        .with_grace(Duration::from_secs(1))
        .with_operation_grace(Operation::Extrude, CANCELLATION_GRACE);
    let request = ExtrudeRequest::new(
        "reliability-request".to_string(),
        vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)],
        2.0,
    )
    .with_output_path(&export, "failed.brep")
    .with_feature_id(SEMANTIC_FEATURE);

    let started = Instant::now();
    let outcome = host.extrude_with_cancel(&project, request, &worker, &cancel);
    let ready_observed = watcher
        .join()
        .map_err(|_| "ready watcher panicked".to_string())?;
    let error = match outcome {
        Err(HostError::WorkerTerminated { record }) => *record,
        Err(error) => {
            return Err(format!(
                "injected failure used the wrong error path: {error}"
            ));
        }
        Ok(_) => return Err("injected failure unexpectedly completed".to_string()),
    };
    if !ready_observed {
        return Err("failure fixture did not reach its ready marker".to_string());
    }
    let measured_elapsed = started.elapsed();
    if measured_elapsed > TOTAL_BOUND {
        return Err(format!(
            "failure drill exceeded total bound: {:?} > {:?}; stage={}, exit_kind={}",
            measured_elapsed,
            TOTAL_BOUND,
            error.stage,
            error.exit_kind.as_str()
        ));
    }

    transcript.extend([
        json!({
            "event": "cancellation_requested",
            "grace_bound_ms": CANCELLATION_GRACE.as_millis(),
        }),
        json!({
            "event": "failure_observed",
            "stage": error.stage,
            "exit_kind": error.exit_kind.as_str(),
            "elapsed_ms": error.elapsed.as_millis(),
        }),
        json!({"event": "forced_cleanup_verified"}),
    ]);
    write_json_lines(&evidence.join("transcript.jsonl"), &transcript)?;
    fs::write(logs.join("worker.stderr"), error.stderr_tail.as_bytes()).map_err(io_detail)?;
    fs::write(
        logs.join("host-diagnostic.txt"),
        format!(
            "stage={}\ndetail={:?}\n",
            error.stage, error.termination_error
        )
        .as_bytes(),
    )
    .map_err(io_detail)?;
    fs::write(
        artifacts.join("failure-marker.txt"),
        b"injected failure: api/extrude/injected-worker-failure/reliability-failure-feature\n",
    )
    .map_err(io_detail)?;
    fs::write(
        screenshots.join("not-applicable.json"),
        serde_json::to_vec_pretty(&json!({
            "status": "not_applicable",
            "reason": "headless API surface has no graphical viewport"
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(io_detail)?;

    let after_manifest = fs::read(project.join("manifest.json")).map_err(io_detail)?;
    let after_log = fs::read(project.join("transactions.log")).map_err(io_detail)?;
    if before_manifest != after_manifest || before_log != after_log {
        return Err("failure changed canonical project state".to_string());
    }
    if export.join("failed.brep").exists() {
        return Err("failed staged BREP was retained outside evidence".to_string());
    }

    let owned_processes = read_process_identities(&process_log)?;
    let remaining = live_owned_processes(&owned_processes)?;
    let forced = error.exit_kind.as_str() == "force_after_grace"
        && error.stage.starts_with("cancel_grace_exceeded");
    if !forced || !remaining.is_empty() {
        return Err(format!(
            "cleanup was not forced and complete: forced={forced}, remaining={remaining:?}"
        ));
    }

    let report = json!({
        "schema_version": RELIABILITY_SCHEMA_VERSION,
        "test": RELIABILITY_TEST_ID,
        "surface": SURFACE,
        "tool": TOOL,
        "recipe_step": RECIPE_STEP,
        "semantic_feature": SEMANTIC_FEATURE,
        "diagnostic": {
            "code": "injected_failure",
            "stage": error.stage,
            "detail": error.failed_detail,
            "stderr": error.stderr_tail,
            "last_progress": error.last_progress.as_ref().map(|progress| {
                json!({"stage": progress.stage, "percent": progress.percent})
            }),
            "last_artifact_error": error.last_artifact_error,
            "failed_code": error.failed_code,
            "protocol_diagnostic": error.protocol_diagnostic.as_ref().map(|diagnostic| {
                json!({
                    "code": diagnostic.code.as_str(),
                    "detail": diagnostic.detail
                })
            }),
            "termination_error": error.termination_error,
            "artifact_location": "artifacts/failure-marker.txt"
        },
        "cleanup": {
            "graceful_bound_ms": CANCELLATION_GRACE.as_millis(),
            "forced_cleanup_bound_ms": FORCED_CLEANUP_BOUND.as_millis(),
            "total_bound_ms": TOTAL_BOUND.as_millis(),
            "graceful_elapsed_ms": error.elapsed.as_millis().min(CANCELLATION_GRACE.as_millis()),
            "forced_cleanup_elapsed_ms": error.elapsed.as_millis().saturating_sub(CANCELLATION_GRACE.as_millis()),
            "measured_elapsed_ms": measured_elapsed.as_millis(),
            "forced": forced,
            "exit_kind": error.exit_kind.as_str(),
            "exit_signal": error.exit_signal,
            "remaining_processes": remaining
        },
        "owned_processes": owned_processes,
        "screenshots": {
            "status": "not_applicable",
            "reason": "headless API surface has no graphical viewport",
            "path": "screenshots/not-applicable.json"
        },
        "paths": {
            "project": "project",
            "configuration": "config",
            "export": "export",
            "logs": "logs",
            "transcript": "evidence/transcript.jsonl",
            "artifacts": "artifacts",
            "evidence": "evidence/report.json"
        },
        "canonical_state_unchanged": true
    });
    write_json_atomic(&evidence.join("report.json"), &report)?;
    let manifest = write_artifact_manifest(root, &artifacts)?;
    if manifest["artifacts"].as_array().is_none_or(Vec::is_empty) {
        return Err("reliability artifact manifest is empty".to_string());
    }
    Ok(report)
}

/// Verify retained reliability evidence without rerunning the worker.
pub fn verify_reliability_failure_evidence(root: impl AsRef<Path>) -> DrillResult {
    let root = root.as_ref();
    require_real_directory(root)?;
    let report: Value = read_json(&root.join("evidence/report.json"))?;
    require(
        report["schema_version"] == RELIABILITY_SCHEMA_VERSION,
        "schema version",
    )?;
    for (field, expected) in [
        ("test", RELIABILITY_TEST_ID),
        ("surface", SURFACE),
        ("tool", TOOL),
        ("recipe_step", RECIPE_STEP),
        ("semantic_feature", SEMANTIC_FEATURE),
    ] {
        require(report[field] == expected, field)?;
    }
    require(
        report["diagnostic"]["code"] == "injected_failure",
        "diagnostic code",
    )?;
    require(
        report["diagnostic"]["last_progress"]["percent"] == 37,
        "last progress",
    )?;
    require(
        report["diagnostic"]["artifact_location"] == "artifacts/failure-marker.txt",
        "diagnostic artifact location",
    )?;
    require(report["cleanup"]["forced"] == true, "forced cleanup")?;
    require(
        report["cleanup"]["exit_kind"] == "force_after_grace",
        "forced exit kind",
    )?;
    require(
        report["diagnostic"]["stage"] == "cancel_grace_exceeded",
        "cancellation grace stage",
    )?;
    require(
        report["cleanup"]["graceful_elapsed_ms"]
            .as_u64()
            .is_some_and(|value| value <= CANCELLATION_GRACE.as_millis() as u64),
        "graceful cleanup bound",
    )?;
    require(
        report["cleanup"]["forced_cleanup_elapsed_ms"]
            .as_u64()
            .is_some_and(|value| value <= FORCED_CLEANUP_BOUND.as_millis() as u64),
        "forced cleanup bound",
    )?;
    require(
        report["cleanup"]["measured_elapsed_ms"]
            .as_u64()
            .is_some_and(|value| value <= TOTAL_BOUND.as_millis() as u64),
        "total cleanup bound",
    )?;
    require(
        report["cleanup"]["remaining_processes"] == json!([]),
        "remaining process inventory",
    )?;
    require(
        report["screenshots"]["status"] == "not_applicable",
        "screenshot applicability",
    )?;
    require(
        report["canonical_state_unchanged"] == true,
        "canonical state",
    )?;
    let paths = report["paths"]
        .as_object()
        .ok_or_else(|| "report paths are not an object".to_string())?;
    for path in paths.values() {
        let relative = path
            .as_str()
            .ok_or_else(|| "report path is not a string".to_string())?;
        let _ = safe_join(root, relative)?;
    }

    for directory in [
        "project",
        "config",
        "export",
        "logs",
        "evidence",
        "artifacts",
        "screenshots",
    ] {
        require_real_directory(&root.join(directory))?;
    }
    for file in [
        "project/manifest.json",
        "project/transactions.log",
        "config/worker.sh",
        "config/processes.jsonl",
        "logs/worker.stderr",
        "logs/host-diagnostic.txt",
        "evidence/transcript.jsonl",
        "evidence/report.json",
        "screenshots/not-applicable.json",
        "artifacts/failure-marker.txt",
    ] {
        require_regular_file(&root.join(file))?;
    }

    let manifest_path = root.join("artifacts/manifest.json");
    require_regular_file(&manifest_path)?;
    let manifest = read_json(&manifest_path)?;
    let entries = manifest["artifacts"]
        .as_array()
        .ok_or_else(|| "artifact manifest has no artifacts array".to_string())?;
    for entry in entries {
        let relative = entry["path"]
            .as_str()
            .ok_or_else(|| "artifact path is not a string".to_string())?;
        let path = safe_join(root, relative)?;
        require_regular_file(&path)?;
        require(
            relative != "artifacts/manifest.json",
            "manifest must exclude itself",
        )?;
        let bytes = fs::read(&path).map_err(io_detail)?;
        require(entry["bytes"] == json!(bytes.len()), "artifact byte count")?;
        require(entry["sha256"] == sha256(&bytes), "artifact digest")?;
    }
    require(
        entries
            .iter()
            .any(|entry| entry["path"] == "evidence/report.json"),
        "report artifact",
    )?;
    require(
        entries
            .iter()
            .all(|entry| entry["path"].as_str().is_some_and(|path| !path.is_empty())),
        "artifact paths",
    )?;
    let owned_processes = report["owned_processes"]
        .as_array()
        .ok_or_else(|| "owned process inventory is not an array".to_string())?;
    for role in ["worker", "worker-child", "server", "graphical"] {
        require(
            owned_processes
                .iter()
                .any(|process| process["role"] == role),
            role,
        )?;
    }
    for process in owned_processes {
        for field in [
            "pid",
            "role",
            "start_time",
            "process_group",
            "cgroup",
            "state",
        ] {
            require(!process[field].is_null(), field)?;
        }
    }
    Ok(report)
}

fn prepare_root(root: &Path) -> DrillResult<()> {
    if root.exists() {
        let metadata = fs::symlink_metadata(root).map_err(io_detail)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("reliability root must be a real directory".to_string());
        }
        let entries = fs::read_dir(root).map_err(io_detail)?;
        for entry in entries {
            let entry = entry.map_err(io_detail)?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(io_detail)?;
            if path.file_name().and_then(|name| name.to_str()) != Some("sentinel.txt")
                || metadata.file_type().is_symlink()
                || !metadata.is_file()
            {
                return Err("reliability root may only retain a regular sentinel.txt".to_string());
            }
        }
    } else {
        fs::create_dir_all(root).map_err(io_detail)?;
    }
    Ok(())
}

fn fixture_script(process_log: &Path, ready: &Path) -> String {
    let process_log = shell_quote(process_log);
    let ready = shell_quote(ready);
    format!(
        "#!/bin/sh\n\
set -eu\n\
record_pid() {{\n\
  pid=\"$1\"\n\
  role=\"$2\"\n\
  stat=\"$(cat /proc/$pid/stat)\"\n\
  rest=\"${{stat#*) }}\"\n\
  state=\"$(printf '%s' \"$rest\" | awk '{{print $1}}')\"\n\
  pgid=\"$(printf '%s' \"$rest\" | awk '{{print $3}}')\"\n\
  start=\"$(printf '%s' \"$rest\" | awk '{{print $20}}')\"\n\
  cgroup=\"$(tr '\\\\n' ';' < /proc/$pid/cgroup | tr -d '\"')\"\n\
  printf '{{\"pid\":%s,\"role\":\"%s\",\"start_time\":\"%s\",\"process_group\":%s,\"cgroup\":\"%s\",\"state\":\"%s\"}}\\n' \"$pid\" \"$role\" \"$start\" \"$pgid\" \"$cgroup\" \"$state\" >> {process_log}\n\
}}\n\
record_pid $$ worker\n\
(sleep 30) & worker_child=$!\n\
(setsid sleep 30) & server_child=$!\n\
(setsid sleep 30) & graphical_child=$!\n\
record_pid \"$worker_child\" worker-child\n\
record_pid \"$server_child\" server\n\
record_pid \"$graphical_child\" graphical\n\
printf '%s\\n' '{{\"event\":\"descendants_spawned\"}}' >> {process_log}\n\
: > {ready}\n\
printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"reliability-fixture\"}}'\n\
read request\n\
rid=\"$(printf '%s' \"$request\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\"\n\
printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"'$rid'\",\"stage\":\"injected-failure\",\"percent\":37}}'\n\
printf '%s\\n' 'injected failure surface=api tool=extrude recipe_step=injected-worker-failure semantic_feature=reliability-failure-feature artifact=artifacts/failure-marker.txt' >&2\n\
read cancellation\n\
sleep 30\n",
        process_log = process_log,
        ready = ready,
    )
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn read_process_identities(path: &Path) -> DrillResult<Vec<Value>> {
    let text = fs::read_to_string(path).map_err(io_detail)?;
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.contains("descendants_spawned"))
        .map(|line| {
            serde_json::from_str(line).map_err(|error| format!("process identity JSON: {error}"))
        })
        .collect()
}

fn live_owned_processes(identities: &[Value]) -> DrillResult<Vec<Value>> {
    let mut remaining = Vec::new();
    for identity in identities {
        let pid = identity["pid"]
            .as_i64()
            .ok_or_else(|| "process identity has no pid".to_string())?;
        let path = PathBuf::from(format!("/proc/{pid}/stat"));
        if !path.exists() {
            continue;
        }
        let stat = fs::read_to_string(&path).map_err(io_detail)?;
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            return Err(format!("unable to parse live process identity {pid}"));
        };
        let fields = fields.split_whitespace().collect::<Vec<_>>();
        let state = fields.first().copied().unwrap_or("?");
        let start = fields.get(19).copied().unwrap_or("");
        if start == identity["start_time"].as_str().unwrap_or("") && state != "Z" {
            remaining.push(identity.clone());
        }
    }
    Ok(remaining)
}

fn write_json_lines(path: &Path, entries: &[Value]) -> DrillResult<()> {
    let mut file = File::create(path).map_err(io_detail)?;
    for entry in entries {
        serde_json::to_writer(&mut file, entry).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(io_detail)?;
    }
    file.sync_all().map_err(io_detail)?;
    Ok(())
}

fn write_json_atomic(path: &Path, value: &Value) -> DrillResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(io_detail)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_detail)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(io_detail)?;
    file.sync_all().map_err(io_detail)?;
    fs::rename(&temporary, path).map_err(io_detail)
}

fn write_artifact_manifest(root: &Path, artifacts_root: &Path) -> DrillResult<Value> {
    let mut entries = Vec::new();
    collect_files(root, root, artifacts_root, &mut entries)?;
    entries.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest = json!({
        "schema_version": "threeterm.reliability.artifacts/1",
        "test": RELIABILITY_TEST_ID,
        "artifacts": entries,
    });
    write_json_atomic(&artifacts_root.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

fn collect_files(
    root: &Path,
    current: &Path,
    artifacts_root: &Path,
    entries: &mut Vec<Value>,
) -> DrillResult<()> {
    for entry in fs::read_dir(current).map_err(io_detail)? {
        let path = entry.map_err(io_detail)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(io_detail)?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "symlink retained in reliability evidence: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, artifacts_root, entries)?;
        } else if metadata.is_file() && path != artifacts_root.join("manifest.json") {
            let bytes = fs::read(&path).map_err(io_detail)?;
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            entries.push(json!({
                "path": relative,
                "bytes": bytes.len(),
                "sha256": sha256(&bytes),
            }));
        }
    }
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> DrillResult<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(format!("artifact path escapes evidence root: {relative}"));
    }
    let candidate = root.join(path);
    let mut current = root.to_path_buf();
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            current.push(name);
            if fs::symlink_metadata(&current)
                .map_err(io_detail)?
                .file_type()
                .is_symlink()
            {
                return Err(format!("symlink escapes reliability root: {relative}"));
            }
        }
    }
    let canonical_root = fs::canonicalize(root).map_err(io_detail)?;
    let canonical_candidate = fs::canonicalize(&candidate).map_err(io_detail)?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(format!("artifact path escapes evidence root: {relative}"));
    }
    Ok(candidate)
}

fn require_real_directory(path: &Path) -> DrillResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_detail)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "expected real evidence directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> DrillResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_detail)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "expected regular evidence file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn read_json(path: &Path) -> DrillResult<Value> {
    serde_json::from_slice(&fs::read(path).map_err(io_detail)?).map_err(|error| error.to_string())
}

fn require(condition: bool, label: &str) -> DrillResult<()> {
    condition
        .then_some(())
        .ok_or_else(|| format!("reliability evidence failed: {label}"))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn io_detail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

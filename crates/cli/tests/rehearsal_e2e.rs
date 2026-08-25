use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{REHEARSE_REQUEST_SCHEMA, REHEARSE_RESPONSE_SCHEMA, find};
use threeterm_protocol::schema_validator::validate;

fn temp_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-rehearsal-{label}-{suffix}"))
}

fn run_rehearsal(output_dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "--machine",
            "rehearse",
            "--output-dir",
            output_dir.to_str().expect("output path is UTF-8"),
            "--release-candidate",
            "rc-1",
        ])
        .output()
        .expect("rehearsal process runs")
}

fn files(root: &Path, prefix: &str, output: &mut Vec<String>) {
    let mut entries = fs::read_dir(root)
        .expect("artifact root reads")
        .map(|entry| entry.expect("artifact entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        let name = path.file_name().expect("artifact name").to_str().unwrap();
        let relative = format!("{prefix}/{name}");
        if path.is_dir() {
            files(&path, &relative, output);
        } else {
            output.push(relative);
        }
    }
}

fn hash(root: &Path, relative: &str) -> (u64, String) {
    let bytes = fs::read(root.join(relative)).expect("catalog artifact reads");
    (bytes.len() as u64, format!("{:x}", Sha256::digest(bytes)))
}

#[test]
fn rehearsal_runs_the_production_l_bracket_pipeline_and_catalogs_every_artifact() {
    if OcctWorker::locate().is_err() {
        eprintln!("rehearsal_e2e: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let output_dir = temp_root("happy");
    let output = run_rehearsal(&output_dir);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).expect("report is JSON");
    let entry = find(threeterm_protocol::schema::REHEARSE_COMMAND_ID).expect("rehearse registry");
    validate(&REHEARSE_RESPONSE_SCHEMA, &report).expect("report validates");
    validate(
        &REHEARSE_REQUEST_SCHEMA,
        &serde_json::json!({
            "output_dir": output_dir,
            "release_candidate": "rc-1"
        }),
    )
    .expect("request validates");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.rehearse.response/1"
    );
    assert_eq!(report["run_count"], 1);
    assert_eq!(report["promoted"], false);
    assert_eq!(report["fixture"], "l-bracket");
    assert_eq!(
        report["timings"].as_array().unwrap().len(),
        9,
        "one band per production rehearsal stage"
    );
    for timing in report["timings"].as_array().unwrap() {
        assert_eq!(timing["sample_count"], 1);
        assert_eq!(timing["unit"], "ms");
        assert_eq!(timing["samples_ms"].as_array().unwrap().len(), 1);
        assert_eq!(timing["p50_ms"], timing["p95_ms"]);
        assert_eq!(timing["p95_ms"], timing["p99_ms"]);
    }

    let project = output_dir.join("project");
    let export = output_dir.join("export");
    let loaded = Bundle::at(&project)
        .open()
        .expect("rehearsal project reloads");
    assert!(project.join("manifest.json").is_file());
    assert!(project.join("transactions.log").is_file());
    assert!(project.join("brep/l-bracket.brep").is_file());
    assert!(export.join("l-bracket.stl").is_file());
    assert!(export.join("l-bracket.3mf").is_file());
    assert!(export.join("l-bracket.step").is_file());
    assert!(
        fs::read_to_string(project.join("transactions.log"))
            .unwrap()
            .contains("65.00000000000000000"),
        "the edit dimensions are canonical"
    );
    assert_eq!(loaded.revision_hash_hex().len(), 64);

    let catalog_path = output_dir.join("sha256-manifest.json");
    assert!(catalog_path.is_file());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&catalog_path).unwrap()).unwrap(),
        report
    );
    let mut actual = Vec::new();
    files(&project, "project", &mut actual);
    if output_dir.join("project.previous-generation").is_dir() {
        files(
            &output_dir.join("project.previous-generation"),
            "project.previous-generation",
            &mut actual,
        );
    }
    files(&export, "export", &mut actual);
    actual.sort();
    let cataloged = report["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|artifact| artifact["relative_path"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(cataloged, actual);
    for artifact in report["artifacts"].as_array().unwrap() {
        let relative = artifact["relative_path"].as_str().unwrap();
        let (bytes, sha256) = hash(&output_dir, relative);
        assert_eq!(artifact["bytes"].as_u64(), Some(bytes));
        assert_eq!(artifact["sha256"].as_str(), Some(sha256.as_str()));
    }
    let top_level = fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(top_level.contains("project"));
    assert!(top_level.contains("export"));
    assert!(top_level.contains("sha256-manifest.json"));
    assert!(top_level.iter().all(|name| matches!(
        name.as_str(),
        "project" | "project.previous-generation" | "export" | "sha256-manifest.json"
    )));
    assert!(
        !actual
            .iter()
            .any(|path| path.contains(".tmp") || path.contains("stage"))
    );
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn failed_export_reports_a_structured_diagnostic_and_keeps_the_project_reloadable() {
    if OcctWorker::locate().is_err() {
        eprintln!("rehearsal_e2e: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let output_dir = temp_root("export-failure");
    fs::create_dir_all(&output_dir).expect("output root creates");
    fs::write(output_dir.join("export"), b"not-a-directory").expect("export sentinel writes");
    let output = run_rehearsal(&output_dir);
    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "rehearsal_failure");
    assert_eq!(diagnostic["stage"], "export");
    assert!(diagnostic["current_revision"].as_str().unwrap().len() == 64);
    assert!(diagnostic["detail"].is_object());
    assert!(!output_dir.join("sha256-manifest.json").exists());
    let project = output_dir.join("project");
    let before_manifest = fs::read(project.join("manifest.json")).expect("manifest remains");
    let before_log = fs::read(project.join("transactions.log")).expect("log remains");
    let before_brep = fs::read(project.join("brep/l-bracket.brep")).expect("brep remains");
    let loaded = Bundle::at(&project)
        .open()
        .expect("failed run remains reloadable");
    assert_eq!(
        loaded.revision_hash_hex(),
        diagnostic["current_revision"].as_str().unwrap()
    );
    assert_eq!(
        fs::read(project.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(
        fs::read(project.join("transactions.log")).unwrap(),
        before_log
    );
    assert_eq!(
        fs::read(project.join("brep/l-bracket.brep")).unwrap(),
        before_brep
    );
    let _ = fs::remove_dir_all(output_dir);
}

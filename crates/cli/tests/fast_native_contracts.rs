use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_persistence::Bundle;

mod common;

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-fast-native-contract-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

#[test]
fn boolean_fuse_cli_preserves_the_worker_operation_and_artifact_binding() {
    if common::skip_shell_fixture_contract_in_real_worker_tier(
        "boolean_fuse_cli_preserves_the_worker_operation_and_artifact_binding",
    ) {
        return;
    }
    let fixture = common::install_occt_fixture();
    let project = root("boolean-fuse");
    Bundle::create(&project).expect("bundle creates");
    common::extrude_canonical_with_worker(
        &project,
        "base",
        serde_json::json!([[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]]),
        1.0,
        &fixture.worker(),
    );
    common::extrude_canonical_with_worker(
        &project,
        "tool",
        serde_json::json!([[1.0, 0.0], [3.0, 0.0], [1.0, 2.0]]),
        1.0,
        &fixture.worker(),
    );
    let source_revision = Bundle::at(&project)
        .open()
        .expect("bundle opens before fuse")
        .revision_hash_hex()
        .to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .env("THREETERM_OCCTBUILD_WORKER", fixture.path())
        .args([
            "--machine",
            "boolean-fuse",
            "--bundle",
            project.to_str().expect("project path is utf-8"),
            "--feature-id",
            "fused",
            "--base",
            "base",
            "--tool",
            "tool",
        ])
        .output()
        .expect("CLI runs");
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("response is JSON");
    assert_eq!(response["operation"], "boolean-fuse");
    assert_eq!(response["feature_id"], "fused");
    assert!(project.join("brep/fused.brep").is_file());

    let request = fixture.last_request();
    assert_eq!(request["command_id"], "boolean_fuse");
    assert_eq!(request["args"]["feature_id"], "fused");
    assert!(
        request["args"]["base_path"]
            .as_str()
            .expect("base path is present")
            .ends_with("/brep/base.brep")
    );
    assert!(
        request["args"]["tool_path"]
            .as_str()
            .expect("tool path is present")
            .ends_with("/brep/tool.brep")
    );
    assert_eq!(
        request["args"]["artifact_request"]["source_revision_id"],
        source_revision
    );

    let original = fs::read(project.join("brep/fused.brep")).expect("fused BREP reads");
    fs::remove_file(project.join("brep/fused.brep")).expect("derived BREP removes");
    let replayed = Host::new()
        .reload_and_recompute_geometry(&project, &fixture.worker())
        .expect("fused BREP replays");
    assert!(replayed.feature_ids.contains(&"fused".to_string()));
    assert_eq!(
        fs::read(project.join("brep/fused.brep")).expect("replayed BREP reads"),
        original
    );

    let _ = fs::remove_dir_all(project);
}

#[test]
fn boolean_fuse_cli_reports_a_fixture_worker_failure_without_mutating_history() {
    if common::skip_shell_fixture_contract_in_real_worker_tier(
        "boolean_fuse_cli_reports_a_fixture_worker_failure_without_mutating_history",
    ) {
        return;
    }
    let fixture = common::install_occt_fixture();
    let project = root("boolean-fuse-failure");
    Bundle::create(&project).expect("bundle creates");
    common::extrude_canonical_with_worker(
        &project,
        "base",
        serde_json::json!([[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]]),
        1.0,
        &fixture.worker(),
    );
    common::extrude_canonical_with_worker(
        &project,
        "tool",
        serde_json::json!([[1.0, 0.0], [3.0, 0.0], [1.0, 2.0]]),
        1.0,
        &fixture.worker(),
    );
    let manifest = fs::read(project.join("manifest.json")).expect("manifest reads");
    let log = fs::read(project.join("transactions.log")).expect("log reads");
    let failure = common::install_occt_failure_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .env("THREETERM_OCCTBUILD_WORKER", failure.path())
        .args([
            "--machine",
            "boolean-fuse",
            "--bundle",
            project.to_str().expect("project path is utf-8"),
            "--feature-id",
            "failed-fuse",
            "--base",
            "base",
            "--tool",
            "tool",
        ])
        .output()
        .expect("CLI runs");
    assert!(
        !output.status.success(),
        "fixture failure must reach the CLI"
    );
    let diagnostic: Value = serde_json::from_slice(&output.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "brep_invalid");
    assert!(
        diagnostic["arg"]
            .as_str()
            .expect("diagnostic detail is text")
            .contains("fixture rejects this request")
    );
    assert_eq!(failure.last_request()["command_id"], "boolean_fuse");
    assert_eq!(fs::read(project.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(project.join("transactions.log")).unwrap(), log);
    assert!(!project.join("brep/failed-fuse.brep").exists());

    let _ = fs::remove_dir_all(project);
}

#[test]
fn boolean_cli_commands_preserve_their_distinct_worker_operation_names() {
    if common::skip_shell_fixture_contract_in_real_worker_tier(
        "boolean_cli_commands_preserve_their_distinct_worker_operation_names",
    ) {
        return;
    }
    let fixture = common::install_occt_fixture();
    let project = root("boolean-operation-names");
    Bundle::create(&project).expect("bundle creates");
    for (feature_id, profile) in [
        (
            "base",
            serde_json::json!([[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]]),
        ),
        (
            "tool",
            serde_json::json!([[1.0, 0.0], [3.0, 0.0], [1.0, 2.0]]),
        ),
    ] {
        common::extrude_canonical_with_worker(
            &project,
            feature_id,
            profile,
            1.0,
            &fixture.worker(),
        );
    }

    for (command, worker_operation) in [
        ("boolean-fuse", "boolean_fuse"),
        ("boolean-cut", "boolean_cut"),
        ("boolean-common", "boolean_common"),
    ] {
        let feature_id = format!("{worker_operation}-result");
        let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
            .env("THREETERM_OCCTBUILD_WORKER", fixture.path())
            .args([
                "--machine",
                command,
                "--bundle",
                project.to_str().expect("project path is utf-8"),
                "--feature-id",
                &feature_id,
                "--base",
                "base",
                "--tool",
                "tool",
            ])
            .output()
            .expect("CLI runs");
        assert!(
            output.status.success(),
            "{command} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).expect("response is JSON");
        assert_eq!(response["operation"], command);
        assert_eq!(response["feature_id"], feature_id);
    }

    let operations: Vec<_> = fixture
        .requests()
        .into_iter()
        .map(|request| request["command_id"].as_str().unwrap().to_string())
        .collect();
    assert!(operations.ends_with(&[
        "boolean_fuse".to_string(),
        "boolean_cut".to_string(),
        "boolean_common".to_string(),
    ]));

    let _ = fs::remove_dir_all(project);
}

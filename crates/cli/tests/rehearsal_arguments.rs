use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn temporary_output(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-adversarial-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

#[test]
fn rehearsal_requires_an_output_directory_and_release_candidate() {
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "rehearse", "--output-dir", "/tmp/rehearsal"])
        .output()
        .expect("rehearse process runs");

    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "rehearsal_failure");
    assert_eq!(diagnostic["stage"], "argument_parse");
    assert_eq!(diagnostic["current_revision"], Value::Null);
}

#[test]
fn rehearsal_validates_empty_output_directory_through_the_registered_schema() {
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "--machine",
            "rehearse",
            "--output-dir",
            "",
            "--release-candidate",
            "rc-1",
        ])
        .output()
        .expect("rehearse process runs");

    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "rehearsal_failure");
    assert_eq!(diagnostic["stage"], "argument_parse");
    assert!(
        diagnostic["detail"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("output_dir")
    );
}

#[test]
fn rehearsal_validates_empty_release_candidate_through_the_registered_schema() {
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "--machine",
            "rehearse",
            "--output-dir",
            "/tmp/rehearsal",
            "--release-candidate",
            "",
        ])
        .output()
        .expect("rehearse process runs");

    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "rehearsal_failure");
    assert_eq!(diagnostic["stage"], "argument_parse");
    assert!(
        diagnostic["detail"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("release_candidate")
    );
}

#[test]
fn adversarial_run_rejects_unknown_cases_before_creating_evidence() {
    let output_dir = temporary_output("invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "run",
            "lbracket",
            "--adversarial=unknown",
            "--output-dir",
            output_dir.to_str().expect("output path is UTF-8"),
        ])
        .output()
        .expect("adversarial process runs");

    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "rehearsal_failure");
    assert_eq!(diagnostic["stage"], "argument_parse");
    assert!(!output_dir.exists());
}

#[test]
fn adversarial_run_schema_case_is_demoable_through_the_production_binary() {
    let output_dir = temporary_output("schema");
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "run",
            "lbracket",
            "--adversarial=schema-v0",
            "--output-dir",
            output_dir.to_str().expect("output path is UTF-8"),
        ])
        .output()
        .expect("adversarial process runs");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("report is JSON");
    assert_eq!(
        report["report"]["diagnostic"]["code"],
        "SCHEMA_EPOCH_V0_REQUIRES_BACKUP"
    );
    assert!(output_dir.join("schema-v0/report.json").is_file());
    assert!(output_dir.join("sha256-manifest.json").is_file());
    let _ = std::fs::remove_dir_all(output_dir);
}

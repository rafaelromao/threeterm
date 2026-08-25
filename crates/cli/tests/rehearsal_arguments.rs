use std::process::Command;

use serde_json::Value;

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

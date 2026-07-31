//! Subprocess integration tests for the structured `unknown_command` diagnostic.

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm binary runs")
}

fn parse_stderr(output: &std::process::Output) -> Value {
    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr is utf-8");
    serde_json::from_str(&stderr).expect("stderr is parseable JSON")
}

use serde_json::Value;

#[test]
fn threeterm_bogus_writes_unknown_command_diagnostic() {
    let output = run(&["bogus"]);

    assert!(
        !output.status.success(),
        "non-zero exit on unknown subcommand"
    );
    assert!(
        output.stdout.is_empty(),
        "stdout must be empty on diagnostic, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let parsed = parse_stderr(&output);
    assert_eq!(parsed["code"], "unknown_command");
    assert_eq!(parsed["arg"], "bogus");
    assert_eq!(
        parsed["schema_version"],
        Value::from(threeterm_protocol::schema_version())
    );

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn threeterm_with_no_args_writes_unknown_command_diagnostic() {
    let output = run(&[]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let parsed = parse_stderr(&output);
    assert_eq!(parsed["code"], "unknown_command");
    assert_eq!(parsed["arg"], "");
    assert_eq!(output.status.code(), Some(2));
}

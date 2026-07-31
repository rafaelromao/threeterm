//! Asserts that every production subcommand's output validates against
//! the versioned response schema registered in `protocol::schema`.
//!
//! The four new subcommands (`identity`, `load`, `apply`) plus the
//! `new-project` subcommand are exercised end-to-end so the schema
//! layer participates in the same runnable path as the production code.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, IDENTITY_COMMAND_ID, LOAD_COMMAND_ID, NEW_PROJECT_COMMAND_ID, find,
};
use threeterm_protocol::schema_validator::validate;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_threeterm")
}

fn fresh_bundle_root(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "threeterm-schema-validation-{name}-{nanos}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn s(value: &str) -> OsString {
    OsString::from(value)
}

fn run(args: &[OsString]) -> std::process::Output {
    let bin = bin();
    Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm binary runs")
}

fn parse_stdout(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is utf-8");
    serde_json::from_str(&stdout).expect("stdout is parseable JSON")
}

#[test]
fn new_project_output_validates_against_registered_response_schema() {
    let dir = fresh_bundle_root("new-project");
    let output = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(
        output.status.success(),
        "new-project failed: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_stdout(&output);
    let entry = find(NEW_PROJECT_COMMAND_ID).expect("new-project registered");
    validate(&entry.response_schema, &parsed).expect("new-project output validates");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn identity_output_validates_against_registered_response_schema() {
    let dir = fresh_bundle_root("identity");
    let setup = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(setup.status.success());
    let output = run(&[s("identity"), dir.clone().into_os_string()]);
    assert!(
        output.status.success(),
        "identity failed: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_stdout(&output);
    let entry = find(IDENTITY_COMMAND_ID).expect("identity registered");
    validate(&entry.response_schema, &parsed).expect("identity output validates");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn load_output_validates_against_registered_response_schema() {
    let dir = fresh_bundle_root("load");
    let setup = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(setup.status.success());
    let output = run(&[s("load"), dir.clone().into_os_string()]);
    assert!(
        output.status.success(),
        "load failed: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_stdout(&output);
    let entry = find(LOAD_COMMAND_ID).expect("load registered");
    validate(&entry.response_schema, &parsed).expect("load output validates");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn apply_output_validates_against_registered_response_schema() {
    let dir = fresh_bundle_root("apply");
    let setup = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(setup.status.success());
    let output = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(
            r#"{"kind":"add-feature","feature_id":"feat-1","feature_kind":"sketch","parameters":{}}"#,
        ),
    ]);
    assert!(
        output.status.success(),
        "apply failed: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_stdout(&output);
    let entry = find(APPLY_COMMAND_ID).expect("apply registered");
    validate(&entry.response_schema, &parsed).expect("apply output validates");
    let _ = fs::remove_dir_all(&dir);
}

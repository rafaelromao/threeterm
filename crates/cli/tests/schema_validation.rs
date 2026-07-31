//! Asserts that the production `--machine list` output validates against
//! the versioned response schema registered in `protocol::schema`.

use std::process::Command;

use serde_json::Value;
use threeterm_protocol::schema::{LIST_COMMAND_ID, LIST_RESPONSE_SCHEMA, find};
use threeterm_protocol::schema_validator::validate;

#[test]
fn threeterm_machine_list_output_validates_against_registered_response_schema() {
    let bin = env!("CARGO_BIN_EXE_threeterm");

    let output = Command::new(bin)
        .arg("--machine")
        .arg("list")
        .output()
        .expect("threeterm binary runs");

    assert!(
        output.status.success(),
        "expected success exit, got {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout is parseable JSON");

    let entry = find(LIST_COMMAND_ID).expect("`list` is registered");
    validate(&entry.response_schema, &parsed)
        .expect("production output must satisfy the registered response schema");
    validate(&LIST_RESPONSE_SCHEMA, &parsed)
        .expect("the explicit response schema constant must also accept the output");
}

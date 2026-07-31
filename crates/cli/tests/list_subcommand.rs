//! Subprocess integration test for `threeterm --machine list`.
//!
//! Invokes the compiled `threeterm` binary via `CARGO_BIN_EXE_threeterm`
//! (set by Cargo for integration tests) and asserts the demoable behavior
//! end-to-end on the production code path.

use std::process::Command;

use serde_json::Value;

#[test]
fn threeterm_machine_list_prints_top_level_json_array_to_stdout() {
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
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout is parseable JSON");

    let commands = parsed
        .as_array()
        .expect("dispatch output is a top-level JSON array");
    assert_eq!(
        commands.len(),
        1,
        "exactly one entry in the seeded registry"
    );
    assert_eq!(commands[0]["id"], "list");
    assert_eq!(commands[0]["name"], "list");
    assert_eq!(commands[0]["schema_version"], "threeterm.command.list/1");
    assert_eq!(
        commands[0]["request_schema_version"],
        "threeterm.command.list.request/1"
    );
    assert_eq!(
        commands[0]["response_schema_version"],
        "threeterm.command.list.response/1"
    );
    assert!(commands[0]["request_schema"].is_object());
    assert!(commands[0]["response_schema"].is_object());
}

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
    assert_eq!(commands.len(), 7, "seven registered commands");
    let list = commands
        .iter()
        .find(|command| command["id"] == "list")
        .expect("list command is registered");
    assert_eq!(list["name"], "list");
    assert_eq!(list["schema_version"], "threeterm.command.list/1");
    assert_eq!(
        list["request_schema_version"],
        "threeterm.command.list.request/1"
    );
    assert_eq!(
        list["response_schema_version"],
        "threeterm.command.list.response/1"
    );
    assert!(list["request_schema"].is_object());
    assert!(list["response_schema"].is_object());

    let define = commands
        .iter()
        .find(|command| command["id"] == "define-component")
        .expect("define-component command is registered");
    assert_eq!(define["name"], "define-component");
    assert_eq!(
        define["schema_version"],
        "threeterm.command.define-component/1"
    );

    let place = commands
        .iter()
        .find(|command| command["id"] == "place-instance")
        .expect("place-instance command is registered");
    assert_eq!(place["name"], "place-instance");
    assert_eq!(
        place["schema_version"],
        "threeterm.command.place-instance/1"
    );

    let transform = commands
        .iter()
        .find(|command| command["id"] == "transform-instance")
        .expect("transform-instance command is registered");
    assert_eq!(transform["name"], "transform-instance");
    assert_eq!(
        transform["schema_version"],
        "threeterm.command.transform-instance/1"
    );

    let copy = commands
        .iter()
        .find(|command| command["id"] == "independent-copy")
        .expect("independent-copy command is registered");
    assert_eq!(copy["name"], "independent-copy");
    assert_eq!(
        copy["schema_version"],
        "threeterm.command.independent-copy/1"
    );

    let edit = commands
        .iter()
        .find(|command| command["id"] == "edit-parameter")
        .expect("edit-parameter command is registered");
    assert_eq!(edit["name"], "edit-parameter");
    assert_eq!(edit["schema_version"], "threeterm.command.edit-parameter/1");
}

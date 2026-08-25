//! Production CLI/MCP tracer bullet for reusable component commands.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_occt_worker::OcctWorker;

fn bundle() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-components-{nonce}"))
}

fn cli() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_threeterm")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/threeterm")
        })
}

fn mcp() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_threeterm_mcp")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/threeterm-mcp")
        })
}

fn cli_command(command: &str, root: &PathBuf, args: &[&str]) -> Value {
    let output = Command::new(cli())
        .args(["--machine", command])
        .arg(root)
        .args(args)
        .output()
        .expect("CLI starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI returns JSON")
}

fn cli_failure(command: &str, root: &PathBuf, args: &[&str]) -> Value {
    let output = Command::new(cli())
        .args(["--machine", command])
        .arg(root)
        .args(args)
        .output()
        .expect("CLI starts");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).expect("CLI returns a diagnostic")
}

fn mcp_command(name: &str, arguments: Value) -> Value {
    let mut child = Command::new(mcp())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("MCP starts");
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":name,"arguments":arguments}});
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("{request}\n").as_bytes())
        .expect("write");
    let output = child.wait_with_output().expect("MCP exits");
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).expect("MCP returns JSON");
    assert!(response["error"].is_null(), "{response}");
    response["result"]["structuredContent"].clone()
}

fn mcp_response(name: &str, arguments: Value) -> Value {
    let mut child = Command::new(mcp())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("MCP starts");
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":name,"arguments":arguments}});
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("{request}\n").as_bytes())
        .expect("write");
    let output = child.wait_with_output().expect("MCP exits");
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).expect("MCP returns JSON")
}

#[test]
fn reusable_component_survives_cli_mcp_copy_edit_and_reopen() {
    let root = bundle();
    let root_text = root.to_string_lossy().into_owned();
    cli_command(
        "define-component",
        &root,
        &[
            "--definition-id",
            "bracket",
            "--feature-id",
            "bracket-feature",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    cli_command(
        "create-component-instance",
        &root,
        &[
            "--instance-id",
            "first",
            "--definition-id",
            "bracket",
            "--transform",
            "0,0,0",
        ],
    );
    mcp_command(
        "threeterm.command.create-component-instance/1",
        json!({"bundle_path":root_text,"instance_id":"second","definition_id":"bracket","transform":[10.0,0.0,0.0]}),
    );
    let before = cli_command("component-state", &root, &[]);
    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(root.join("transactions.log")).expect("log reads");
    let invalid = mcp_response(
        "threeterm.command.transform-component-instance/1",
        json!({"bundle_path":root.to_string_lossy(),"instance_id":"missing","transform":[0.0,0.0,90.0]}),
    );
    assert_eq!(invalid["error"]["code"], -32603);
    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads"),
        manifest_before
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("log reads"),
        log_before
    );
    mcp_command(
        "threeterm.command.transform-component-instance/1",
        json!({"bundle_path":root.to_string_lossy(),"instance_id":"second","transform":[0.0,0.0,90.0]}),
    );
    mcp_command(
        "threeterm.command.make-component-independent/1",
        json!({"bundle_path":root.to_string_lossy(),"source_instance_id":"second","definition_id":"copy","instance_id":"copy-instance","feature_id":"copy-feature"}),
    );
    mcp_command(
        "threeterm.command.edit-component-parameter/1",
        json!({"bundle_path":root.to_string_lossy(),"definition_id":"copy","parameter":"length","value":75.0}),
    );
    let after = cli_command("component-state", &root, &[]);
    assert_eq!(
        before["definitions"]["bracket"],
        after["definitions"]["bracket"]
    );
    assert_eq!(before["instances"]["first"], after["instances"]["first"]);
    assert_eq!(
        after["instances"]["second"]["transform"],
        json!([0.0, 0.0, 90.0])
    );
    assert_eq!(
        after["instances"]["copy-instance"]["transform"],
        json!([0.0, 0.0, 90.0])
    );
    let reopened = cli_command("component-state", &root, &[]);
    assert_eq!(reopened, after);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn captured_definition_survives_recompute_and_is_reusable() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = bundle();
    let root_text = root.to_string_lossy().into_owned();
    cli_command(
        "bracket",
        &root,
        &[
            "--bracket-id",
            "l-bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    cli_command(
        "capture-component",
        &root,
        &[
            "--definition-id",
            "captured-bracket",
            "--feature-id",
            "l-bracket-base",
            "--feature-id",
            "l-bracket-bend",
            "--feature-id",
            "l-bracket-finish",
            "--feature-id",
            "l-bracket-independent-base",
        ],
    );
    let captured = cli_command("component-state", &root, &[]);
    assert_eq!(
        captured["definitions"]["captured-bracket"]["selected_feature_ids"],
        json!([
            "l-bracket-base",
            "l-bracket-bend",
            "l-bracket-finish",
            "l-bracket-independent-base"
        ])
    );
    assert_eq!(
        captured["definitions"]["captured-bracket"]["descriptor"]["length"],
        json!(60.0)
    );

    cli_command(
        "historical-edit",
        &root,
        &[
            "--feature-id",
            "l-bracket-base",
            "--parameter",
            "length",
            "--value",
            "75",
        ],
    );
    mcp_command(
        "threeterm.command.create-component-instance/1",
        json!({
            "bundle_path": root_text,
            "instance_id": "captured-instance",
            "definition_id": "captured-bracket",
            "transform": [12.0, 0.0, 90.0]
        }),
    );

    let after_recompute = cli_command("component-state", &root, &[]);
    assert_eq!(
        after_recompute["definitions"]["captured-bracket"],
        captured["definitions"]["captured-bracket"]
    );
    assert_eq!(
        after_recompute["instances"]["captured-instance"]["definition_id"],
        json!("captured-bracket")
    );
    assert_eq!(
        cli_command("component-state", &root, &[]),
        after_recompute,
        "reopening the canonical bundle preserves the captured definition"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_capture_preserves_the_canonical_bundle() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = bundle();
    cli_command(
        "bracket",
        &root,
        &[
            "--bracket-id",
            "l-bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(root.join("transactions.log")).expect("log reads");
    let diagnostic = cli_failure(
        "capture-component",
        &root,
        &[
            "--definition-id",
            "invalid-capture",
            "--feature-id",
            "l-bracket-finish",
        ],
    );
    assert_eq!(diagnostic["code"], json!("invalid_request"));
    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads"),
        manifest_before
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("log reads"),
        log_before
    );
    let _ = std::fs::remove_dir_all(root);
}

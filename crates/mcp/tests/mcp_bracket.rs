//! Subprocess integration test for the `threeterm-mcp` binary.
//!
//! Spawns the compiled `threeterm-mcp` binary via `CARGO_BIN_EXE_threeterm_mcp`
//! (set by Cargo for integration tests), writes newline-framed JSON-RPC 2.0
//! envelopes to its stdin, reads newline-framed responses from its stdout,
//! and asserts the demoable behavior end-to-end on the production code path.
//!
//! The subprocess fixture demonstrates that:
//! - `tools/list` advertises every registered command from the protocol
//!   registry with `name == <entry.schema_version>` plus populated
//!   `inputSchema` / `outputSchema`.
//! - `tools/call` to `threeterm.command.bracket/1` produces a result
//!   identical to the CLI invocation for the same input.
//! - `tools/call` rejects invalid arguments against the registered request
//!   schema with `code: -32602` and preserves canonical host state.
//! - `tools/call` rejects unknown tool names with `code: -32601`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_occt_worker::OcctWorker;

fn fresh_bundle(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-mcp-{label}-{suffix}"))
}

fn threeterm_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_threeterm") {
        return PathBuf::from(path);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("..")
        .join("..")
        .join("target")
        .join("debug")
        .join("threeterm")
}

fn threeterm_mcp_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_threeterm_mcp") {
        return PathBuf::from(path);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("..")
        .join("..")
        .join("target")
        .join("debug")
        .join("threeterm-mcp")
}

fn run_mcp(requests: &[Value]) -> Vec<Value> {
    let bin = threeterm_mcp_binary();
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("threeterm-mcp binary runs");

    let mut stdin = child.stdin.take().expect("stdin is captured");
    for request in requests {
        let mut bytes = serde_json::to_vec(request).expect("request serializes");
        bytes.push(b'\n');
        stdin.write_all(&bytes).expect("stdin write");
    }
    drop(stdin);

    let output = child.wait_with_output().expect("mcp process completes");
    assert!(
        output.status.success(),
        "mcp process exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut responses = Vec::new();
    for line in output.stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let parsed: Value = serde_json::from_slice(line).expect("response is JSON");
        responses.push(parsed);
    }
    responses
}

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: ChildStderr,
}

impl McpSession {
    fn spawn() -> Self {
        let mut child = Command::new(threeterm_mcp_binary())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("threeterm-mcp session runs");
        Self {
            stdin: child.stdin.take().expect("session stdin is captured"),
            stdout: BufReader::new(child.stdout.take().expect("session stdout is captured")),
            stderr: child.stderr.take().expect("session stderr is captured"),
            child,
        }
    }

    fn send(&mut self, request: &Value) -> Value {
        let mut bytes = serde_json::to_vec(request).expect("request serializes");
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .expect("session request writes");
        self.stdin.flush().expect("session request flushes");
        let mut line = Vec::new();
        self.stdout
            .read_until(b'\n', &mut line)
            .expect("session response reads");
        serde_json::from_slice(&line).expect("session response is JSON")
    }

    fn finish(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("mcp session completes");
        let mut stderr = Vec::new();
        self.stderr
            .read_to_end(&mut stderr)
            .expect("session stderr reads");
        assert!(
            status.success(),
            "mcp session exited non-zero: stderr={}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(
            stderr.is_empty(),
            "stderr must be empty on success: {}",
            String::from_utf8_lossy(&stderr)
        );
    }
}

fn structured(responses: &[Value], index: usize) -> &Value {
    &responses[index]["result"]["structuredContent"]
}

#[test]
fn initialize_returns_protocol_version_server_info_and_capabilities() {
    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "fixture", "version": "0"}
        }
    })]);

    assert_eq!(responses.len(), 1);
    let response = &responses[0];
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert_eq!(result["serverInfo"]["name"], "threeterm-mcp");
    assert!(result["serverInfo"]["version"].is_string());
    assert!(
        result["capabilities"]["tools"].is_object(),
        "initialize must declare tools capability"
    );
}

#[test]
fn tools_list_advertises_every_registered_command_with_populated_schemas() {
    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
    })]);

    assert_eq!(responses.len(), 1);
    let response = &responses[0];
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools is an array");
    let bracket = tools
        .iter()
        .find(|tool| tool["name"] == "threeterm.command.bracket/1")
        .expect("bracket is advertised");
    assert!(bracket["inputSchema"].is_object());
    assert!(bracket["outputSchema"].is_object());
    let required = bracket["inputSchema"]["required"]
        .as_array()
        .expect("input schema declares required");
    let required_keys: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
    for key in [
        "bundle_path",
        "bracket_id",
        "length",
        "width",
        "height",
        "thickness",
    ] {
        assert!(
            required_keys.contains(&key),
            "advertised input schema must require {key:?}"
        );
    }
}

#[test]
fn tools_call_to_bracket_produces_a_result_identical_to_the_cli_invocation() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let cli_root = fresh_bundle("happy-cli");
    let mcp_root = fresh_bundle("happy-mcp");

    let cli = Command::new(threeterm_binary())
        .args(["--machine", "bracket"])
        .arg(&cli_root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("cli bracket process runs");
    assert!(
        cli.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let cli: Value = serde_json::from_slice(&cli.stdout).expect("cli response is JSON");

    let mcp_responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.bracket/1",
            "arguments": {
                "bundle_path": mcp_root.to_string_lossy(),
                "bracket_id": "l-1",
                "length": 60.0,
                "width": 30.0,
                "height": 40.0,
                "thickness": 3.0
            }
        }
    })]);
    assert_eq!(mcp_responses.len(), 1);
    let mcp = &mcp_responses[0];
    assert!(mcp["error"].is_null(), "mcp response must not be an error");
    let structured = &mcp["result"]["structuredContent"];
    assert_eq!(
        structured, &cli,
        "the MCP bracket result must be structurally equal to the CLI bracket result"
    );

    let loaded = Command::new(threeterm_binary())
        .args(["--machine", "load"])
        .arg(&mcp_root)
        .output()
        .expect("load process runs");
    assert!(
        loaded.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    let loaded: Value = serde_json::from_slice(&loaded.stdout).expect("load response is JSON");
    assert_eq!(
        loaded["feature_graph_hash"], cli["feature_graph_hash"],
        "load must report the same feature_graph_hash as the bracket write"
    );
    assert_eq!(
        loaded["revision_hash"], cli["revision_hash"],
        "load must report the same revision_hash as the bracket write"
    );
    assert_eq!(
        loaded["schema_version"], "threeterm.command.load.response/2",
        "load returns its own response-schema version, distinct from bracket's"
    );

    let _ = std::fs::remove_dir_all(cli_root);
    let _ = std::fs::remove_dir_all(mcp_root);
}

#[test]
fn bracket_edit_lifecycle_previews_commits_and_discards_through_mcp() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = fresh_bundle("bracket-edit-lifecycle");
    let seeded = Command::new(threeterm_binary())
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("seed bracket process runs");
    assert!(
        seeded.status.success(),
        "seed bracket stderr: {}",
        String::from_utf8_lossy(&seeded.stderr)
    );
    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(root.join("transactions.log")).expect("log reads");
    let brep_before = std::fs::read(root.join("brep/l-1.brep")).expect("brep reads");
    let call = |id, phase, draft_id, thickness, sequence: Option<u64>| {
        let mut arguments = serde_json::json!({
            "phase": phase,
            "bundle_path": root.to_string_lossy(),
            "draft_id": draft_id,
            "bracket_id": "l-1",
            "length": 60.0,
            "width": 30.0,
            "height": 40.0,
            "thickness": thickness,
        });
        if let Some(sequence) = sequence {
            arguments["draft_sequence"] = sequence.into();
        }
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "threeterm.command.bracket-edit/1", "arguments": arguments }
        })
    };
    let discarded = run_mcp(&[
        call(1, "open", "edit-discard", 3.0, None),
        call(2, "open", "edit-discard", 5.0, None),
        call(3, "preview", "edit-discard", 5.0, None),
        call(4, "preview", "edit-discard", 3.0, None),
        call(5, "discard", "edit-discard", 3.0, None),
    ]);
    assert_eq!(discarded.len(), 5);
    assert_eq!(structured(&discarded, 1)["status"], "rejected");
    assert_eq!(
        structured(&discarded, 1)["diagnostic"]["draft_id"],
        "edit-discard"
    );
    assert_eq!(structured(&discarded, 2)["status"], "rejected");
    assert_eq!(
        structured(&discarded, 2)["diagnostic"]["kind"],
        "draft_input_conflict",
        "duplicate open must expose a structured conflict: {}",
        structured(&discarded, 2)
    );
    assert!(structured(&discarded, 2)["diagnostic"]["source_revision"].is_string());
    assert!(structured(&discarded, 2)["diagnostic"]["current_revision"].is_string());
    assert_eq!(structured(&discarded, 3)["phase"], "preview");
    assert_ne!(
        structured(&discarded, 3)["preview_revision"],
        structured(&discarded, 3)["source_revision"]
    );
    assert_eq!(structured(&discarded, 4)["phase"], "discard");
    assert_eq!(
        std::fs::read(root.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).unwrap(),
        log_before
    );
    assert_eq!(
        std::fs::read(root.join("brep/l-1.brep")).unwrap(),
        brep_before
    );
    let mut session = McpSession::spawn();
    let opened = session.send(&call(4, "open", "edit-commit", 3.0, None));
    let draft_fingerprint = opened["result"]["structuredContent"]["input_fingerprint"]
        .as_str()
        .expect("open returns the draft fingerprint")
        .to_string();
    let mut update = call(5, "update", "edit-commit", 4.0, Some(0));
    update["params"]["arguments"]["input_fingerprint"] = draft_fingerprint.into();
    let committed = vec![
        session.send(&update),
        session.send(&call(6, "preview", "edit-commit", 4.0, None)),
        session.send(&call(7, "commit", "edit-commit", 4.0, None)),
    ];
    session.finish();
    assert_eq!(committed.len(), 3);
    assert_eq!(
        structured(&committed, 0)["draft_sequence"],
        1,
        "update must return the updated draft: {}",
        committed[0]
    );
    assert_eq!(structured(&committed, 1)["phase"], "preview");
    assert_eq!(structured(&committed, 2)["phase"], "commit");
    assert_ne!(
        structured(&committed, 2)["current_revision"],
        structured(&committed, 2)["source_revision"]
    );
    assert_ne!(
        std::fs::read(root.join("brep/l-1.brep")).unwrap(),
        brep_before
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tools_call_rejects_invalid_arguments_with_invalid_params_code() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = fresh_bundle("invalid-args");

    let seeded = Command::new(threeterm_binary())
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("seed bracket process runs");
    assert!(seeded.status.success());

    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let transactions_before =
        std::fs::read(root.join("transactions.log")).expect("transactions log reads");

    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.bracket/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "bracket_id": "l-2",
                "length": "not-a-number",
                "width": 30.0,
                "height": 40.0,
                "thickness": 3.0
            }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let response = &responses[0];
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 9);
    let error = &response["error"];
    assert!(
        !error.is_null(),
        "invalid args must be reported as an error"
    );
    assert_eq!(error["code"], -32602, "schema violation uses -32602");
    let message = error["message"]
        .as_str()
        .expect("error message is a string");
    assert!(
        message.contains("length"),
        "error message must name the offending field; got {message:?}"
    );

    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads after failure"),
        manifest_before,
        "canonical manifest must be unchanged after a rejected tools/call"
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("transactions log after failure"),
        transactions_before,
        "canonical transaction log must be unchanged after a rejected tools/call"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tools_call_rejects_unknown_tool_with_method_not_found_code() {
    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.does-not-exist/1",
            "arguments": {}
        }
    })]);

    assert_eq!(responses.len(), 1);
    let response = &responses[0];
    assert_eq!(
        response["error"]["code"], -32601,
        "unknown tool uses -32601"
    );
    let message = response["error"]["message"]
        .as_str()
        .expect("error message is a string");
    assert!(
        message.contains("does-not-exist"),
        "error message must reference the unknown tool name; got {message:?}"
    );
}

#[test]
fn tools_call_on_tampered_bundle_reports_internal_error_and_preserves_state() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = fresh_bundle("tampered");

    let seeded = Command::new(threeterm_binary())
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("seed bracket process runs");
    assert!(seeded.status.success());

    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).expect("manifest reads"))
            .expect("manifest parses");
    manifest["terminal_log_digest"] = "f".repeat(64).into();
    std::fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");

    let manifest_before =
        std::fs::read(root.join("manifest.json")).expect("manifest reads after tampering");
    let transactions_before =
        std::fs::read(root.join("transactions.log")).expect("transactions log after tampering");

    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.bracket/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "bracket_id": "l-2",
                "length": 60.0,
                "width": 30.0,
                "height": 40.0,
                "thickness": 3.0
            }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let response = &responses[0];
    assert_eq!(
        response["error"]["code"], -32603,
        "host failure uses -32603"
    );

    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads after failure"),
        manifest_before,
        "canonical manifest must be unchanged after a failed tools/call"
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("transactions log after failure"),
        transactions_before,
        "canonical transaction log must be unchanged after a failed tools/call"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tools_call_rejects_empty_bracket_id_violating_min_length_with_invalid_params() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = fresh_bundle("empty-bracket-id");

    let seeded = Command::new(threeterm_binary())
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("seed bracket process runs");
    assert!(seeded.status.success());

    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let transactions_before =
        std::fs::read(root.join("transactions.log")).expect("transactions log reads");

    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.bracket/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "bracket_id": "",
                "length": 60.0,
                "width": 30.0,
                "height": 40.0,
                "thickness": 3.0
            }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let response = &responses[0];
    assert_eq!(response["error"]["code"], -32602);
    let message = response["error"]["message"]
        .as_str()
        .expect("error message is a string");
    assert!(
        message.contains("bracket_id"),
        "error must name the offending field; got {message:?}"
    );

    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads after failure"),
        manifest_before,
        "canonical manifest must be unchanged after a rejected tools/call"
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("transactions log after failure"),
        transactions_before,
        "canonical transaction log must be unchanged after a rejected tools/call"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tools_call_rejects_non_positive_length_violating_minimum_with_invalid_params() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = fresh_bundle("non-positive-length");

    let seeded = Command::new(threeterm_binary())
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("seed bracket process runs");
    assert!(seeded.status.success());

    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let transactions_before =
        std::fs::read(root.join("transactions.log")).expect("transactions log reads");

    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.bracket/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "bracket_id": "l-2",
                "length": 0.0,
                "width": 30.0,
                "height": 40.0,
                "thickness": 3.0
            }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let response = &responses[0];
    assert_eq!(response["error"]["code"], -32602);
    let message = response["error"]["message"]
        .as_str()
        .expect("error message is a string");
    assert!(
        message.contains("length"),
        "error must name the offending field; got {message:?}"
    );

    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads after failure"),
        manifest_before,
        "canonical manifest must be unchanged after a rejected tools/call"
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("transactions log after failure"),
        transactions_before,
        "canonical transaction log must be unchanged after a rejected tools/call"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tools_list_and_call_expose_the_feature_scoped_timeline_contract() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = fresh_bundle("timeline");
    let seeded = Command::new(threeterm_binary())
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("seed bracket process runs");
    assert!(seeded.status.success());

    let listed = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
    })]);
    let timeline_tool = listed[0]["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "threeterm.command.timeline/1")
        .expect("timeline tool is advertised");
    assert!(
        timeline_tool["inputSchema"]["required"]
            .as_array()
            .expect("timeline request fields")
            .iter()
            .any(|field| field == "feature_id")
    );

    let responses = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.timeline/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "feature_id": "l-1-base"
            }
        }
    })]);
    assert_eq!(responses.len(), 1);
    assert!(responses[0]["error"].is_null());
    let timeline = &responses[0]["result"]["structuredContent"];
    assert_eq!(timeline["feature_id"], "l-1-base");
    assert_eq!(timeline["revisions"][0]["ordinal"], 1);
    assert_eq!(timeline["revisions"][0]["status"], "current-valid");

    let named = Command::new(threeterm_binary())
        .args(["--machine", "create-revision"])
        .arg(&root)
        .args(["--name", "before-restore"])
        .output()
        .expect("create revision process runs");
    assert!(named.status.success());
    let restored = run_mcp(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.restore-revision/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "feature_id": "l-1-base",
                "name": "before-restore"
            }
        }
    })]);
    assert_eq!(restored.len(), 1);
    assert!(restored[0]["error"].is_null());
    assert_eq!(
        restored[0]["result"]["structuredContent"]["active_revision"],
        "history-revision-1"
    );

    let _ = std::fs::remove_dir_all(root);
}

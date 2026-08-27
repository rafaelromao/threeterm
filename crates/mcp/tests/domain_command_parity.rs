use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{APPLY_COMMAND_ID, IDENTITY_COMMAND_ID};

fn root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-parity-{label}-{suffix}"))
}

fn apply_request(root: &std::path::Path, revision: &str) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "expected_revision": revision,
        "operation": "add",
        "feature_id": "box",
        "kind": "cube"
    })
}

fn identity_request(root: &std::path::Path) -> Value {
    json!({"bundle_path": root.to_string_lossy()})
}

fn cli_identity(root: &std::path::Path) -> Value {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let path = root.to_string_lossy().into_owned();
    let status = threeterm_cli::dispatch::dispatch(
        ["--machine", "identity", path.as_str()]
            .into_iter()
            .map(OsString::from),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(
        status,
        0,
        "CLI identity failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    serde_json::from_slice(&stdout).expect("CLI identity returns JSON")
}

fn cli_apply(root: &std::path::Path, revision: &str) -> Value {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let path = root.to_string_lossy().into_owned();
    let args = [
        "--machine",
        "apply",
        path.as_str(),
        "--expected-revision",
        revision,
        "--operation",
        "add",
        "--feature-id",
        "box",
        "--kind",
        "cube",
    ]
    .into_iter()
    .map(OsString::from);
    let status = threeterm_cli::dispatch::dispatch(args, &mut stdout, &mut stderr);
    assert_eq!(
        status,
        0,
        "CLI apply failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    stdout.flush().expect("CLI output flushes");
    serde_json::from_slice(&stdout).expect("CLI returns JSON")
}

fn mcp_identity(root: &std::path::Path) -> Value {
    let server = McpServer::new();
    let response = server.handle_request(&JsonRpcRequest {
        id: json!(1),
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.identity/1",
            "arguments": identity_request(root)
        }),
    });
    assert!(
        response.error.is_none(),
        "MCP identity failed: {:?}",
        response.error
    );
    response.result.expect("MCP has result")["structuredContent"].clone()
}

fn mcp_apply(root: &std::path::Path, revision: &str) -> Value {
    let server = McpServer::new();
    let response = server.handle_request(&JsonRpcRequest {
        id: json!(1),
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.apply/1",
            "arguments": apply_request(root, revision)
        }),
    });
    assert!(
        response.error.is_none(),
        "MCP apply failed: {:?}",
        response.error
    );
    response.result.expect("MCP has result")["structuredContent"].clone()
}

#[test]
fn cli_mcp_and_tui_apply_the_same_versioned_request() {
    let cli_root = root("cli");
    let mcp_root = root("mcp");
    let tui_root = root("tui");
    for path in [&cli_root, &mcp_root, &tui_root] {
        Bundle::create(path).expect("bundle creates");
    }

    let initial = Bundle::at(&cli_root).open().expect("CLI fixture opens");
    let revision = initial.revision_hash_hex().to_string();
    let cli_identity_result = cli_identity(&cli_root);
    let mcp_identity_result = mcp_identity(&mcp_root);
    let tui_host = threeterm_host::Host::new();
    let tui_identity_result = threeterm_tui::execute_domain_command(
        &tui_host,
        IDENTITY_COMMAND_ID,
        identity_request(&tui_root),
    )
    .expect("TUI identity succeeds");
    assert_eq!(cli_identity_result, mcp_identity_result);
    assert_eq!(cli_identity_result, tui_identity_result);

    let cli_result = cli_apply(&cli_root, &revision);
    let mcp_result = mcp_apply(&mcp_root, &revision);
    let tui_result = threeterm_tui::execute_domain_command(
        &tui_host,
        APPLY_COMMAND_ID,
        apply_request(&tui_root, &revision),
    )
    .expect("TUI apply succeeds");

    assert_eq!(cli_result, mcp_result, "CLI and MCP domain results differ");
    assert_eq!(cli_result, tui_result, "CLI and TUI domain results differ");
    for path in [&cli_root, &mcp_root, &tui_root] {
        let loaded = Bundle::at(path).open().expect("applied bundle reloads");
        assert_eq!(loaded.log.len(), 1);
        assert_eq!(loaded.log.entries()[0].operation.as_deref(), Some("add"));
        assert_eq!(loaded.log.entries()[0].feature_id, "box");
        assert_eq!(loaded.log.entries()[0].kind, "cube");
        assert_eq!(loaded.revision_hash_hex(), cli_result["revision_hash"]);
        assert_eq!(
            loaded.feature_graph_hash_hex(),
            cli_result["feature_graph_hash"]
        );
        assert_eq!(
            loaded.log.terminal_digest_hex(),
            cli_result["terminal_log_digest"]
        );
    }

    let _ = fs::remove_dir_all(&cli_root);
    let _ = fs::remove_dir_all(&mcp_root);
    let _ = fs::remove_dir_all(&tui_root);
}

#[test]
fn migrated_adapters_preserve_shared_schema_and_validation_errors() {
    let root = root("errors");
    Bundle::create(&root).expect("bundle creates");
    let revision = Bundle::at(&root)
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let host = threeterm_host::Host::new();
    let tui_error = threeterm_tui::execute_domain_command(
        &host,
        APPLY_COMMAND_ID,
        json!({
            "bundle_path": root.to_string_lossy(),
            "expected_revision": revision,
            "operation": "add",
            "feature_id": "box"
        }),
    )
    .expect_err("missing kind is rejected");
    assert!(matches!(
        tui_error,
        threeterm_protocol::command_execution::ExecutionError::Handler(
            threeterm_host::HostError::Validation { .. }
        )
    ));

    let identity = host
        .execute_domain_command(
            IDENTITY_COMMAND_ID,
            json!({"bundle_path": root.to_string_lossy()}),
        )
        .expect("identity remains available after rejection");
    assert_eq!(identity["transaction_count"], 0);

    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");
    let server = McpServer::new();
    let invalid = server.handle_request(&JsonRpcRequest {
        id: json!(2),
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.apply/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "expected_revision": identity["revision_hash"],
                "operation": "rename",
                "feature_id": "box"
            }
        }),
    });
    assert_eq!(invalid.error.expect("schema error").code, -32602);
    let semantic = server.handle_request(&JsonRpcRequest {
        id: json!(3),
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.apply/1",
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "expected_revision": identity["revision_hash"],
                "operation": "add",
                "feature_id": "box"
            }
        }),
    });
    let semantic_error = semantic.error.expect("semantic error");
    assert_eq!(semantic_error.code, -32603);
    assert!(semantic_error.message.contains("requires kind"));
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log_before);
    let _ = fs::remove_dir_all(root);
}

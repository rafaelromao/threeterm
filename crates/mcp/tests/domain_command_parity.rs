use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{APPLY_COMMAND_ID, EXTRUDE_COMMAND_ID, IDENTITY_COMMAND_ID};

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

fn extrude_request(root: &std::path::Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "extrude",
        "profile": [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
        "height": 2.0
    })
}

fn edge_reference(revision: &str) -> Value {
    json!({
        "semantic_id": "edge-source",
        "provenance": {
            "source_feature_id": "base",
            "source_revision_id": revision,
            "source_edge_id": "edge-source"
        },
        "role": "outer-perimeter",
        "evidence": {
            "midpoint": [2.0, 0.0, 0.0],
            "tangent": [1.0, 0.0, 0.0],
            "length": 4.0
        }
    })
}

fn edge_edit_target(revision: &str) -> Value {
    json!({
        "semantic_id": "edge-target",
        "provenance": {
            "source_feature_id": "base",
            "source_revision_id": revision,
            "source_edge_id": "edge-target"
        },
        "role": "outer-perimeter",
        "evidence": {
            "midpoint": [0.0, 4.0, 1.0],
            "tangent": [0.0, 0.0, 1.0],
            "length": 2.0
        }
    })
}

fn edge_adjacent_target(revision: &str) -> Value {
    let mut target = edge_edit_target(revision);
    target["semantic_id"] = json!("edge-adjacent-target");
    target["provenance"]["source_edge_id"] = json!("edge-adjacent-target");
    target["evidence"]["midpoint"] = json!([0.0, 0.0, 1.0]);
    target
}

fn edge_request(root: &std::path::Path, revision: &str, reference: Value) -> Value {
    edge_request_with_target(root, revision, reference, edge_edit_target(revision))
}

fn edge_request_with_target(
    root: &std::path::Path,
    revision: &str,
    reference: Value,
    edit_target: Value,
) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "expected_revision": revision,
        "edit_feature_id": "fillet-after-edge",
        "edit_kind": "fillet",
        "base_feature_id": "base",
        "radius": 0.25,
        "reference": reference,
        "edit_target": edit_target
    })
}

fn edge_split_request(
    root: &std::path::Path,
    revision: &str,
    reference: Value,
    edit_target: Value,
) -> Value {
    let mut request = edge_request_with_target(root, revision, reference, edit_target);
    request["edit_kind"] = json!("split");
    request["plane_point"] = json!([2.0, 0.0, 0.0]);
    request["plane_normal"] = json!([1.0, 0.0, 0.0]);
    request
}

fn setup_edge_root(root: &std::path::Path, label: &str) -> Option<String> {
    let worker = OcctWorker::locate().ok()?;
    Bundle::create(root).expect("bundle creates");
    let host = threeterm_host::Host::new();
    host.extrude(
        root,
        ExtrudeRequest::new(
            format!("edge-{label}"),
            vec![(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)],
            2.0,
        )
        .with_output_path(root.join("stage"), "base.brep")
        .with_feature_id("base"),
        &worker,
    )
    .expect("base solid commits");
    Some(host.identity(root).expect("identity loads").revision_hash)
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

fn cli_missing_kind(root: &std::path::Path, revision: &str) -> Value {
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
    ]
    .into_iter()
    .map(OsString::from);
    let status = threeterm_cli::dispatch::dispatch(args, &mut stdout, &mut stderr);
    assert_ne!(status, 0, "CLI accepts a semantically invalid request");
    assert!(stdout.is_empty());
    serde_json::from_slice(&stderr).expect("CLI returns a structured diagnostic")
}

fn cli_reattach_edge(
    root: &std::path::Path,
    revision: &str,
    reference: Value,
    edit_target: Value,
) -> Value {
    cli_reattach_edge_with_kind(root, revision, reference, edit_target, "fillet", None)
}

fn cli_reattach_edge_split(
    root: &std::path::Path,
    revision: &str,
    reference: Value,
    edit_target: Value,
) -> Value {
    cli_reattach_edge_with_kind(
        root,
        revision,
        reference,
        edit_target,
        "split",
        Some(([2.0, 0.0, 0.0], [1.0, 0.0, 0.0])),
    )
}

fn cli_reattach_edge_with_kind(
    root: &std::path::Path,
    revision: &str,
    reference: Value,
    edit_target: Value,
    edit_kind: &str,
    split_plane: Option<([f64; 3], [f64; 3])>,
) -> Value {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let path = root.to_string_lossy().into_owned();
    let reference = serde_json::to_string(&reference).expect("reference serializes");
    let edit_target = serde_json::to_string(&edit_target).expect("edit target serializes");
    let mut args = vec![
        OsString::from("--machine"),
        OsString::from("reattach-edge"),
        OsString::from("--bundle"),
        OsString::from(path),
        OsString::from("--expected-revision"),
        OsString::from(revision),
        OsString::from("--edit-feature-id"),
        OsString::from("fillet-after-edge"),
        OsString::from("--edit-kind"),
        OsString::from(edit_kind),
        OsString::from("--base"),
        OsString::from("base"),
        OsString::from("--radius"),
        OsString::from("0.25"),
        OsString::from("--reference"),
        OsString::from(reference),
        OsString::from("--edit-target"),
        OsString::from(edit_target),
    ];
    if let Some((point, normal)) = split_plane {
        args.extend([
            OsString::from("--plane-point"),
            OsString::from(format!("{},{},{}", point[0], point[1], point[2])),
            OsString::from("--plane-normal"),
            OsString::from(format!("{},{},{}", normal[0], normal[1], normal[2])),
        ]);
    }
    let status = threeterm_cli::dispatch::dispatch(args, &mut stdout, &mut stderr);
    assert_eq!(
        status,
        0,
        "CLI edge reattachment failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    serde_json::from_slice(&stdout).expect("CLI edge command returns JSON")
}

fn mcp_identity(root: &std::path::Path) -> Value {
    let server = McpServer::new();
    let response = server.handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
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
        is_notification: false,
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
    let initial_terminal_digest = initial.log.terminal_digest_hex().to_string();
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
        assert_eq!(loaded.generation.id, cli_result["generation_id"]);
        assert_eq!(loaded.manifest.revision_id, cli_result["revision_id"]);
        assert_eq!(loaded.log.len(), cli_result["transaction_count"]);
        assert_eq!(loaded.log.entries()[0].log_index, 0);
        assert_eq!(
            loaded.log.entries()[0].previous_digest,
            initial_terminal_digest
        );
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
        &tui_error,
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
        is_notification: false,
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
        is_notification: false,
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
    assert!(semantic.error.is_none());
    let semantic_result = semantic.result.expect("semantic failure is a tool result");
    assert_eq!(semantic_result["isError"], true);
    assert_eq!(
        semantic_result["content"][0]["type"], "text",
        "semantic failures use MCP text content"
    );
    assert!(
        semantic_result["content"][0]["text"]
            .as_str()
            .expect("semantic error content is text")
            .contains("requires kind")
    );
    let cli_error = cli_missing_kind(&root, identity["revision_hash"].as_str().unwrap());
    assert_eq!(cli_error["code"], "invalid_request");
    assert!(cli_error["arg"].as_str().unwrap().contains("requires kind"));
    if let threeterm_protocol::command_execution::ExecutionError::Handler(
        threeterm_host::HostError::Validation { detail },
    ) = tui_error
    {
        assert!(detail.contains("requires kind"));
    } else {
        panic!("TUI diagnostic classification changed");
    }
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log_before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cli_mcp_and_tui_route_extrude_through_the_shared_executor() {
    let cli_root = root("extrude-cli");
    let mcp_root = root("extrude-mcp");
    let tui_root = root("extrude-tui");
    for path in [&cli_root, &mcp_root, &tui_root] {
        Bundle::create(path).expect("bundle creates");
    }

    let cli = threeterm_cli::dispatch::dispatch_registered_command(
        &threeterm_host::Host::new(),
        EXTRUDE_COMMAND_ID,
        extrude_request(&cli_root),
    );
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        EXTRUDE_COMMAND_ID,
        extrude_request(&tui_root),
    );
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.extrude/1",
            "arguments": extrude_request(&mcp_root)
        }),
    });

    if threeterm_occt_worker::OcctWorker::locate().is_err() {
        assert!(
            !format!("{cli:?}").contains("UnsupportedTool")
                && !format!("{tui:?}").contains("not handled")
                && mcp.error.is_none()
                && mcp
                    .result
                    .as_ref()
                    .is_some_and(|result| result["isError"] == true),
            "adapters must route extrude through the executor"
        );
    } else {
        let cli = cli.expect("CLI extrude executes");
        let tui = tui.expect("TUI extrude executes");
        let mcp = mcp.result.expect("MCP extrude executes")["structuredContent"].clone();
        for result in [&cli, &tui, &mcp] {
            assert_eq!(result["status"], "ok");
            assert_eq!(result["operation"], "extrude");
            assert_eq!(result["feature_id"], "extrude");
        }
        assert_eq!(cli["brep_sha256"], tui["brep_sha256"]);
        assert_eq!(cli["brep_sha256"], mcp["brep_sha256"]);
    }

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

#[test]
fn cli_mcp_and_tui_route_edge_reattachment_through_the_shared_executor() {
    let cli_root = root("edge-cli");
    let mcp_root = root("edge-mcp");
    let tui_root = root("edge-tui");
    let Some(cli_revision) = setup_edge_root(&cli_root, "cli") else {
        return;
    };
    let Some(tui_revision) = setup_edge_root(&tui_root, "tui") else {
        return;
    };
    let Some(mcp_revision) = setup_edge_root(&mcp_root, "mcp") else {
        return;
    };
    let cli = cli_reattach_edge(
        &cli_root,
        &cli_revision,
        edge_reference(&cli_revision),
        edge_edit_target(&cli_revision),
    );
    let tui = threeterm_tui::execute_selected_edge_reattachment(
        &threeterm_host::Host::new(),
        &tui_root,
        &tui_revision,
        "fillet-after-edge",
        "fillet",
        "base",
        0.25,
        edge_reference(&tui_revision),
        edge_edit_target(&tui_revision),
    )
    .expect("TUI edge command executes");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.reattach-edge/2",
            "arguments": edge_request(&mcp_root, &mcp_revision, edge_reference(&mcp_revision))
        }),
    });
    let mcp = mcp.result.expect("MCP edge command executes")["structuredContent"].clone();
    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["outcome"], "resolved");
        assert!(
            result["selected_edge_id"]
                .as_str()
                .expect("selected edge id")
                .starts_with("edge-")
        );
        assert_eq!(result["committed"], true);
    }
    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

#[test]
fn cli_mcp_and_tui_report_real_worker_role_incompatibility_without_commit() {
    let cli_root = root("edge-incompatible-cli");
    let mcp_root = root("edge-incompatible-mcp");
    let tui_root = root("edge-incompatible-tui");
    let Some(cli_revision) = setup_edge_root(&cli_root, "incompatible-cli") else {
        return;
    };
    let Some(tui_revision) = setup_edge_root(&tui_root, "incompatible-tui") else {
        return;
    };
    let Some(mcp_revision) = setup_edge_root(&mcp_root, "incompatible-mcp") else {
        return;
    };

    let mut cli_reference = edge_reference(&cli_revision);
    cli_reference["role"] = json!("inner-perimeter");
    let cli = cli_reattach_edge(
        &cli_root,
        &cli_revision,
        cli_reference,
        edge_edit_target(&cli_revision),
    );

    let mut tui_reference = edge_reference(&tui_revision);
    tui_reference["role"] = json!("inner-perimeter");
    let tui = threeterm_tui::execute_selected_edge_reattachment(
        &threeterm_host::Host::new(),
        &tui_root,
        &tui_revision,
        "fillet-after-incompatible",
        "fillet",
        "base",
        0.25,
        tui_reference,
        edge_edit_target(&tui_revision),
    )
    .expect("TUI edge command reports incompatibility");

    let mut mcp_reference = edge_reference(&mcp_revision);
    mcp_reference["role"] = json!("inner-perimeter");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.reattach-edge/2",
            "arguments": edge_request(&mcp_root, &mcp_revision, mcp_reference)
        }),
    });
    let mcp = mcp
        .result
        .expect("MCP edge command reports incompatibility")["structuredContent"]
        .clone();

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["outcome"], "incompatible");
        assert_eq!(result["committed"], false);
    }
    for path in [&cli_root, &mcp_root, &tui_root] {
        assert_eq!(Bundle::at(path).open().unwrap().log.len(), 1);
        assert!(!path.join("brep/fillet-after-incompatible.brep").exists());
    }
    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

#[test]
fn cli_mcp_and_tui_report_real_worker_ambiguity_without_commit() {
    let cli_root = root("edge-ambiguous-cli");
    let mcp_root = root("edge-ambiguous-mcp");
    let tui_root = root("edge-ambiguous-tui");
    let Some(cli_revision) = setup_edge_root(&cli_root, "ambiguous-cli") else {
        return;
    };
    let Some(tui_revision) = setup_edge_root(&tui_root, "ambiguous-tui") else {
        return;
    };
    let Some(mcp_revision) = setup_edge_root(&mcp_root, "ambiguous-mcp") else {
        return;
    };

    let before = [
        fs::read(cli_root.join("manifest.json")).expect("CLI manifest reads"),
        fs::read(mcp_root.join("manifest.json")).expect("MCP manifest reads"),
        fs::read(tui_root.join("manifest.json")).expect("TUI manifest reads"),
    ];
    let logs = [
        fs::read(cli_root.join("transactions.log")).expect("CLI log reads"),
        fs::read(mcp_root.join("transactions.log")).expect("MCP log reads"),
        fs::read(tui_root.join("transactions.log")).expect("TUI log reads"),
    ];

    let cli = cli_reattach_edge_split(
        &cli_root,
        &cli_revision,
        edge_reference(&cli_revision),
        edge_adjacent_target(&cli_revision),
    );
    let tui = threeterm_tui::execute_selected_edge_split(
        &threeterm_host::Host::new(),
        &tui_root,
        &tui_revision,
        "fillet-after-ambiguous",
        "base",
        0.25,
        [2.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        edge_reference(&tui_revision),
        edge_adjacent_target(&tui_revision),
    )
    .expect("TUI edge command reports ambiguity");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.reattach-edge/2",
            "arguments": edge_split_request(
                &mcp_root,
                &mcp_revision,
                edge_reference(&mcp_revision),
                edge_adjacent_target(&mcp_revision),
            )
        }),
    });
    let mcp = mcp.result.expect("MCP edge command reports ambiguity")["structuredContent"].clone();

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["outcome"], "ambiguous");
        let candidates = result["candidate_edge_ids"].as_array().unwrap();
        assert!(candidates.len() >= 2);
        assert_ne!(candidates[0], candidates[1]);
        assert_eq!(result["committed"], false);
    }
    for (index, path) in [&cli_root, &mcp_root, &tui_root].into_iter().enumerate() {
        assert_eq!(Bundle::at(path).open().unwrap().log.len(), 1);
        assert_eq!(fs::read(path.join("manifest.json")).unwrap(), before[index]);
        assert_eq!(
            fs::read(path.join("transactions.log")).unwrap(),
            logs[index]
        );
        assert!(!path.join("brep/fillet-after-ambiguous.brep").exists());
    }
    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

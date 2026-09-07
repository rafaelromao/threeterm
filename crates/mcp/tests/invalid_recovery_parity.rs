use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_cli::dispatch::{DispatchError, dispatch_registered_command};
use threeterm_host::{
    Host, HostError, StaleLastValidGeometryEntry, domain_command_diagnostic,
    domain_command_failure_value,
};
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::EXTRUDE_COMMAND_ID;

fn root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-invalid-recovery-{label}-{suffix}"))
}

fn require_worker(test_name: &str) -> Option<OcctWorker> {
    match OcctWorker::locate() {
        Ok(worker) => Some(worker),
        Err(error) if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() => {
            panic!("{test_name}: OCCT worker unavailable: {error}");
        }
        Err(_) => {
            eprintln!("{test_name}: OCCT worker unavailable; skipping");
            None
        }
    }
}

fn seed_base(root: &Path, worker: &OcctWorker) {
    Bundle::create(root).expect("bundle creates");
    Host::new()
        .extrude(
            root,
            ExtrudeRequest::new(
                "invalid-recovery-seed",
                vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
                3.0,
            )
            .with_feature_id("base"),
            worker,
        )
        .expect("base solid commits");
}

fn invalid_geometry_request(root: &Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "invalid-cut",
        "profile": [[20.0, 20.0], [22.0, 20.0], [22.0, 22.0], [20.0, 22.0]],
        "height": 3.0,
        "mode": "subtractive",
        "target_feature_id": "base",
    })
}

fn self_intersecting_geometry_request(root: &Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "self-intersecting",
        "profile": [[0.0, 0.0], [10.0, 10.0], [0.0, 10.0], [10.0, 0.0]],
        "height": 3.0,
        "mode": "additive",
    })
}

fn cli_failure(root: &Path) -> Value {
    let error = dispatch_registered_command(
        &Host::new(),
        EXTRUDE_COMMAND_ID,
        invalid_geometry_request(root),
    )
    .expect_err("CLI adapter rejects invalid geometry");
    let DispatchError::Host(error) = error else {
        panic!("CLI returned a non-host failure: {error:?}");
    };
    serde_json::to_value(domain_command_diagnostic(&error)).expect("CLI diagnostic serializes")
}

fn cli_self_intersecting_failure(root: &Path) -> Value {
    let error = dispatch_registered_command(
        &Host::new(),
        EXTRUDE_COMMAND_ID,
        self_intersecting_geometry_request(root),
    )
    .expect_err("CLI adapter rejects a self-intersecting profile");
    let DispatchError::Host(error) = error else {
        panic!("CLI returned a non-host failure: {error:?}");
    };
    serde_json::to_value(domain_command_diagnostic(&error)).expect("CLI diagnostic serializes")
}

fn tui_failure(root: &Path) -> Value {
    let error = threeterm_tui::execute_domain_command(
        &Host::new(),
        EXTRUDE_COMMAND_ID,
        invalid_geometry_request(root),
    )
    .expect_err("TUI adapter rejects invalid geometry");
    let threeterm_protocol::command_execution::ExecutionError::Handler(error) = error else {
        panic!("TUI returned a non-handler failure: {error:?}");
    };
    serde_json::to_value(domain_command_diagnostic(&error)).expect("TUI diagnostic serializes")
}

fn tui_self_intersecting_failure(root: &Path) -> Value {
    let error = threeterm_tui::execute_domain_command(
        &Host::new(),
        EXTRUDE_COMMAND_ID,
        self_intersecting_geometry_request(root),
    )
    .expect_err("TUI adapter rejects a self-intersecting profile");
    let threeterm_protocol::command_execution::ExecutionError::Handler(error) = error else {
        panic!("TUI returned a non-handler failure: {error:?}");
    };
    serde_json::to_value(domain_command_diagnostic(&error)).expect("TUI diagnostic serializes")
}

fn mcp_failure(root: &Path) -> Value {
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.extrude/2",
            "arguments": invalid_geometry_request(root),
        }),
    });
    assert!(response.error.is_none(), "MCP should return a tool failure");
    let result = response.result.expect("MCP has a tool result");
    assert_eq!(result["isError"], true);
    result["structuredContent"].clone()
}

fn mcp_self_intersecting_failure(root: &Path) -> Value {
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.extrude/2",
            "arguments": self_intersecting_geometry_request(root),
        }),
    });
    assert!(response.error.is_none(), "MCP should return a tool failure");
    let result = response.result.expect("MCP has a tool result");
    assert_eq!(result["isError"], true);
    result["structuredContent"].clone()
}

fn canonical_files(root: &Path) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    (
        fs::read(root.join("manifest.json")).expect("manifest reads"),
        fs::read(root.join("transactions.log")).expect("transaction log reads"),
        fs::read(root.join("brep/base.brep")).expect("base BREP reads"),
    )
}

fn file_inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = fs::read_dir(current)
            .expect("artifact inventory directory reads")
            .map(|entry| entry.expect("artifact inventory entry reads"))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("artifact inventory path is under bundle")
                .to_path_buf();
            if entry
                .file_type()
                .expect("artifact inventory type reads")
                .is_dir()
            {
                visit(root, &path, files);
            } else {
                files.insert(
                    relative,
                    fs::read(path).expect("artifact inventory file reads"),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn cli_load(root: &Path) -> Value {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let path = root.to_string_lossy().into_owned();
    let status = threeterm_cli::dispatch::dispatch(
        ["--machine", "load", path.as_str()]
            .into_iter()
            .map(OsString::from),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(
        status,
        0,
        "CLI load failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    serde_json::from_slice(&stdout).expect("CLI load returns JSON")
}

fn mcp_load(root: &Path) -> Value {
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.load/1",
            "arguments": {"bundle_path": root.to_string_lossy()}
        }),
    });
    assert!(
        response.error.is_none(),
        "MCP load failed: {:?}",
        response.error
    );
    response.result.expect("MCP load has result")["structuredContent"].clone()
}

#[test]
fn invalid_geometry_preserves_canonical_state_and_matches_all_adapter_diagnostics() {
    let Some(worker) = require_worker(
        "invalid_geometry_preserves_canonical_state_and_matches_all_adapter_diagnostics",
    ) else {
        return;
    };
    let cli_root = root("cli");
    let mcp_root = root("mcp");
    let tui_root = root("tui");
    for root in [&cli_root, &mcp_root, &tui_root] {
        seed_base(root, &worker);
    }

    let before = [&cli_root, &mcp_root, &tui_root].map(|root| {
        let host = Host::new();
        (
            host.load(root).expect("canonical snapshot loads"),
            canonical_files(root),
            file_inventory(root),
        )
    });
    let diagnostics = [
        cli_failure(&cli_root),
        mcp_failure(&mcp_root),
        tui_failure(&tui_root),
    ];

    assert_eq!(diagnostics[0], diagnostics[1]);
    assert_eq!(diagnostics[0], diagnostics[2]);
    assert_eq!(diagnostics[0]["code"], "brep_invalid");
    assert_eq!(
        diagnostics[0]["affected_ids"],
        json!(["invalid-cut", "base"])
    );
    assert_eq!(
        diagnostics[0]["recovery"],
        "correct_geometry_or_restore_revision"
    );
    assert!(
        !diagnostics[0]["arg"]
            .as_str()
            .unwrap()
            .contains("request_id=")
    );

    for (root, (snapshot, files, inventory)) in [&cli_root, &mcp_root, &tui_root]
        .into_iter()
        .zip(before.iter())
    {
        assert_eq!(
            Host::new().load(root).expect("failed edit reloads"),
            snapshot.clone()
        );
        assert_eq!(canonical_files(root), files.clone());
        assert_eq!(file_inventory(root), inventory.clone());
        assert!(!root.join("brep/invalid-cut.brep").exists());
        assert!(
            fs::read_dir(root.join("stage"))
                .expect("stage directory reads")
                .next()
                .is_none()
        );
    }

    for root in [&cli_root, &mcp_root, &tui_root] {
        fs::remove_file(root.join("brep/base.brep")).expect("base BREP removes for reload");
        for directory in ["cache", ".derived", "stage"] {
            let _ = fs::remove_dir_all(root.join(directory));
        }
    }
    let loads = [
        cli_load(&cli_root),
        mcp_load(&mcp_root),
        threeterm_tui::execute_domain_command(
            &Host::new(),
            threeterm_protocol::schema::LOAD_COMMAND_ID,
            json!({"bundle_path": tui_root.to_string_lossy()}),
        )
        .expect("TUI reloads after the rejected edit"),
    ];
    for ((root, (snapshot, files, _)), load_result) in [&cli_root, &mcp_root, &tui_root]
        .into_iter()
        .zip(before.iter())
        .zip(loads)
    {
        assert_eq!(
            load_result["feature_graph_hash"],
            snapshot.feature_graph_hash
        );
        assert_eq!(load_result["revision_hash"], snapshot.revision_hash);
        assert_eq!(fs::read(root.join("manifest.json")).unwrap(), files.0);
        assert_eq!(fs::read(root.join("transactions.log")).unwrap(), files.1);
        assert_eq!(fs::read(root.join("brep/base.brep")).unwrap(), files.2);
        assert!(
            !Bundle::at(root)
                .open()
                .unwrap()
                .graph
                .contains_feature("invalid-cut")
        );
        assert!(!root.join("brep/invalid-cut.brep").exists());
    }

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn self_intersecting_profile_preserves_canonical_state_and_matches_all_adapter_diagnostics() {
    let Some(worker) = require_worker(
        "self_intersecting_profile_preserves_canonical_state_and_matches_all_adapter_diagnostics",
    ) else {
        return;
    };
    let cli_root = root("self-intersecting-cli");
    let mcp_root = root("self-intersecting-mcp");
    let tui_root = root("self-intersecting-tui");
    for root in [&cli_root, &mcp_root, &tui_root] {
        seed_base(root, &worker);
    }

    let before = [&cli_root, &mcp_root, &tui_root].map(|root| {
        let host = Host::new();
        (
            host.load(root).expect("canonical snapshot loads"),
            canonical_files(root),
            file_inventory(root),
        )
    });
    let diagnostics = [
        cli_self_intersecting_failure(&cli_root),
        mcp_self_intersecting_failure(&mcp_root),
        tui_self_intersecting_failure(&tui_root),
    ];

    assert_eq!(diagnostics[0], diagnostics[1]);
    assert_eq!(diagnostics[0], diagnostics[2]);
    assert_eq!(diagnostics[0]["code"], "brep_invalid");
    assert_eq!(diagnostics[0]["affected_ids"], json!(["self-intersecting"]));
    assert_eq!(
        diagnostics[0]["recovery"],
        "correct_geometry_or_restore_revision"
    );

    for (root, (snapshot, files, inventory)) in [&cli_root, &mcp_root, &tui_root]
        .into_iter()
        .zip(before.iter())
    {
        assert_eq!(
            Host::new().load(root).expect("failed edit reloads"),
            snapshot.clone()
        );
        assert_eq!(canonical_files(root), files.clone());
        assert_eq!(file_inventory(root), inventory.clone());
        assert!(!root.join("brep/self-intersecting.brep").exists());
    }

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn stale_export_and_domain_failures_use_the_host_owned_value_projection() {
    let invalid_edit = HostError::InvalidEdit {
        detail: "brep_invalid: subtractive extrusion does not intersect the target solid".into(),
        affected_ids: vec!["invalid-cut".into(), "base".into()],
        recovery: "correct_geometry_or_restore_revision",
    };
    assert_eq!(
        domain_command_failure_value(&invalid_edit),
        threeterm_tui::domain_command_failure_value(&invalid_edit)
    );
    assert_eq!(
        domain_command_failure_value(&invalid_edit)["affected_ids"],
        json!(["invalid-cut", "base"])
    );

    let stale = HostError::StaleLastValidGeometry {
        feature_id: "l-bracket".into(),
        active_revision: "history-revision-2".into(),
        stale_features: vec![StaleLastValidGeometryEntry {
            feature_id: "l-bracket-base".into(),
            status: "broken".into(),
            last_valid_geometry_fingerprint: "fingerprint".into(),
        }],
    };
    let value = domain_command_failure_value(&stale);
    assert_eq!(value["code"], "stale_last_valid_geometry");
    assert_eq!(value["override_eligible"], false);
    assert_eq!(
        value["recovery"],
        "correct or restore the feature and recompute current geometry"
    );
}

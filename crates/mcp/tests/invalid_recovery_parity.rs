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

fn canonical_files(root: &Path) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    (
        fs::read(root.join("manifest.json")).expect("manifest reads"),
        fs::read(root.join("transactions.log")).expect("transaction log reads"),
        fs::read(root.join("brep/base.brep")).expect("base BREP reads"),
    )
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

    for (root, (snapshot, files)) in [&cli_root, &mcp_root, &tui_root].into_iter().zip(before) {
        assert_eq!(
            Host::new().load(root).expect("failed edit reloads"),
            snapshot
        );
        assert_eq!(canonical_files(root), files);
        assert!(!root.join("brep/invalid-cut.brep").exists());
        assert!(
            fs::read_dir(root.join("stage"))
                .expect("stage directory reads")
                .next()
                .is_none()
        );
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

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_cli::dispatch::dispatch;
use threeterm_domain::history::HistoryStatus;
use threeterm_host::Host;
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::{BracketRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{
    CREATE_REVISION_COMMAND_ID, EXPORT_COMMAND_ID, HISTORICAL_EDIT_COMMAND_ID,
    HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION, UNDO_COMMAND_ID,
};
use threeterm_tui::{FeatureTarget, SelectionEvent, SelectionVerification, TuiSession};

fn temp_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-historical-parity-{label}-{suffix}"))
}

fn require_occt_worker(test_name: &str) -> Option<OcctWorker> {
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

fn seed_bundle(root: &Path, worker: &OcctWorker) {
    Host::new()
        .create_bracket(
            root,
            BracketRequest::new("historical-recovery-seed", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("l-bracket"),
            worker,
        )
        .expect("L-bracket fixture persists through the production worker");
}

fn historical_edit_request(root: &Path, value: f64) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "l-bracket-base",
        "parameter": "length",
        "value": value,
    })
}

fn cli_call<I>(args: I) -> Result<Value, Value>
where
    I: IntoIterator<Item = OsString>,
{
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = dispatch(args, &mut stdout, &mut stderr);
    if status == 0 {
        return Ok(serde_json::from_slice(&stdout).expect("CLI returns JSON"));
    }
    assert!(stdout.is_empty());
    Err(serde_json::from_slice(&stderr).expect("CLI returns a diagnostic"))
}

fn cli_historical_edit(root: &Path, value: f64) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("historical-edit"),
        OsString::from(root),
        OsString::from("--feature-id"),
        OsString::from("l-bracket-base"),
        OsString::from("--parameter"),
        OsString::from("length"),
        OsString::from("--value"),
        OsString::from(value.to_string()),
    ])
    .expect("CLI historical edit succeeds")
}

fn mcp_call(name: &str, arguments: Value) -> Result<Value, Value> {
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": name,
            "arguments": arguments,
        }),
    });
    if let Some(error) = response.error {
        return Err(json!({"code": error.code, "message": error.message}));
    }
    let result = response.result.expect("MCP has a result");
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(result
            .get("structuredContent")
            .cloned()
            .unwrap_or_else(|| json!(result["content"][0]["text"])));
    }
    Ok(result["structuredContent"].clone())
}

fn mcp_historical_edit(root: &Path, value: f64) -> Value {
    mcp_call(
        "threeterm.command.historical-edit/1",
        historical_edit_request(root, value),
    )
    .expect("MCP historical edit succeeds")
}

fn tui_historical_edit(root: &Path, value: f64) -> Value {
    threeterm_tui::execute_domain_command(
        &Host::new(),
        HISTORICAL_EDIT_COMMAND_ID,
        historical_edit_request(root, value),
    )
    .expect("TUI historical edit succeeds")
}

fn create_revision_request(root: &Path, name: &str) -> Value {
    json!({"bundle_path": root.to_string_lossy(), "name": name})
}

fn timeline_request(root: &Path) -> Value {
    json!({"bundle_path": root.to_string_lossy(), "feature_id": "l-bracket"})
}

fn cli_timeline(root: &Path) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("timeline"),
        OsString::from(root),
        OsString::from("--feature-id"),
        OsString::from("l-bracket"),
    ])
    .expect("CLI timeline succeeds")
}

fn mcp_timeline(root: &Path) -> Value {
    mcp_call("threeterm.command.timeline/1", timeline_request(root)).expect("MCP timeline succeeds")
}

fn tui_timeline(root: &Path) -> (TuiSession, Value) {
    let host = Host::new();
    let active_revision = host
        .history(root)
        .expect("TUI reads the canonical history before opening its timeline")
        .active_snapshot()
        .revision_id
        .clone();
    let mut session = TuiSession::new(
        [FeatureTarget::new("l-bracket", "L-bracket")],
        active_revision,
    );
    session
        .transition_selection(SelectionEvent::Nominate {
            candidates: vec!["l-bracket".to_string()],
        })
        .expect("TUI nominates the selected feature");
    session
        .transition_selection(SelectionEvent::Verify(SelectionVerification::Exact {
            stable_ids: vec!["l-bracket".to_string()],
        }))
        .expect("TUI verifies the selected feature");
    session
        .open_feature_timeline(&host, root)
        .expect("TUI timeline opens");
    let state = session.state();
    let timeline = state.feature_timeline.expect("TUI timeline state exists");
    let stale_overlay = session.stale_last_valid_geometry_overlay();
    let active_stale_fingerprint = state
        .stale_last_valid_geometry
        .iter()
        .find(|feature| feature.feature_id == "l-bracket-base")
        .map(|feature| feature.last_valid_geometry_fingerprint.clone())
        .unwrap_or_default();
    let canonical_revision = state.canonical_revision.clone();
    let revisions = timeline
        .revisions
        .iter()
        .map(|revision| {
            json!({
                "revision_id": revision.revision_id,
                "operation": revision.operation,
                "status": revision.status,
                "stale_last_valid_geometry_fingerprint": if revision.revision_id == canonical_revision {
                    active_stale_fingerprint.clone()
                } else {
                    String::new()
                },
                "named_revision_names": revision.named_revision_names,
            })
        })
        .collect::<Vec<_>>();
    (
        session,
        json!({
            "feature_id": timeline.feature_id,
            "active_revision": state.canonical_revision,
            "revisions": revisions,
            "named_revisions": timeline.named_revisions,
            "stale_last_valid_geometry": state.stale_last_valid_geometry.iter().map(|feature| json!({
                "feature_id": feature.feature_id,
                "status": feature.status,
                "active_revision": feature.active_revision,
                "last_valid_geometry_fingerprint": feature.last_valid_geometry_fingerprint,
            })).collect::<Vec<_>>(),
            "stale_overlay": stale_overlay,
        }),
    )
}

fn tui_call(
    command: threeterm_protocol::schema::CommandId,
    request: Value,
) -> Result<Value, Value> {
    threeterm_tui::execute_domain_command(&Host::new(), command, request).map_err(|error| {
        match error {
            threeterm_protocol::command_execution::ExecutionError::Handler(error) => {
                serde_json::to_value(threeterm_cli::dispatch::host_error_diagnostic(&error))
                    .expect("TUI diagnostic serializes")
            }
            error => json!({"detail": format!("{error:?}")}),
        }
    })
}

fn cli_undo(root: &Path) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("undo"),
        OsString::from(root),
    ])
    .expect("CLI undo succeeds")
}

fn mcp_undo(root: &Path) -> Value {
    mcp_call(
        "threeterm.command.undo/1",
        json!({"bundle_path": root.to_string_lossy()}),
    )
    .expect("MCP undo succeeds")
}

fn tui_undo(root: &Path) -> Value {
    tui_call(
        UNDO_COMMAND_ID,
        json!({"bundle_path": root.to_string_lossy()}),
    )
    .expect("TUI undo succeeds")
}

fn cli_restore(root: &Path, feature_id: &str, name: &str) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("restore-revision"),
        OsString::from(root),
        OsString::from("--feature-id"),
        OsString::from(feature_id),
        OsString::from("--name"),
        OsString::from(name),
    ])
    .expect("CLI named revision restore succeeds")
}

fn mcp_restore(root: &Path, feature_id: &str, name: &str) -> Value {
    mcp_call(
        "threeterm.command.restore-revision/1",
        json!({
            "bundle_path": root.to_string_lossy(),
            "feature_id": feature_id,
            "name": name,
        }),
    )
    .expect("MCP named revision restore succeeds")
}

fn tui_restore(root: &Path, name: &str) -> Value {
    let (mut session, _) = tui_timeline(root);
    let view = session
        .restore_feature_timeline(&Host::new(), root, name)
        .expect("TUI named revision restore succeeds");
    threeterm_host::history_commit_value(
        "restore-revision",
        HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION,
        &view,
    )
}

fn tui_export(root: &Path, output_dir: &Path) -> Result<Value, Value> {
    tui_call(EXPORT_COMMAND_ID, export_request(root, output_dir))
}

fn current_brep(root: &Path) -> Vec<u8> {
    fs::read(root.join("brep/l-bracket.brep")).expect("current BREP reads")
}

fn semantic_timeline(value: &Value) -> Value {
    let named_revisions = value["named_revisions"]
        .as_array()
        .expect("timeline named revisions are an array")
        .iter()
        .map(|revision| {
            revision
                .get("name")
                .cloned()
                .unwrap_or_else(|| revision.clone())
        })
        .collect::<Vec<_>>();
    json!({
        "feature_id": value["feature_id"],
        "active_revision": value["active_revision"],
        "revisions": value["revisions"],
        "named_revisions": named_revisions,
    })
}

fn semantic_export(value: &Value) -> Value {
    let artifacts = value["derived_artifacts"]
        .as_array()
        .expect("export returns derived artifact metadata")
        .iter()
        .map(|artifact| {
            json!({
                "operation": artifact["operation"],
                "feature_id": artifact["feature_id"],
                "artifact_kind": artifact["artifact_kind"],
                "artifact_name": artifact["artifact_name"],
                "byte_count": artifact["byte_count"],
                "sha256": artifact["sha256"],
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": value["status"],
        "feature_id": value["feature_id"],
        "accepted_stale_last_valid_geometry": value["accepted_stale_last_valid_geometry"],
        "artifacts": artifacts,
    })
}

fn canonical_semantics(root: &Path) -> Value {
    let loaded = Bundle::at(root).open().expect("canonical bundle reloads");
    let features = loaded
        .history
        .active_snapshot()
        .features
        .values()
        .map(|feature| {
            json!({
                "id": feature.id,
                "dependencies": feature.dependencies,
                "input_value": feature.input_value,
                "geometry_fingerprint": feature.geometry_fingerprint,
                "status": feature.status,
            })
        })
        .collect::<Vec<_>>();
    let graph_features = loaded
        .graph
        .features()
        .map(|feature| json!({"id": feature.id, "kind": feature.kind}))
        .collect::<Vec<_>>();
    json!({
        "active_revision": loaded.history.active_snapshot().revision_id,
        "features": features,
        "feature_graph_hash": loaded.feature_graph_hash_hex(),
        "graph_features": graph_features,
    })
}

fn export_request(root: &Path, output_dir: &Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "l-bracket",
        "formats": ["stl"],
        "output_dir": output_dir.to_string_lossy(),
        "tessellation_deflection": 1.0,
        "override_warnings": true,
        "accept_stale_geometry": true,
    })
}

fn cli_export(root: &Path, output_dir: &Path) -> Result<Value, Value> {
    let root = root.to_string_lossy().into_owned();
    let output_dir = output_dir.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("export"),
        OsString::from("--bundle"),
        OsString::from(root),
        OsString::from("--feature-id"),
        OsString::from("l-bracket"),
        OsString::from("--formats"),
        OsString::from("stl"),
        OsString::from("--output-dir"),
        OsString::from(output_dir),
        OsString::from("--tessellation-deflection"),
        OsString::from("1.0"),
        OsString::from("--override-warnings"),
        OsString::from("--accept-stale-geometry"),
    ])
}

fn cli_create_revision(root: &Path, name: &str) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("create-revision"),
        OsString::from(root),
        OsString::from("--name"),
        OsString::from(name),
    ])
    .expect("CLI named revision creation succeeds")
}

fn mcp_create_revision(root: &Path, name: &str) -> Value {
    mcp_call(
        "threeterm.command.create-revision/1",
        create_revision_request(root, name),
    )
    .expect("MCP named revision creation succeeds")
}

fn tui_create_revision(root: &Path, name: &str) -> Value {
    threeterm_tui::execute_domain_command(
        &Host::new(),
        CREATE_REVISION_COMMAND_ID,
        create_revision_request(root, name),
    )
    .expect("TUI named revision creation succeeds")
}

fn semantic_history(value: &Value) -> Value {
    json!({
        "status": value["status"],
        "operation": value["operation"],
        "active_revision": value["active_revision"],
        "dirty_features": value["dirty_features"],
        "evaluated_features": value["evaluated_features"],
        "blocked_features": value["blocked_features"],
        "diagnostics": value["diagnostics"],
        "named_revisions": value["named_revisions"],
        "features": value["features"],
    })
}

fn semantic_stale_features(value: &Value) -> Vec<Value> {
    value["features"]
        .as_array()
        .expect("history response exposes features")
        .iter()
        .filter(|feature| feature["stale_last_valid_geometry"] == true)
        .map(|feature| {
            json!({
                "id": feature["id"],
                "status": feature["status"],
                "last_valid_geometry_fingerprint": feature["last_valid_geometry_fingerprint"],
            })
        })
        .collect()
}

#[test]
fn successful_historical_edit_has_equivalent_current_geometry_through_all_adapters() {
    let Some(worker) = require_occt_worker(
        "successful_historical_edit_has_equivalent_current_geometry_through_all_adapters",
    ) else {
        return;
    };

    let cli_root = temp_root("cli-success");
    let mcp_root = temp_root("mcp-success");
    let tui_root = temp_root("tui-success");
    for root in [&cli_root, &mcp_root, &tui_root] {
        seed_bundle(root, &worker);
    }

    let before = [&cli_root, &mcp_root, &tui_root]
        .map(|root| Bundle::at(root).open().expect("fixture reloads").history);
    let results = [
        cli_historical_edit(&cli_root, 61.0),
        mcp_historical_edit(&mcp_root, 61.0),
        tui_historical_edit(&tui_root, 61.0),
    ];

    assert_eq!(semantic_history(&results[0]), semantic_history(&results[1]));
    assert_eq!(semantic_history(&results[0]), semantic_history(&results[2]));
    assert_eq!(results[0]["status"], "ok");
    assert_eq!(results[0]["operation"], "historical-edit");
    assert_eq!(
        results[0]["dirty_features"],
        json!(["l-bracket-base", "l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(results[0]["blocked_features"], json!([]));

    for (root, prior) in [&cli_root, &mcp_root, &tui_root].into_iter().zip(before) {
        let after = Bundle::at(root).open().expect("edited bundle reloads");
        for feature_id in ["l-bracket-base", "l-bracket-bend", "l-bracket-finish"] {
            let previous = &prior.active_snapshot().features[feature_id];
            let current = &after.history.active_snapshot().features[feature_id];
            assert_eq!(current.status, HistoryStatus::CurrentValid);
            assert_ne!(
                current.geometry_fingerprint, previous.geometry_fingerprint,
                "successful edit changes current geometry for {feature_id}"
            );
        }
        assert_eq!(
            after.history.active_snapshot().features["l-bracket-independent-base"]
                .geometry_fingerprint,
            prior.active_snapshot().features["l-bracket-independent-base"].geometry_fingerprint,
            "successful edit preserves the independent branch"
        );
    }

    let output_roots = [
        temp_root("cli-success-export"),
        temp_root("mcp-success-export"),
        temp_root("tui-success-export"),
    ];
    for output in &output_roots {
        fs::create_dir_all(output).expect("success export directory creates");
    }
    let exports = [
        cli_export(&cli_root, &output_roots[0]).expect("CLI success export succeeds"),
        mcp_call(
            "threeterm.command.export/1",
            export_request(&mcp_root, &output_roots[1]),
        )
        .expect("MCP success export succeeds"),
        tui_export(&tui_root, &output_roots[2]).expect("TUI success export succeeds"),
    ];
    assert_eq!(semantic_export(&exports[0]), semantic_export(&exports[1]));
    assert_eq!(semantic_export(&exports[0]), semantic_export(&exports[2]));
    assert!(exports.iter().all(|export| export["status"] == "ok"));
    let exported_geometry = output_roots
        .iter()
        .map(|output| fs::read(output.join("l-bracket.stl")).expect("STL export reads"))
        .collect::<Vec<_>>();
    assert_eq!(exported_geometry[0], exported_geometry[1]);
    assert_eq!(exported_geometry[0], exported_geometry[2]);

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
    for output in output_roots {
        let _ = fs::remove_dir_all(output);
    }
}

#[test]
fn failed_historical_edit_preserves_independent_geometry_and_exposes_stale_state() {
    let Some(worker) = require_occt_worker(
        "failed_historical_edit_preserves_independent_geometry_and_exposes_stale_state",
    ) else {
        return;
    };

    let cli_root = temp_root("cli-failure");
    let mcp_root = temp_root("mcp-failure");
    let tui_root = temp_root("tui-failure");
    for root in [&cli_root, &mcp_root, &tui_root] {
        seed_bundle(root, &worker);
    }
    let before = [&cli_root, &mcp_root, &tui_root]
        .map(|root| Bundle::at(root).open().expect("fixture reloads").history);
    let before_geometry = [&cli_root, &mcp_root, &tui_root].map(|root| current_brep(root));
    let before_canonical = [&cli_root, &mcp_root, &tui_root].map(|root| canonical_semantics(root));

    let revisions = [
        cli_create_revision(&cli_root, "before-failure"),
        mcp_create_revision(&mcp_root, "before-failure"),
        tui_create_revision(&tui_root, "before-failure"),
    ];
    assert_eq!(revisions[0]["status"], "ok");
    assert_eq!(revisions[0]["operation"], "create-revision");

    let results = [
        cli_historical_edit(&cli_root, 0.0),
        mcp_historical_edit(&mcp_root, 0.0),
        tui_historical_edit(&tui_root, 0.0),
    ];
    assert_eq!(semantic_history(&results[0]), semantic_history(&results[1]));
    assert_eq!(semantic_history(&results[0]), semantic_history(&results[2]));
    assert_eq!(
        semantic_stale_features(&results[0]),
        semantic_stale_features(&results[1])
    );
    assert_eq!(
        semantic_stale_features(&results[0]),
        semantic_stale_features(&results[2])
    );
    assert_eq!(results[0]["status"], "degraded");
    assert_eq!(
        results[0]["dirty_features"],
        json!(["l-bracket-base", "l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(results[0]["evaluated_features"], json!([]));
    assert_eq!(
        results[0]["blocked_features"],
        json!(["l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(
        results[0]["diagnostics"][0]["code"],
        "historical_geometry_invalid"
    );

    for (root, prior) in [&cli_root, &mcp_root, &tui_root].into_iter().zip(before) {
        let after = Bundle::at(root).open().expect("degraded bundle reloads");
        let active = after.history.active_snapshot();
        assert_eq!(
            active.features["l-bracket-base"].status,
            HistoryStatus::Broken
        );
        assert_eq!(
            active.features["l-bracket-base"].last_valid_geometry_fingerprint,
            prior.active_snapshot().features["l-bracket-base"].geometry_fingerprint
        );
        for feature_id in ["l-bracket-bend", "l-bracket-finish"] {
            assert_eq!(
                active.features[feature_id].status,
                HistoryStatus::BlockedByFailure
            );
            assert_eq!(
                active.features[feature_id].last_valid_geometry_fingerprint,
                prior.active_snapshot().features[feature_id].geometry_fingerprint
            );
        }
        assert_eq!(
            active.features["l-bracket-independent-base"].status,
            HistoryStatus::CurrentValid
        );
        assert_eq!(
            active.features["l-bracket-independent-base"].geometry_fingerprint,
            prior.active_snapshot().features["l-bracket-independent-base"].geometry_fingerprint
        );
    }

    let timelines = [
        cli_timeline(&cli_root),
        mcp_timeline(&mcp_root),
        tui_timeline(&tui_root).1,
    ];
    assert_eq!(
        semantic_timeline(&timelines[0]),
        semantic_timeline(&timelines[1])
    );
    assert_eq!(
        semantic_timeline(&timelines[0]),
        semantic_timeline(&timelines[2])
    );
    assert_eq!(timelines[0]["active_revision"], "history-revision-3");
    let failure_revision = timelines[0]["revisions"]
        .as_array()
        .expect("timeline revisions are an array")
        .iter()
        .find(|revision| revision["operation"] == "historical-edit")
        .expect("timeline contains the failed edit");
    assert_eq!(failure_revision["status"], "broken");
    assert!(
        !failure_revision["stale_last_valid_geometry_fingerprint"]
            .as_str()
            .expect("timeline exposes stale geometry")
            .is_empty()
    );
    let tui_stale = timelines[2]["stale_last_valid_geometry"]
        .as_array()
        .expect("TUI exposes stale geometry entries");
    assert_eq!(
        tui_stale
            .iter()
            .map(|feature| feature["feature_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["l-bracket-base", "l-bracket-bend", "l-bracket-finish"]
    );
    assert_eq!(
        tui_stale
            .iter()
            .map(|feature| {
                json!({
                    "id": feature["feature_id"],
                    "status": feature["status"],
                    "last_valid_geometry_fingerprint": feature["last_valid_geometry_fingerprint"],
                })
            })
            .collect::<Vec<_>>(),
        semantic_stale_features(&results[0])
    );
    assert!(
        timelines[2]["stale_overlay"]
            .as_str()
            .expect("TUI exposes a stale geometry overlay")
            .starts_with("[warning-glyph] stale-last-valid-geometry")
    );

    let output_roots = [
        temp_root("cli-failure-export"),
        temp_root("mcp-failure-export"),
        temp_root("tui-failure-export"),
    ];
    for output in &output_roots {
        fs::create_dir_all(output).expect("failure export directory creates");
        fs::write(output.join("sentinel.txt"), b"preserve me").expect("export sentinel writes");
    }
    let cli_error = cli_export(&cli_root, &output_roots[0]).expect_err("CLI stale export fails");
    let mcp_error = mcp_call(
        "threeterm.command.export/1",
        export_request(&mcp_root, &output_roots[1]),
    )
    .expect_err("MCP stale export fails");
    let tui_error = tui_export(&tui_root, &output_roots[2]).expect_err("TUI stale export fails");
    assert_eq!(cli_error["code"], "invalid_request");
    assert!(
        cli_error["arg"]
            .as_str()
            .expect("CLI stale error detail")
            .contains("stale last-valid geometry")
    );
    assert_eq!(mcp_error["code"], "stale_last_valid_geometry");
    assert_eq!(mcp_error["override_eligible"], false);
    assert_eq!(tui_error["code"], "invalid_request");
    for output in &output_roots {
        assert_eq!(
            fs::read(output.join("sentinel.txt")).unwrap(),
            b"preserve me"
        );
        assert!(!output.join("l-bracket.stl").exists());
        assert!(!fs::read_dir(output).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".threeterm")
        }));
    }

    let restores = [
        cli_restore(&cli_root, "l-bracket", "before-failure"),
        mcp_restore(&mcp_root, "l-bracket", "before-failure"),
        tui_restore(&tui_root, "before-failure"),
    ];
    assert_eq!(
        semantic_history(&restores[0]),
        semantic_history(&restores[1])
    );
    assert_eq!(
        semantic_history(&restores[0]),
        semantic_history(&restores[2])
    );
    assert_eq!(restores[0]["status"], "ok");
    assert_eq!(restores[0]["operation"], "restore-revision");
    assert_eq!(restores[0]["blocked_features"], json!([]));
    for ((root, previous_geometry), expected_canonical) in [&cli_root, &mcp_root, &tui_root]
        .into_iter()
        .zip(before_geometry)
        .zip(before_canonical)
    {
        let loaded = Bundle::at(root).open().expect("restored bundle reloads");
        assert!(
            loaded
                .history
                .active_snapshot()
                .features
                .values()
                .all(|feature| feature.status == HistoryStatus::CurrentValid)
        );
        assert!(
            loaded
                .history
                .active_snapshot()
                .features
                .values()
                .all(|feature| {
                    feature.last_valid_geometry_fingerprint.is_none()
                        && feature.diagnostic.is_none()
                })
        );
        assert_eq!(canonical_semantics(root), expected_canonical);
        assert_eq!(current_brep(root), previous_geometry);
    }

    let restored_output_roots = [
        temp_root("cli-restored-export"),
        temp_root("mcp-restored-export"),
        temp_root("tui-restored-export"),
    ];
    for output in &restored_output_roots {
        fs::create_dir_all(output).expect("restored export directory creates");
    }
    let restored_exports = [
        cli_export(&cli_root, &restored_output_roots[0]).expect("CLI restored export succeeds"),
        mcp_call(
            "threeterm.command.export/1",
            export_request(&mcp_root, &restored_output_roots[1]),
        )
        .expect("MCP restored export succeeds"),
        tui_export(&tui_root, &restored_output_roots[2]).expect("TUI restored export succeeds"),
    ];
    assert_eq!(
        semantic_export(&restored_exports[0]),
        semantic_export(&restored_exports[1])
    );
    assert_eq!(
        semantic_export(&restored_exports[0]),
        semantic_export(&restored_exports[2])
    );
    let restored_geometry = restored_output_roots
        .iter()
        .map(|output| fs::read(output.join("l-bracket.stl")).expect("restored STL reads"))
        .collect::<Vec<_>>();
    assert_eq!(restored_geometry[0], restored_geometry[1]);
    assert_eq!(restored_geometry[0], restored_geometry[2]);

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
    for output in output_roots.into_iter().chain(restored_output_roots) {
        let _ = fs::remove_dir_all(output);
    }
}

#[test]
fn divergent_work_preserves_and_restores_the_named_future_through_all_adapters() {
    let Some(worker) = require_occt_worker(
        "divergent_work_preserves_and_restores_the_named_future_through_all_adapters",
    ) else {
        return;
    };

    let cli_root = temp_root("cli-divergence");
    let mcp_root = temp_root("mcp-divergence");
    let tui_root = temp_root("tui-divergence");
    for root in [&cli_root, &mcp_root, &tui_root] {
        seed_bundle(root, &worker);
    }

    let successful = [
        cli_historical_edit(&cli_root, 61.0),
        mcp_historical_edit(&mcp_root, 61.0),
        tui_historical_edit(&tui_root, 61.0),
    ];
    assert_eq!(
        semantic_history(&successful[0]),
        semantic_history(&successful[1])
    );
    assert_eq!(
        semantic_history(&successful[0]),
        semantic_history(&successful[2])
    );
    let future_semantics = [&cli_root, &mcp_root, &tui_root].map(|root| canonical_semantics(root));
    let future_geometry = [&cli_root, &mcp_root, &tui_root].map(|root| current_brep(root));

    let future_output_roots = [
        temp_root("cli-future-export"),
        temp_root("mcp-future-export"),
        temp_root("tui-future-export"),
    ];
    for output in &future_output_roots {
        fs::create_dir_all(output).expect("future export directory creates");
    }
    let future_exports = [
        cli_export(&cli_root, &future_output_roots[0]).expect("CLI future export succeeds"),
        mcp_call(
            "threeterm.command.export/1",
            export_request(&mcp_root, &future_output_roots[1]),
        )
        .expect("MCP future export succeeds"),
        tui_export(&tui_root, &future_output_roots[2]).expect("TUI future export succeeds"),
    ];
    assert_eq!(
        semantic_export(&future_exports[0]),
        semantic_export(&future_exports[1])
    );
    assert_eq!(
        semantic_export(&future_exports[0]),
        semantic_export(&future_exports[2])
    );
    let future_export_geometry = future_output_roots
        .iter()
        .map(|output| fs::read(output.join("l-bracket.stl")).expect("future STL reads"))
        .collect::<Vec<_>>();

    let undone = [
        cli_undo(&cli_root),
        mcp_undo(&mcp_root),
        tui_undo(&tui_root),
    ];
    assert_eq!(semantic_history(&undone[0]), semantic_history(&undone[1]));
    assert_eq!(semantic_history(&undone[0]), semantic_history(&undone[2]));
    assert_eq!(undone[0]["active_revision"], "history-revision-1");

    let divergent = [
        cli_historical_edit(&cli_root, 62.0),
        mcp_historical_edit(&mcp_root, 62.0),
        tui_historical_edit(&tui_root, 62.0),
    ];
    assert_eq!(
        semantic_history(&divergent[0]),
        semantic_history(&divergent[1])
    );
    assert_eq!(
        semantic_history(&divergent[0]),
        semantic_history(&divergent[2])
    );
    assert!(
        divergent[0]["named_revisions"]
            .as_array()
            .expect("divergent edit exposes named revisions")
            .iter()
            .any(|revision| revision["name"] == "recovered-before-undo-3")
    );
    for root in [&cli_root, &mcp_root, &tui_root] {
        let loaded = Bundle::at(root).open().expect("divergent bundle reloads");
        let preserved = loaded
            .history
            .named_revisions()
            .get("recovered-before-undo-3")
            .expect("undoed future is preserved as a named revision");
        assert_eq!(
            preserved.snapshot.features["l-bracket-base"].input_value,
            61.0
        );
    }

    let restored = [
        cli_restore(&cli_root, "l-bracket", "recovered-before-undo-3"),
        mcp_restore(&mcp_root, "l-bracket", "recovered-before-undo-3"),
        tui_restore(&tui_root, "recovered-before-undo-3"),
    ];
    assert_eq!(
        semantic_history(&restored[0]),
        semantic_history(&restored[1])
    );
    assert_eq!(
        semantic_history(&restored[0]),
        semantic_history(&restored[2])
    );
    assert_eq!(restored[0]["status"], "ok");
    assert_eq!(restored[0]["active_revision"], "history-revision-2");
    for ((root, expected), expected_geometry) in [&cli_root, &mcp_root, &tui_root]
        .into_iter()
        .zip(future_semantics)
        .zip(future_geometry)
    {
        assert_eq!(canonical_semantics(root), expected);
        assert_eq!(current_brep(root), expected_geometry);
    }

    let restored_output_roots = [
        temp_root("cli-future-restored-export"),
        temp_root("mcp-future-restored-export"),
        temp_root("tui-future-restored-export"),
    ];
    for output in &restored_output_roots {
        fs::create_dir_all(output).expect("restored future export directory creates");
    }
    let restored_exports = [
        cli_export(&cli_root, &restored_output_roots[0])
            .expect("CLI restored future export succeeds"),
        mcp_call(
            "threeterm.command.export/1",
            export_request(&mcp_root, &restored_output_roots[1]),
        )
        .expect("MCP restored future export succeeds"),
        tui_export(&tui_root, &restored_output_roots[2])
            .expect("TUI restored future export succeeds"),
    ];
    assert_eq!(
        semantic_export(&restored_exports[0]),
        semantic_export(&future_exports[0])
    );
    assert_eq!(
        semantic_export(&restored_exports[0]),
        semantic_export(&restored_exports[1])
    );
    assert_eq!(
        semantic_export(&restored_exports[0]),
        semantic_export(&restored_exports[2])
    );
    let restored_export_geometry = restored_output_roots
        .iter()
        .map(|output| fs::read(output.join("l-bracket.stl")).expect("restored future STL reads"))
        .collect::<Vec<_>>();
    assert_eq!(restored_export_geometry[0], future_export_geometry[0]);
    assert_eq!(restored_export_geometry[0], restored_export_geometry[1]);
    assert_eq!(restored_export_geometry[0], restored_export_geometry[2]);

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
    for output in future_output_roots.into_iter().chain(restored_output_roots) {
        let _ = fs::remove_dir_all(output);
    }
}

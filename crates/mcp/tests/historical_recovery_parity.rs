use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_cli::dispatch::dispatch;
use threeterm_domain::history::HistoryStatus;
use threeterm_host::{Host, HostError};
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::{BracketRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, CREATE_REVISION_COMMAND_ID, EXPORT_COMMAND_ID, HISTORICAL_EDIT_COMMAND_ID,
    HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION, LOAD_COMMAND_ID, RESTORE_REVISION_COMMAND_ID,
    TIMELINE_COMMAND_ID, UNDO_COMMAND_ID,
};
use threeterm_tui::{FeatureTarget, SelectionEvent, SelectionVerification, TuiSession};

fn temp_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-historical-parity-{label}-{suffix}"))
}

fn copy_historical_fixture(root: &Path) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/research/rehearsal-evidence/l-bracket/run-2/project");
    fs::create_dir_all(root.join("brep")).expect("historical fixture BREP directory creates");
    for file in ["manifest.json", "transactions.log"] {
        fs::copy(fixture.join(file), root.join(file)).expect("historical fixture file copies");
    }
    fs::copy(
        fixture.join("brep/l-bracket.brep"),
        root.join("brep/l-bracket.brep"),
    )
    .expect("historical fixture BREP copies");
}

fn historical_fixture_roots(label: &str) -> [PathBuf; 3] {
    let roots = [
        temp_root(&format!("{label}-cli")),
        temp_root(&format!("{label}-mcp")),
        temp_root(&format!("{label}-tui")),
    ];
    for root in &roots {
        copy_historical_fixture(root);
    }
    roots
}

fn require_occt_worker(test_name: &str) -> Option<OcctWorker> {
    match OcctWorker::locate() {
        Ok(worker) => Some(worker),
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some()
                || std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() =>
        {
            panic!("{test_name}: OCCT worker unavailable: {error}");
        }
        Err(_) => {
            eprintln!("{test_name}: OCCT worker unavailable; skipping");
            None
        }
    }
}

fn require_native_occt_worker(test_name: &str) -> OcctWorker {
    if std::env::var_os("THREETERM_REQUIRE_OCCT").is_none()
        || std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_none()
    {
        panic!(
            "{test_name}: native proof requires THREETERM_REQUIRE_OCCT=1 and THREETERM_REQUIRE_REAL_WORKER=1"
        );
    }
    OcctWorker::locate()
        .unwrap_or_else(|error| panic!("{test_name}: OCCT worker unavailable: {error}"))
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

fn timeline_request(root: &Path, feature_id: &str) -> Value {
    json!({"bundle_path": root.to_string_lossy(), "feature_id": feature_id})
}

fn cli_timeline(root: &Path) -> Value {
    cli_timeline_for(root, "l-bracket")
}

fn cli_timeline_for(root: &Path, feature_id: &str) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("timeline"),
        OsString::from(root),
        OsString::from("--feature-id"),
        OsString::from(feature_id),
    ])
    .expect("CLI timeline succeeds")
}

fn mcp_timeline(root: &Path) -> Value {
    mcp_timeline_for(root, "l-bracket")
}

fn mcp_timeline_for(root: &Path, feature_id: &str) -> Value {
    mcp_call(
        "threeterm.command.timeline/1",
        timeline_request(root, feature_id),
    )
    .expect("MCP timeline succeeds")
}

fn tui_timeline(root: &Path) -> (TuiSession, Value) {
    tui_timeline_for(root, "l-bracket")
}

fn tui_timeline_for(root: &Path, feature_id: &str) -> (TuiSession, Value) {
    let host = Host::new();
    let active_revision = host
        .history(root)
        .expect("TUI reads the canonical history before opening its timeline")
        .active_snapshot()
        .revision_id
        .clone();
    let graph = Bundle::at(root)
        .open()
        .expect("TUI timeline graph reloads")
        .graph;
    let graph_has_target = graph.contains_feature(feature_id)
        || graph.features().any(|feature| {
            feature.id.as_str() == format!("{feature_id}-plate-vertical")
                || feature.id.as_str() == format!("{feature_id}-plate-horizontal")
        });
    let mut session = if feature_id.ends_with("-plate-vertical")
        || feature_id.ends_with("-plate-horizontal")
        || !graph_has_target
    {
        TuiSession::new(
            [FeatureTarget::new(feature_id, "L-bracket")],
            active_revision,
        )
    } else {
        TuiSession::from_feature_graph(&graph, active_revision)
    };
    session
        .transition_selection(SelectionEvent::Nominate {
            candidates: vec![feature_id.to_string()],
        })
        .expect("TUI nominates the selected feature");
    session
        .transition_selection(SelectionEvent::Verify(SelectionVerification::Exact {
            stable_ids: vec![feature_id.to_string()],
        }))
        .expect("TUI verifies the selected feature");
    session
        .open_feature_timeline(&host, root)
        .expect("TUI timeline opens");
    let state = session.state();
    let timeline = state.feature_timeline.expect("TUI timeline state exists");
    let stale_overlay = session.stale_last_valid_geometry_overlay();
    let revisions = timeline
        .revisions
        .iter()
        .map(|revision| {
            json!({
                "ordinal": revision.ordinal,
                "revision_id": revision.revision_id,
                "operation": revision.operation,
                "status": revision.status,
                "stale_last_valid_geometry_fingerprint": revision
                    .stale_last_valid_geometry_fingerprint,
                "named_revision_names": revision.named_revision_names,
            })
        })
        .collect::<Vec<_>>();
    let named_revisions = timeline
        .named_revisions
        .iter()
        .zip(timeline.named_revision_provenance.iter())
        .map(|(name, (provenance_name, provenance))| {
            debug_assert_eq!(name, provenance_name);
            json!({"name": name, "provenance": provenance})
        })
        .collect::<Vec<_>>();
    (
        session,
        json!({
            "feature_id": timeline.feature_id,
            "active_revision": timeline.active_revision,
            "revisions": revisions,
            "named_revisions": named_revisions,
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
            threeterm_protocol::command_execution::ExecutionError::Handler(
                HostError::StaleLastValidGeometry {
                    feature_id,
                    active_revision,
                    stale_features,
                },
            ) => json!({
                "severity": "error",
                "code": "stale_last_valid_geometry",
                "feature_id": feature_id,
                "active_revision": active_revision,
                "stale_features": stale_features,
                "recovery": "correct or restore the feature and recompute current geometry",
                "override_eligible": false,
                "schema_version": threeterm_protocol::schema::EXPORT_RESPONSE_SCHEMA_VERSION,
            }),
            threeterm_protocol::command_execution::ExecutionError::Handler(error) => {
                serde_json::to_value(threeterm_cli::dispatch::host_error_diagnostic(&error))
                    .expect("TUI diagnostic serializes")
            }
            error => json!({"detail": format!("{error:?}")}),
        }
    })
}

fn cli_load(root: &Path) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("load"),
        OsString::from(root),
    ])
    .expect("CLI load succeeds")
}

fn mcp_load(root: &Path) -> Value {
    mcp_call(
        "threeterm.command.load/1",
        json!({"bundle_path": root.to_string_lossy()}),
    )
    .expect("MCP load succeeds")
}

fn tui_load(root: &Path) -> Value {
    tui_call(
        LOAD_COMMAND_ID,
        json!({"bundle_path": root.to_string_lossy()}),
    )
    .expect("TUI load succeeds")
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
    cli_restore_result(root, feature_id, name).expect("CLI named revision restore succeeds")
}

fn cli_restore_result(root: &Path, feature_id: &str, name: &str) -> Result<Value, Value> {
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
}

fn mcp_restore_result(root: &Path, feature_id: &str, name: &str) -> Result<Value, Value> {
    mcp_call(
        "threeterm.command.restore-revision/1",
        json!({
            "bundle_path": root.to_string_lossy(),
            "feature_id": feature_id,
            "name": name,
        }),
    )
}

fn mcp_restore(root: &Path, feature_id: &str, name: &str) -> Value {
    mcp_restore_result(root, feature_id, name).expect("MCP named revision restore succeeds")
}

fn tui_restore(root: &Path, name: &str) -> Value {
    tui_restore_for(root, "l-bracket", name)
}

fn tui_restore_for(root: &Path, feature_id: &str, name: &str) -> Value {
    tui_restore_result_for(root, feature_id, name).expect("TUI named revision restore succeeds")
}

fn tui_restore_result(root: &Path, name: &str) -> Result<Value, Value> {
    tui_restore_result_for(root, "l-bracket", name)
}

fn tui_restore_result_for(root: &Path, feature_id: &str, name: &str) -> Result<Value, Value> {
    let (mut session, _) = tui_timeline_for(root, feature_id);
    let view = session
        .restore_feature_timeline(&Host::new(), root, name)
        .map_err(|error| json!({"code": error.code.as_str(), "detail": error.detail}))?;
    Ok(threeterm_host::history_commit_value(
        "restore-revision",
        HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION,
        &view,
    ))
}

fn tui_export(root: &Path, output_dir: &Path) -> Result<Value, Value> {
    tui_call(EXPORT_COMMAND_ID, export_request(root, output_dir))
}

fn current_brep(root: &Path) -> Vec<u8> {
    fs::read(root.join("brep/l-bracket.brep")).expect("current BREP reads")
}

fn delete_derived_results(root: &Path) {
    for directory in ["brep", "cache", ".derived", "stage"] {
        let path = root.join(directory);
        if path.exists() {
            fs::remove_dir_all(path).expect("derived result directory removes");
        }
    }
}

fn semantic_history_feature(value: &Value) -> Value {
    json!({
        "id": value["id"],
        "status": value["status"],
        "geometry_fingerprint": value["geometry_fingerprint"],
        "last_valid_geometry_fingerprint": value["last_valid_geometry_fingerprint"],
        "stale_last_valid_geometry": value["stale_last_valid_geometry"],
        "diagnostic": value["diagnostic"],
    })
}

fn semantic_named_revision(value: &Value) -> Value {
    json!({
        "name": value["name"],
        "revision_id": value["revision_id"],
        "provenance": value["provenance"],
    })
}

fn semantic_timeline_revision(value: &Value) -> Value {
    json!({
        "ordinal": value["ordinal"],
        "revision_id": value["revision_id"],
        "operation": value["operation"],
        "status": value["status"],
        "stale_last_valid_geometry_fingerprint": value["stale_last_valid_geometry_fingerprint"],
        "named_revision_names": value["named_revision_names"],
    })
}

fn semantic_timeline(value: &Value) -> Value {
    let revisions = value["revisions"]
        .as_array()
        .expect("timeline revisions are an array")
        .iter()
        .map(semantic_timeline_revision)
        .collect::<Vec<_>>();
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
    let named_revision_provenance = value["named_revisions"]
        .as_array()
        .expect("timeline named revisions are an array")
        .iter()
        .map(|revision| revision.get("provenance").cloned().unwrap_or(Value::Null))
        .collect::<Vec<_>>();
    json!({
        "feature_id": value["feature_id"],
        "active_revision": value["active_revision"],
        "revisions": revisions,
        "named_revisions": named_revisions,
        "named_revision_provenance": named_revision_provenance,
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

fn semantic_stale_export_error(value: &Value) -> Value {
    json!({
        "severity": value["severity"],
        "code": value["code"],
        "feature_id": value["feature_id"],
        "active_revision": value["active_revision"],
        "stale_features": value["stale_features"],
        "recovery": value["recovery"],
        "override_eligible": value["override_eligible"],
        "schema_version": value["schema_version"],
    })
}

fn expected_stale_export_features(value: &Value) -> Vec<Value> {
    semantic_stale_features(value)
        .into_iter()
        .map(|feature| {
            json!({
                "feature_id": feature["id"],
                "status": feature["status"],
                "last_valid_geometry_fingerprint": feature["last_valid_geometry_fingerprint"],
            })
        })
        .collect()
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

fn named_revision_semantics(root: &Path, name: &str) -> Value {
    let loaded = Bundle::at(root)
        .open()
        .expect("named revision bundle reloads");
    let revision = loaded
        .history
        .named_revisions()
        .get(name)
        .unwrap_or_else(|| panic!("named revision {name} exists"));
    serde_json::to_value(revision).expect("named revision metadata serializes")
}

fn named_revision_name(value: &Value, provenance: &str) -> String {
    value["named_revisions"]
        .as_array()
        .expect("history named revisions are an array")
        .iter()
        .find(|revision| revision["provenance"] == provenance)
        .and_then(|revision| revision["name"].as_str())
        .expect("history contains a named revision with the expected provenance")
        .to_string()
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

fn bracket_request(root: &Path, bracket_id: &str) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "bracket_id": bracket_id,
        "length": 60.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0,
    })
}

fn cli_create_bracket(root: &Path, bracket_id: &str) -> Value {
    let root = root.to_string_lossy().into_owned();
    cli_call([
        OsString::from("--machine"),
        OsString::from("bracket"),
        OsString::from(root),
        OsString::from("--bracket-id"),
        OsString::from(bracket_id),
        OsString::from("--length"),
        OsString::from("60"),
        OsString::from("--width"),
        OsString::from("30"),
        OsString::from("--height"),
        OsString::from("40"),
        OsString::from("--thickness"),
        OsString::from("3"),
    ])
    .expect("CLI bracket creation succeeds")
}

fn mcp_create_bracket(root: &Path, bracket_id: &str) -> Value {
    mcp_call(
        "threeterm.command.bracket/1",
        bracket_request(root, bracket_id),
    )
    .expect("MCP bracket creation succeeds")
}

fn tui_create_bracket(root: &Path, bracket_id: &str) -> Value {
    tui_call(BRACKET_COMMAND_ID, bracket_request(root, bracket_id))
        .expect("TUI bracket creation succeeds")
}

fn seed_standard_brackets_through_adapters(roots: [&Path; 3], test_name: &str) -> bool {
    if require_occt_worker(test_name).is_none() {
        return false;
    }
    let results = [
        cli_create_bracket(roots[0], "l-bracket"),
        mcp_create_bracket(roots[1], "l-bracket"),
        tui_create_bracket(roots[2], "l-bracket"),
    ];
    assert!(results.iter().all(|value| value["status"] == "ok"));
    true
}

fn current_brep_for(root: &Path, feature_id: &str) -> Vec<u8> {
    fs::read(root.join(format!("brep/{feature_id}.brep"))).expect("current feature BREP reads")
}

fn bundle_inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = fs::read_dir(current)
            .expect("bundle inventory directory reads")
            .map(|entry| entry.expect("bundle inventory entry reads"))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("bundle inventory path is under root")
                .to_path_buf();
            if entry
                .file_type()
                .expect("bundle inventory type reads")
                .is_dir()
            {
                visit(root, &path, files);
            } else {
                files.insert(
                    relative,
                    fs::read(path).expect("bundle inventory file reads"),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn object_timeline_semantic_identity_is_canonical_across_all_adapters() {
    let cli_root = temp_root("object-identity-cli");
    let mcp_root = temp_root("object-identity-mcp");
    let tui_root = temp_root("object-identity-tui");
    if !seed_standard_brackets_through_adapters(
        [&cli_root, &mcp_root, &tui_root],
        "object_timeline_semantic_identity_is_canonical_across_all_adapters",
    ) {
        for root in [cli_root, mcp_root, tui_root] {
            let _ = fs::remove_dir_all(root);
        }
        return;
    }

    let canonical = [
        cli_timeline_for(&cli_root, "l-bracket"),
        mcp_timeline_for(&mcp_root, "l-bracket"),
        tui_timeline_for(&tui_root, "l-bracket").1,
    ];
    let legacy_aliases = [
        cli_timeline_for(&cli_root, "l-bracket-plate-vertical"),
        mcp_timeline_for(&mcp_root, "l-bracket-plate-vertical"),
        tui_timeline_for(&tui_root, "l-bracket-plate-vertical").1,
    ];
    for timeline in canonical.iter().chain(legacy_aliases.iter()) {
        assert_eq!(timeline["feature_id"], "l-bracket");
        assert_eq!(timeline["active_revision"], "history-revision-1");
        assert_eq!(timeline["revisions"][0]["ordinal"], 1);
    }
    assert_eq!(
        semantic_timeline(&canonical[0]),
        semantic_timeline(&canonical[1])
    );
    assert_eq!(
        semantic_timeline(&canonical[0]),
        semantic_timeline(&canonical[2])
    );
    for timeline in &legacy_aliases {
        assert_eq!(
            semantic_timeline(&canonical[0]),
            semantic_timeline(timeline)
        );
    }

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn object_timeline_adapter_parity_matches_registered_cli_mcp_and_tui_payloads() {
    let cli_root = temp_root("object-parity-cli");
    let mcp_root = temp_root("object-parity-mcp");
    let tui_root = temp_root("object-parity-tui");
    if !seed_standard_brackets_through_adapters(
        [&cli_root, &mcp_root, &tui_root],
        "object_timeline_adapter_parity_matches_registered_cli_mcp_and_tui_payloads",
    ) {
        for root in [cli_root, mcp_root, tui_root] {
            let _ = fs::remove_dir_all(root);
        }
        return;
    }
    let revisions = [
        cli_create_revision(&cli_root, "before-edit"),
        mcp_create_revision(&mcp_root, "before-edit"),
        tui_create_revision(&tui_root, "before-edit"),
    ];
    assert!(revisions.iter().all(|value| value["status"] == "ok"));

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
    assert_eq!(timelines[0]["feature_id"], "l-bracket");
    assert_eq!(timelines[0]["active_revision"], "history-revision-1");
    assert_eq!(
        timelines[0]["revisions"]
            .as_array()
            .expect("timeline revisions are ordered")
            .iter()
            .map(|revision| revision["ordinal"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        timelines[0]["named_revisions"][0]["provenance"],
        "explicit-create"
    );

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn object_timeline_restore_preserves_canonical_identity_through_all_adapters() {
    let cli_root = temp_root("object-restore-cli");
    let mcp_root = temp_root("object-restore-mcp");
    let tui_root = temp_root("object-restore-tui");
    if !seed_standard_brackets_through_adapters(
        [&cli_root, &mcp_root, &tui_root],
        "object_timeline_restore_preserves_canonical_identity_through_all_adapters",
    ) {
        for root in [cli_root, mcp_root, tui_root] {
            let _ = fs::remove_dir_all(root);
        }
        return;
    }

    let revisions = [
        cli_create_revision(&cli_root, "before-failure"),
        mcp_create_revision(&mcp_root, "before-failure"),
        tui_create_revision(&tui_root, "before-failure"),
    ];
    assert!(revisions.iter().all(|value| value["status"] == "ok"));

    let edits = [
        cli_historical_edit(&cli_root, 0.0),
        mcp_historical_edit(&mcp_root, 0.0),
        tui_historical_edit(&tui_root, 0.0),
    ];
    assert_eq!(semantic_history(&edits[0]), semantic_history(&edits[1]));
    assert_eq!(semantic_history(&edits[0]), semantic_history(&edits[2]));

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
    assert_eq!(restores[0]["active_revision"], "history-revision-1");
    let restored_timelines = [
        cli_timeline(&cli_root),
        mcp_timeline(&mcp_root),
        tui_timeline(&tui_root).1,
    ];
    assert_eq!(
        semantic_timeline(&restored_timelines[0]),
        semantic_timeline(&restored_timelines[1])
    );
    assert_eq!(
        semantic_timeline(&restored_timelines[0]),
        semantic_timeline(&restored_timelines[2])
    );
    assert_eq!(
        restored_timelines[0]["revisions"]
            .as_array()
            .expect("restored timeline revisions are ordered")
            .last()
            .expect("restored timeline has a compensating event")["operation"],
        "restore-named-revision"
    );

    for root in [&cli_root, &mcp_root, &tui_root] {
        let loaded = Bundle::at(root).open().expect("restored bundle opens");
        assert_eq!(
            loaded.feature_timeline("l-bracket").unwrap().feature_id,
            "l-bracket"
        );
        assert!(
            loaded
                .history
                .active_snapshot()
                .features
                .values()
                .all(|feature| feature.status == HistoryStatus::CurrentValid)
        );
    }

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn object_timeline_adapter_parity_object_timeline_restore_replays_divergence_through_all_adapters()
{
    let cli_root = temp_root("object-divergence-cli");
    let mcp_root = temp_root("object-divergence-mcp");
    let tui_root = temp_root("object-divergence-tui");
    let roots = [&cli_root, &mcp_root, &tui_root];
    if require_occt_worker(
        "object_timeline_adapter_parity_object_timeline_restore_replays_divergence_through_all_adapters",
    )
    .is_none()
    {
        for root in [cli_root, mcp_root, tui_root] {
            let _ = fs::remove_dir_all(root);
        }
        return;
    }

    let first = [
        cli_create_bracket(&cli_root, "first"),
        mcp_create_bracket(&mcp_root, "first"),
        tui_create_bracket(&tui_root, "first"),
    ];
    assert!(first.iter().all(|value| value["status"] == "ok"));
    let second = [
        cli_create_bracket(&cli_root, "second"),
        mcp_create_bracket(&mcp_root, "second"),
        tui_create_bracket(&tui_root, "second"),
    ];
    assert!(second.iter().all(|value| value["status"] == "ok"));
    let second_geometry = Some(roots.map(|root| current_brep_for(root, "second")));

    let undone = [
        cli_undo(&cli_root),
        mcp_undo(&mcp_root),
        tui_undo(&tui_root),
    ];
    assert_eq!(semantic_history(&undone[0]), semantic_history(&undone[1]));
    assert_eq!(semantic_history(&undone[0]), semantic_history(&undone[2]));
    assert_eq!(undone[0]["active_revision"], "history-revision-1");
    let preserved_name = named_revision_name(&undone[0], "undo");

    let third = [
        cli_create_bracket(&cli_root, "third"),
        mcp_create_bracket(&mcp_root, "third"),
        tui_create_bracket(&tui_root, "third"),
    ];
    assert!(third.iter().all(|value| value["status"] == "ok"));

    let divergent_timelines = [
        cli_timeline_for(&cli_root, "second"),
        mcp_timeline_for(&mcp_root, "second"),
        tui_timeline_for(&tui_root, "second").1,
    ];
    assert_eq!(
        semantic_timeline(&divergent_timelines[0]),
        semantic_timeline(&divergent_timelines[1])
    );
    assert_eq!(
        semantic_timeline(&divergent_timelines[0]),
        semantic_timeline(&divergent_timelines[2])
    );
    assert_eq!(divergent_timelines[0]["feature_id"], "second");
    assert_eq!(
        divergent_timelines[0]["active_revision"],
        "history-revision-4"
    );
    assert_eq!(
        divergent_timelines[0]["revisions"]
            .as_array()
            .expect("divergent timeline revisions are ordered")
            .iter()
            .map(|revision| revision["ordinal"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(
        divergent_timelines[0]["revisions"][0]["operation"],
        "initialize-l-bracket"
    );
    assert_eq!(
        divergent_timelines[0]["revisions"][0]["status"],
        "current-valid"
    );
    assert_eq!(divergent_timelines[0]["revisions"][1]["operation"], "undo");
    assert_eq!(divergent_timelines[0]["revisions"][1]["status"], "absent");

    let restored = [
        cli_restore(&cli_root, "second", &preserved_name),
        mcp_restore(&mcp_root, "second", &preserved_name),
        tui_restore_for(&tui_root, "second", &preserved_name),
    ];
    assert_eq!(
        semantic_history(&restored[0]),
        semantic_history(&restored[1])
    );
    assert_eq!(
        semantic_history(&restored[0]),
        semantic_history(&restored[2])
    );
    assert_eq!(restored[0]["active_revision"], "history-revision-2");
    assert_eq!(restored[0]["operation"], "restore-revision");

    let restored_timelines = [
        cli_timeline_for(&cli_root, "second"),
        mcp_timeline_for(&mcp_root, "second"),
        tui_timeline_for(&tui_root, "second").1,
    ];
    assert_eq!(
        semantic_timeline(&restored_timelines[0]),
        semantic_timeline(&restored_timelines[1])
    );
    assert_eq!(
        semantic_timeline(&restored_timelines[0]),
        semantic_timeline(&restored_timelines[2])
    );
    assert_eq!(
        restored_timelines[0]["active_revision"],
        "history-revision-2"
    );
    assert_eq!(
        restored_timelines[0]["revisions"]
            .as_array()
            .expect("restored timeline revisions are ordered")
            .iter()
            .map(|revision| revision["ordinal"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2, 3, 5]
    );
    assert_eq!(
        restored_timelines[0]["revisions"][0]["operation"],
        "initialize-l-bracket"
    );
    assert_eq!(
        restored_timelines[0]["revisions"][0]["status"],
        "current-valid"
    );
    assert_eq!(restored_timelines[0]["revisions"][1]["operation"], "undo");
    assert_eq!(restored_timelines[0]["revisions"][1]["status"], "absent");
    assert_eq!(
        restored_timelines[0]["revisions"][2]["operation"],
        "restore-named-revision"
    );
    assert_eq!(
        restored_timelines[0]["revisions"][2]["status"],
        "current-valid"
    );

    let restored_revisions = roots.map(|root| {
        let loaded = Bundle::at(root).open().expect("restored bundle reloads");
        (
            loaded.feature_graph_hash_hex().to_string(),
            loaded.revision_hash_hex().to_string(),
        )
    });
    for root in roots {
        for directory in ["brep", "cache", ".derived", "stage"] {
            let path = root.join(directory);
            if path.exists() {
                fs::remove_dir_all(path).expect("derived directory removes before reload");
            }
        }
    }
    let reloaded = [
        cli_load(&cli_root),
        mcp_load(&mcp_root),
        tui_load(&tui_root),
    ];
    for (index, root) in roots.into_iter().enumerate() {
        assert_eq!(
            reloaded[index]["feature_graph_hash"],
            restored_revisions[index].0
        );
        assert_eq!(
            reloaded[index]["revision_hash"],
            restored_revisions[index].1
        );
        if let Some(expected_geometry) = &second_geometry {
            assert_eq!(current_brep_for(root, "second"), expected_geometry[index]);
        }
    }
    let reloaded_timelines = [
        cli_timeline_for(&cli_root, "second"),
        mcp_timeline_for(&mcp_root, "second"),
        tui_timeline_for(&tui_root, "second").1,
    ];
    assert_eq!(
        semantic_timeline(&reloaded_timelines[0]),
        semantic_timeline(&restored_timelines[0])
    );
    assert_eq!(
        semantic_timeline(&reloaded_timelines[0]),
        semantic_timeline(&reloaded_timelines[1])
    );
    assert_eq!(
        semantic_timeline(&reloaded_timelines[0]),
        semantic_timeline(&reloaded_timelines[2])
    );

    for (index, root) in roots.into_iter().enumerate() {
        let loaded = Bundle::at(root).open().expect("restored bundle opens");
        let active = loaded.history.active_snapshot();
        assert!(active.features.contains_key("first-base"));
        assert!(active.features.contains_key("second-base"));
        assert!(!active.features.contains_key("third-base"));
        assert!(loaded.graph.contains_feature("first-plate-vertical"));
        assert!(loaded.graph.contains_feature("second-plate-vertical"));
        assert!(!loaded.graph.contains_feature("third-plate-vertical"));
        if let Some(expected_geometry) = &second_geometry {
            assert_eq!(current_brep_for(root, "second"), expected_geometry[index]);
        }
    }

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn object_timeline_invalid_reference_is_atomic_across_all_adapters() {
    let cli_root = temp_root("object-invalid-cli");
    let mcp_root = temp_root("object-invalid-mcp");
    let tui_root = temp_root("object-invalid-tui");
    for root in [&cli_root, &mcp_root, &tui_root] {
        let host = Host::new();
        host.save(root, "seed", "box")
            .expect("seed feature initializes");
        host.create_named_revision(root, "before-bracket")
            .expect("predating named revision initializes");
        host.save_bracket(root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
            .expect("L-bracket history initializes");
        fs::create_dir_all(root.join(".derived")).expect("derived results directory creates");
        fs::write(root.join(".derived/timeline-sentinel"), b"preserve me")
            .expect("derived result sentinel writes");
    }
    let before = [&cli_root, &mcp_root, &tui_root].map(|root| canonical_semantics(root));
    let before_manifest = [&cli_root, &mcp_root, &tui_root]
        .map(|root| fs::read(root.join("manifest.json")).expect("manifest reads"));
    let before_log = [&cli_root, &mcp_root, &tui_root]
        .map(|root| fs::read(root.join("transactions.log")).expect("log reads"));
    let before_inventory = [&cli_root, &mcp_root, &tui_root].map(|root| bundle_inventory(root));

    let cli_error = cli_call([
        OsString::from("--machine"),
        OsString::from("timeline"),
        OsString::from(cli_root.to_string_lossy().into_owned()),
        OsString::from("--feature-id"),
        OsString::from("missing-feature"),
    ])
    .expect_err("CLI rejects an unknown timeline feature");
    let mcp_error = mcp_call(
        "threeterm.command.timeline/1",
        json!({
            "bundle_path": mcp_root.to_string_lossy(),
            "feature_id": "missing-feature",
        }),
    )
    .expect_err("MCP rejects an unknown timeline feature");
    let tui_error = tui_call(
        TIMELINE_COMMAND_ID,
        json!({
            "bundle_path": tui_root.to_string_lossy(),
            "feature_id": "missing-feature",
        }),
    )
    .expect_err("TUI rejects an unknown timeline feature");

    assert_eq!(cli_error["code"], "invalid_request");
    assert_eq!(mcp_error["code"], cli_error["code"]);
    assert_eq!(tui_error["code"], cli_error["code"]);
    assert!(
        cli_error["arg"]
            .as_str()
            .expect("CLI error has an argument")
            .contains("history feature not found")
    );
    assert_eq!(canonical_semantics(&cli_root), before[0]);
    assert_eq!(canonical_semantics(&mcp_root), before[1]);
    assert_eq!(canonical_semantics(&tui_root), before[2]);

    let malformed_role = "l-bracket-plate-diagonal";
    let malformed_cli = cli_call([
        OsString::from("--machine"),
        OsString::from("timeline"),
        OsString::from(cli_root.to_string_lossy().into_owned()),
        OsString::from("--feature-id"),
        OsString::from(malformed_role),
    ])
    .expect_err("CLI rejects an incompatible role-qualified reference");
    let malformed_mcp = mcp_call(
        "threeterm.command.timeline/1",
        json!({
            "bundle_path": mcp_root.to_string_lossy(),
            "feature_id": malformed_role,
        }),
    )
    .expect_err("MCP rejects an incompatible role-qualified reference");
    let malformed_tui = tui_call(
        TIMELINE_COMMAND_ID,
        json!({
            "bundle_path": tui_root.to_string_lossy(),
            "feature_id": malformed_role,
        }),
    )
    .expect_err("TUI rejects an incompatible role-qualified reference");
    assert_eq!(malformed_cli["code"], "invalid_request");
    assert_eq!(malformed_mcp["code"], malformed_cli["code"]);
    assert_eq!(malformed_tui["code"], malformed_cli["code"]);
    let malformed_restore_tui = tui_call(
        RESTORE_REVISION_COMMAND_ID,
        json!({
            "bundle_path": tui_root.to_string_lossy(),
            "feature_id": malformed_role,
            "name": "before-bracket",
        }),
    )
    .expect_err("TUI rejects an incompatible restore reference");
    assert_eq!(malformed_restore_tui["code"], "persistence_failure");

    let restore_cli = cli_restore_result(&cli_root, "l-bracket", "before-bracket");
    let restore_mcp = mcp_restore_result(&mcp_root, "l-bracket", "before-bracket");
    let restore_tui = tui_restore_result(&tui_root, "before-bracket");
    assert!(
        restore_cli.is_err(),
        "CLI rejects a predated named revision"
    );
    assert!(
        restore_mcp.is_err(),
        "MCP rejects a predated named revision"
    );
    assert!(
        restore_tui.is_err(),
        "TUI rejects a predated named revision"
    );
    let restore_tui_error = restore_tui.expect_err("TUI error is structured");
    assert_eq!(restore_tui_error["code"], "history_rejected");

    let malformed_restore_cli = cli_restore_result(&cli_root, malformed_role, "before-bracket");
    let malformed_restore_mcp = mcp_restore_result(&mcp_root, malformed_role, "before-bracket");
    assert!(
        malformed_restore_cli.is_err(),
        "CLI rejects an incompatible restore reference"
    );
    assert!(
        malformed_restore_mcp.is_err(),
        "MCP rejects an incompatible restore reference"
    );

    for (index, root) in [&cli_root, &mcp_root, &tui_root].into_iter().enumerate() {
        assert_eq!(canonical_semantics(root), before[index]);
        assert_eq!(
            fs::read(root.join("manifest.json")).expect("manifest remains readable"),
            before_manifest[index]
        );
        assert_eq!(
            fs::read(root.join("transactions.log")).expect("log remains readable"),
            before_log[index]
        );
        assert_eq!(bundle_inventory(root), before_inventory[index]);
    }

    for root in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(root);
    }
}

fn semantic_history(value: &Value) -> Value {
    let named_revisions = value["named_revisions"]
        .as_array()
        .expect("history named revisions are an array")
        .iter()
        .map(semantic_named_revision)
        .collect::<Vec<_>>();
    let features = value["features"]
        .as_array()
        .expect("history features are an array")
        .iter()
        .map(semantic_history_feature)
        .collect::<Vec<_>>();
    json!({
        "status": value["status"],
        "operation": value["operation"],
        "active_revision": value["active_revision"],
        "dirty_features": value["dirty_features"],
        "evaluated_features": value["evaluated_features"],
        "blocked_features": value["blocked_features"],
        "diagnostics": value["diagnostics"],
        "named_revisions": named_revisions,
        "features": features,
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
                "active_revision": value["active_revision"],
                "last_valid_geometry_fingerprint": feature["last_valid_geometry_fingerprint"],
            })
        })
        .collect()
}

#[test]
fn historical_edit_stop_point() {
    let roots = historical_fixture_roots("stop-point");
    let roots = [&roots[0], &roots[1], &roots[2]];
    let before = roots.clone().map(|root| {
        Bundle::at(root)
            .open()
            .expect("historical fixture opens")
            .history
    });
    let revisions = [
        cli_create_revision(roots[0], "before-failure"),
        mcp_create_revision(roots[1], "before-failure"),
        tui_create_revision(roots[2], "before-failure"),
    ];
    assert!(revisions.iter().all(|value| value["status"] == "ok"));
    let named = roots.map(|root| named_revision_semantics(root, "before-failure"));

    let results = [
        cli_historical_edit(roots[0], 0.0),
        mcp_historical_edit(roots[1], 0.0),
        tui_historical_edit(roots[2], 0.0),
    ];
    assert_eq!(semantic_history(&results[0]), semantic_history(&results[1]));
    assert_eq!(semantic_history(&results[0]), semantic_history(&results[2]));
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
        results[0]["diagnostics"][0]["affected_ids"],
        json!(["l-bracket-base", "l-bracket-bend", "l-bracket-finish"])
    );

    for ((root, prior), named_revision) in roots.into_iter().zip(before).zip(named) {
        let loaded = Bundle::at(root)
            .open()
            .expect("degraded historical fixture reloads");
        let active = loaded.history.active_snapshot();
        assert_eq!(
            active.features["l-bracket-base"].status,
            HistoryStatus::Broken
        );
        assert_eq!(
            active.features["l-bracket-base"].last_valid_geometry_fingerprint,
            prior.active_snapshot().features["l-bracket-base"].geometry_fingerprint
        );
        assert_eq!(active.features["l-bracket-base"].geometry_fingerprint, None);
        for feature_id in ["l-bracket-bend", "l-bracket-finish"] {
            assert_eq!(
                active.features[feature_id].status,
                HistoryStatus::BlockedByFailure
            );
            assert_eq!(
                active.features[feature_id].last_valid_geometry_fingerprint,
                prior.active_snapshot().features[feature_id].geometry_fingerprint
            );
            assert_eq!(active.features[feature_id].geometry_fingerprint, None);
        }
        assert_eq!(
            active.features["l-bracket-independent-base"].status,
            HistoryStatus::CurrentValid
        );
        assert_eq!(
            active.features["l-bracket-independent-base"].geometry_fingerprint,
            prior.active_snapshot().features["l-bracket-independent-base"].geometry_fingerprint
        );
        assert_eq!(
            named_revision,
            named_revision_semantics(root, "before-failure"),
            "invalid edit preserves Named Revision provenance"
        );
    }

    for root in roots {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn historical_edit_unaffected_geometry() {
    let roots = historical_fixture_roots("unaffected-geometry");
    let before = roots.clone().map(|root| {
        Bundle::at(root)
            .open()
            .expect("historical fixture opens")
            .history
    });
    let results = [
        cli_historical_edit(&roots[0], 0.0),
        mcp_historical_edit(&roots[1], 0.0),
        tui_historical_edit(&roots[2], 0.0),
    ];
    assert_eq!(semantic_history(&results[0]), semantic_history(&results[1]));
    assert_eq!(semantic_history(&results[0]), semantic_history(&results[2]));

    for (root, prior) in roots.iter().zip(before) {
        let active = Bundle::at(root)
            .open()
            .expect("degraded historical fixture reloads")
            .history
            .active_snapshot()
            .clone();
        for feature_id in ["l-bracket-independent-base", "l-bracket-independent-finish"] {
            let previous = &prior.active_snapshot().features[feature_id];
            let current = &active.features[feature_id];
            assert_eq!(current.status, HistoryStatus::CurrentValid);
            assert_eq!(current.input_value, previous.input_value);
            assert_eq!(current.geometry_fingerprint, previous.geometry_fingerprint);
            assert_eq!(current.last_valid_geometry_fingerprint, None);
        }
        for feature_id in ["l-bracket-base", "l-bracket-bend", "l-bracket-finish"] {
            assert_eq!(
                active.features[feature_id].last_valid_geometry_fingerprint,
                prior.active_snapshot().features[feature_id].geometry_fingerprint
            );
        }
    }

    for root in roots {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn stale_last_valid_export_refusal() {
    let roots = historical_fixture_roots("stale-export");
    let results = [
        cli_historical_edit(&roots[0], 0.0),
        mcp_historical_edit(&roots[1], 0.0),
        tui_historical_edit(&roots[2], 0.0),
    ];
    let output_roots = [
        temp_root("stale-export-cli"),
        temp_root("stale-export-mcp"),
        temp_root("stale-export-tui"),
    ];
    for output in &output_roots {
        fs::create_dir_all(output).expect("stale export directory creates");
        fs::write(output.join("sentinel.txt"), b"preserve me").expect("export sentinel writes");
    }
    let canonical_before = roots.clone().map(|root| {
        (
            fs::read(root.join("manifest.json")).expect("manifest reads before stale export"),
            fs::read(root.join("transactions.log")).expect("log reads before stale export"),
            bundle_inventory(&root),
        )
    });

    let errors = [
        cli_export(&roots[0], &output_roots[0]).expect_err("CLI refuses stale geometry"),
        mcp_call(
            "threeterm.command.export/1",
            export_request(&roots[1], &output_roots[1]),
        )
        .expect_err("MCP refuses stale geometry"),
        tui_export(&roots[2], &output_roots[2]).expect_err("TUI refuses stale geometry"),
    ];
    let semantic_errors = errors.map(|error| semantic_stale_export_error(&error));
    assert_eq!(semantic_errors[0], semantic_errors[1]);
    assert_eq!(semantic_errors[0], semantic_errors[2]);
    assert_eq!(semantic_errors[0]["code"], "stale_last_valid_geometry");
    assert_eq!(semantic_errors[0]["override_eligible"], false);
    assert_eq!(
        semantic_errors[0]["stale_features"],
        json!(expected_stale_export_features(&results[0]))
    );

    for (index, root) in roots.iter().enumerate() {
        assert_eq!(
            fs::read(root.join("manifest.json")).expect("manifest remains unchanged"),
            canonical_before[index].0
        );
        assert_eq!(
            fs::read(root.join("transactions.log")).expect("log remains unchanged"),
            canonical_before[index].1
        );
        assert_eq!(bundle_inventory(root), canonical_before[index].2);
        assert_eq!(
            fs::read(output_roots[index].join("sentinel.txt")).expect("sentinel remains"),
            b"preserve me"
        );
        assert_eq!(
            fs::read_dir(&output_roots[index])
                .expect("stale output directory reads")
                .count(),
            1
        );
    }

    for root in roots {
        let _ = fs::remove_dir_all(root);
    }
    for output in output_roots {
        let _ = fs::remove_dir_all(output);
    }
}

#[test]
fn historical_named_revision_restore() {
    let Some(worker) = require_occt_worker("historical_named_revision_restore") else {
        return;
    };
    let root = temp_root("named-revision-restore");
    seed_bundle(&root, &worker);
    let before_geometry = current_brep(&root);
    let host = Host::new();
    host.create_named_revision(&root, "before-failure")
        .expect("Named Revision creates before the invalid edit");
    let named_before = named_revision_semantics(&root, "before-failure");
    let log_len_before_edit = Bundle::at(&root)
        .open()
        .expect("bundle opens before the invalid edit")
        .log
        .len();
    host.historical_edit(&root, "l-bracket-base", "length", 0.0)
        .expect("invalid historical edit records a degraded revision");
    delete_derived_results(&root);

    let restored = Host::new()
        .restore_named_revision(&root, "l-bracket", "before-failure")
        .expect("restore recomputes after Derived Result deletion");
    let loaded = Bundle::at(&root).open().expect("restored bundle opens");
    assert_eq!(
        loaded.log.len(),
        log_len_before_edit + 2,
        "historical edit and restore append transactions without rewriting history"
    );
    assert_eq!(
        named_revision_semantics(&root, "before-failure"),
        named_before,
        "restore preserves Named Revision provenance and log position"
    );
    assert!(
        loaded
            .history
            .active_snapshot()
            .features
            .values()
            .all(|feature| {
                feature.status == HistoryStatus::CurrentValid
                    && feature.last_valid_geometry_fingerprint.is_none()
                    && feature.diagnostic.is_none()
            })
    );
    assert_eq!(current_brep(&root), before_geometry);
    assert_eq!(
        restored.history.active_snapshot().revision_id,
        "history-revision-2"
    );

    let output = temp_root("named-revision-restore-export");
    fs::create_dir_all(&output).expect("restored export directory creates");
    let exported = Host::new()
        .export(
            &root,
            "l-bracket",
            &["stl".to_string()],
            &output,
            0.5,
            false,
            true,
            &[],
        )
        .expect("restored current geometry exports");
    assert_eq!(
        exported.source_snapshot.revision_hash,
        loaded.revision_hash_hex()
    );
    assert!(output.join("l-bracket.stl").is_file());

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output);
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
    assert_eq!(
        results[0]["evaluated_features"],
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
            "successful edit preserves the independent feature"
        );
    }
    let successful_canonical =
        [&cli_root, &mcp_root, &tui_root].map(|root| canonical_semantics(root));
    assert_eq!(successful_canonical[0], successful_canonical[1]);
    assert_eq!(successful_canonical[0], successful_canonical[2]);

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
fn historical_recovery_adapter_parity() {
    let Some(worker) = require_occt_worker("historical_recovery_adapter_parity") else {
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
                    "active_revision": feature["active_revision"],
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
    let stale_errors = [&cli_error, &mcp_error, &tui_error].map(semantic_stale_export_error);
    assert_eq!(stale_errors[0], stale_errors[1]);
    assert_eq!(stale_errors[0], stale_errors[2]);
    assert_eq!(stale_errors[0]["severity"], "error");
    assert_eq!(stale_errors[0]["code"], "stale_last_valid_geometry");
    assert_eq!(stale_errors[0]["feature_id"], "l-bracket");
    assert_eq!(
        stale_errors[0]["active_revision"],
        results[0]["active_revision"]
    );
    assert_eq!(
        stale_errors[0]["stale_features"],
        json!(expected_stale_export_features(&results[0]))
    );
    assert_eq!(
        stale_errors[0]["recovery"],
        "correct or restore the feature and recompute current geometry"
    );
    assert_eq!(stale_errors[0]["override_eligible"], false);
    assert_eq!(
        stale_errors[0]["schema_version"],
        threeterm_protocol::schema::EXPORT_RESPONSE_SCHEMA_VERSION
    );
    for output in &output_roots {
        assert_eq!(
            fs::read(output.join("sentinel.txt")).unwrap(),
            b"preserve me"
        );
        let mut entries = fs::read_dir(output)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, vec!["sentinel.txt"]);
    }

    for root in [&cli_root, &mcp_root, &tui_root] {
        delete_derived_results(root);
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
    let restored_canonical =
        [&cli_root, &mcp_root, &tui_root].map(|root| canonical_semantics(root));
    assert_eq!(restored_canonical[0], restored_canonical[1]);
    assert_eq!(restored_canonical[0], restored_canonical[2]);

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
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn object_timeline_restore_preserves_and_restores_the_divergent_named_future_through_all_adapters()
{
    let worker = require_native_occt_worker(
        "divergent_work_preserves_and_restores_the_named_future_through_all_adapters",
    );

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
    assert_eq!(future_semantics[0], future_semantics[1]);
    assert_eq!(future_semantics[0], future_semantics[2]);
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
    let preserved_name = named_revision_name(&undone[0], "undo");
    let preserved_named_revisions = [&cli_root, &mcp_root, &tui_root]
        .map(|root| named_revision_semantics(root, &preserved_name));
    assert_eq!(preserved_named_revisions[0], preserved_named_revisions[1]);
    assert_eq!(preserved_named_revisions[0], preserved_named_revisions[2]);

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
            .any(|revision| revision["name"] == preserved_name)
    );
    for (root, expected) in [&cli_root, &mcp_root, &tui_root]
        .into_iter()
        .zip(preserved_named_revisions)
    {
        assert_eq!(
            named_revision_semantics(root, &preserved_name),
            expected,
            "divergence preserves complete Named Revision metadata"
        );
    }

    let restored = [
        cli_restore(&cli_root, "l-bracket", &preserved_name),
        mcp_restore(&mcp_root, "l-bracket", &preserved_name),
        tui_restore(&tui_root, &preserved_name),
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
    assert_eq!(
        canonical_semantics(&cli_root),
        canonical_semantics(&mcp_root)
    );
    assert_eq!(
        canonical_semantics(&cli_root),
        canonical_semantics(&tui_root)
    );

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

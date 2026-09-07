use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_cli::dispatch::dispatch_registered_command;
use threeterm_host::Host;
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{
    EXPORT_COMMAND_ID, EXTRUDE_COMMAND_ID, FIT_DIMENSION_COMMAND_ID, SKETCH_SOLVE_COMMAND_ID, find,
};
use threeterm_protocol::schema_validator::validate;
use threeterm_slvs_worker::SlvsWorker;
use threeterm_tui::execute_domain_command;
use threeterm_viewport::ViewportScene;

fn root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-box-lid-adapters-{label}-{suffix}"))
}

fn require_native_workers(test_name: &str) -> bool {
    let occt = OcctWorker::locate();
    let slvs = SlvsWorker::locate();
    if occt.is_ok() && slvs.is_ok() {
        return true;
    }
    if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
        || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some()
    {
        panic!("{test_name}: real OCCT and libslvs workers are required");
    }
    eprintln!("{test_name}: real OCCT and libslvs workers unavailable; skipping");
    false
}

fn sketch_request(root: &Path, feature_id: &str, dimension_id: &str, value: f64) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": feature_id,
        "request_id": format!("{feature_id}-request"),
        "entities": [
            {"kind": "point", "id": format!("{feature_id}-p0"), "x": 0.0, "y": 0.0},
            {"kind": "point", "id": format!("{feature_id}-p1"), "x": value, "y": 0.0},
            {
                "kind": "line_segment",
                "id": format!("{feature_id}-edge"),
                "start": format!("{feature_id}-p0"),
                "end": format!("{feature_id}-p1")
            }
        ],
        "constraints": [
            {
                "id": format!("{feature_id}-anchor"),
                "kind": "fixed",
                "entities": [format!("{feature_id}-p0")]
            },
            {
                "id": dimension_id,
                "kind": "distance",
                "entities": [format!("{feature_id}-p0"), format!("{feature_id}-p1")],
                "value": value
            },
            {
                "id": format!("{feature_id}-horizontal"),
                "kind": "horizontal",
                "entities": [format!("{feature_id}-edge")]
            }
        ]
    })
}

fn extrude_request(root: &Path, feature_id: &str, profile: Value, height: f64) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": feature_id,
        "profile": profile,
        "height": height,
        "mode": "additive"
    })
}

fn fit_request(root: &Path, expected_revision: &str, clearance: f64) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "expected_revision": expected_revision,
        "source_feature_id": "box-sketch",
        "target_feature_id": "lid-sketch",
        "source_dimension_id": "box-width",
        "target_dimension_id": "lid-width",
        "dimension": "width",
        "clearance": clearance
    })
}

fn assert_response(command: threeterm_protocol::schema::CommandId, response: &Value) {
    let schema = &find(command)
        .unwrap_or_else(|| panic!("command {} is registered", command.0))
        .response_schema;
    validate(schema, response)
        .unwrap_or_else(|error| panic!("response for {} violates its schema: {error}", command.0));
}

fn mcp_call(server: &McpServer, command: &str, arguments: Value) -> Result<Value, String> {
    let response = server.handle_request(&JsonRpcRequest {
        id: json!(command),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({"name": command, "arguments": arguments}),
    });
    if let Some(error) = response.error {
        return Err(error.message);
    }
    let result = response
        .result
        .ok_or_else(|| "MCP response is empty".to_string())?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(result.to_string());
    }
    result
        .get("structuredContent")
        .cloned()
        .ok_or_else(|| "MCP success has no structured content".to_string())
}

fn tui_call(
    host: &Host,
    command: threeterm_protocol::schema::CommandId,
    mut request: Value,
) -> Result<Value, String> {
    if command == SKETCH_SOLVE_COMMAND_ID {
        let mut preview = request.clone();
        preview["phase"] = Value::String("preview".to_string());
        let preview = host
            .preview_domain_command(command, preview)
            .map_err(|error| format!("TUI preview failed: {error:?}"))?;
        request["phase"] = Value::String("commit".to_string());
        request["preview_revision"] = Value::String(preview.preview_revision);
    }
    execute_domain_command(host, command, request).map_err(|error| format!("{error:?}"))
}

fn build_workflow<F>(root: &Path, mut call: F) -> (Value, Value, Value)
where
    F: FnMut(threeterm_protocol::schema::CommandId, Value) -> Result<Value, String>,
{
    Bundle::create(root).expect("workflow bundle creates");
    let box_sketch = call(
        SKETCH_SOLVE_COMMAND_ID,
        sketch_request(root, "box-sketch", "box-width", 10.0),
    )
    .expect("box sketch commits");
    assert_response(SKETCH_SOLVE_COMMAND_ID, &box_sketch);
    let lid_sketch = call(
        SKETCH_SOLVE_COMMAND_ID,
        sketch_request(root, "lid-sketch", "lid-width", 9.6),
    )
    .expect("lid sketch commits");
    assert_response(SKETCH_SOLVE_COMMAND_ID, &lid_sketch);
    let box_result = call(
        EXTRUDE_COMMAND_ID,
        extrude_request(
            root,
            "box",
            json!([[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]]),
            4.0,
        ),
    )
    .expect("box extrude commits");
    assert_response(EXTRUDE_COMMAND_ID, &box_result);
    let lid = call(
        EXTRUDE_COMMAND_ID,
        extrude_request(
            root,
            "lid",
            json!([[0.2, 0.2], [9.8, 0.2], [9.8, 7.8], [0.2, 7.8]]),
            1.0,
        ),
    )
    .expect("lid extrude commits");
    assert_response(EXTRUDE_COMMAND_ID, &lid);
    let revision = Bundle::at(root)
        .open()
        .expect("workflow bundle opens")
        .revision_hash_hex()
        .to_string();
    let fit = call(FIT_DIMENSION_COMMAND_ID, fit_request(root, &revision, 0.2))
        .expect("fit dimension commits");
    assert_response(FIT_DIMENSION_COMMAND_ID, &fit);
    assert_eq!(fit["fit"]["source_value"], 10.0);
    assert_eq!(fit["fit"]["target_value"], 9.6);
    (box_result, lid, fit)
}

#[derive(Debug, PartialEq)]
struct GeometryEvidence {
    graph_hash: String,
    box_brep: Vec<u8>,
    lid_brep: Vec<u8>,
    fits: Value,
}

fn geometry_evidence(root: &Path) -> GeometryEvidence {
    let loaded = Bundle::at(root).open().expect("workflow bundle reloads");
    let features = loaded
        .graph
        .features()
        .map(|feature| feature.id.as_str().to_string())
        .collect::<Vec<_>>();
    assert!(features.iter().any(|id| id == "box"));
    assert!(features.iter().any(|id| id == "lid"));
    let fits = loaded.graph.fit_dimensions().cloned().collect::<Vec<_>>();
    assert_eq!(fits.len(), 1);
    GeometryEvidence {
        graph_hash: loaded.feature_graph_hash_hex().to_string(),
        box_brep: fs::read(root.join("brep/box.brep")).expect("box BREP reads"),
        lid_brep: fs::read(root.join("brep/lid.brep")).expect("lid BREP reads"),
        fits: serde_json::to_value(fits).expect("fit relation serializes"),
    }
}

fn solid_bounds(scene: &ViewportScene, feature_id: &str) -> ([f64; 3], [f64; 3]) {
    let solid = scene
        .solids
        .iter()
        .find(|solid| solid.feature_id == feature_id)
        .unwrap_or_else(|| panic!("viewport scene contains {feature_id}"));
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for triangle in &solid.triangles {
        for vertex in triangle.vertices {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex[axis]);
                maximum[axis] = maximum[axis].max(vertex[axis]);
            }
        }
    }
    assert!(minimum.iter().all(|value| value.is_finite()));
    assert!(maximum.iter().all(|value| value.is_finite()));
    (minimum, maximum)
}

fn assert_near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "{actual} differs from {expected}"
    );
}

fn assert_viewport_evidence(host: &Host) {
    let scene = host
        .presentation_viewport_scene()
        .expect("production viewport scene builds");
    assert!(scene.features.iter().any(|feature| feature.id == "box"));
    assert!(scene.features.iter().any(|feature| feature.id == "lid"));
    assert_eq!(scene.fit_relationships.len(), 1);
    assert_eq!(scene.solids.len(), 2);
    assert!(
        scene
            .solids
            .iter()
            .all(|solid| solid.feature_id == "box" || solid.feature_id == "lid")
    );
    let fit = &scene.fit_relationships[0];
    assert_eq!(fit.source_feature_id, "box-sketch");
    assert_eq!(fit.target_feature_id, "lid-sketch");
    assert_near(fit.source_value, 10.0);
    assert_near(fit.target_value, 9.6);
    assert_near(fit.clearance, 0.2);

    let (box_min, box_max) = solid_bounds(&scene, "box");
    assert_near(box_max[0] - box_min[0], 10.0);
    assert_near(box_max[1] - box_min[1], 8.0);
    assert_near(box_max[2] - box_min[2], 4.0);
    let (lid_min, lid_max) = solid_bounds(&scene, "lid");
    assert_near(lid_max[0] - lid_min[0], 9.6);
    assert_near(lid_max[1] - lid_min[1], 7.6);
    assert_near(lid_max[2] - lid_min[2], 1.0);
}

fn replay_and_assert(root: &Path) {
    let before_geometry = geometry_evidence(root);
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("transaction log reads");
    let before_revision = Bundle::at(root)
        .open()
        .expect("bundle opens before replay")
        .revision_hash_hex()
        .to_string();

    fs::remove_dir_all(root.join("brep")).expect("derived BREP directory deletes");
    let host = Host::new();
    let replayed = host
        .load_with_geometry_replay(root)
        .expect("production geometry replay restores derived results");
    assert_eq!(replayed.revision_hash, before_revision);
    assert_viewport_evidence(&host);

    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(geometry_evidence(root), before_geometry);
}

fn export_request(root: &Path, output: &Path, feature_id: &str, formats: &[&str]) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": feature_id,
        "body_ids": ["box", "lid"],
        "formats": formats,
        "output_dir": output.to_string_lossy(),
        "tessellation_deflection": 0.1,
        "override_warnings": false,
        "accept_stale_geometry": false
    })
}

fn stl_bounds(bytes: &[u8], feature_id: &str) -> ([f64; 3], [f64; 3]) {
    let text = std::str::from_utf8(bytes).expect("exported STL is ASCII");
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    let mut vertex_count = 0;
    for line in text.lines() {
        let Some(vertex) = line.trim().strip_prefix("vertex ") else {
            continue;
        };
        let values = vertex
            .split_whitespace()
            .map(|value| value.parse::<f64>().expect("STL vertex is numeric"))
            .collect::<Vec<_>>();
        assert_eq!(
            values.len(),
            3,
            "{feature_id} STL vertex has three coordinates"
        );
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(values[axis]);
            maximum[axis] = maximum[axis].max(values[axis]);
        }
        vertex_count += 1;
    }
    assert!(vertex_count > 0, "{feature_id} STL has vertices");
    (minimum, maximum)
}

fn export_and_assert<F>(root: &Path, output: &Path, mut call: F)
where
    F: FnMut(Value) -> Result<Value, String>,
{
    let box_export =
        call(export_request(root, output, "box", &["stl", "3mf"])).expect("box export commits");
    assert_response(EXPORT_COMMAND_ID, &box_export);
    let lid_export =
        call(export_request(root, output, "lid", &["stl"])).expect("lid export commits");
    assert_response(EXPORT_COMMAND_ID, &lid_export);

    let box_stl = fs::read(output.join("box.stl")).expect("box STL publishes");
    let lid_stl = fs::read(output.join("lid.stl")).expect("lid STL publishes");
    let model = fs::read(output.join("box.3mf")).expect("box 3MF publishes");
    assert!(!box_stl.is_empty());
    assert!(!lid_stl.is_empty());
    assert_ne!(box_stl, lid_stl);
    let (box_min, box_max) = stl_bounds(&box_stl, "box");
    assert_near(box_max[0] - box_min[0], 10.0);
    assert_near(box_max[1] - box_min[1], 8.0);
    assert_near(box_max[2] - box_min[2], 4.0);
    let (lid_min, lid_max) = stl_bounds(&lid_stl, "lid");
    assert_near(lid_max[0] - lid_min[0], 9.6);
    assert_near(lid_max[1] - lid_min[1], 7.6);
    assert_near(lid_max[2] - lid_min[2], 1.0);
    assert!(
        model
            .windows(b"name=\"box\"".len())
            .any(|window| { window == b"name=\"box\"" })
    );
    assert!(
        model
            .windows(b"name=\"lid\"".len())
            .any(|window| { window == b"name=\"lid\"" })
    );
    let item_marker = b"<item objectid=";
    assert_eq!(
        model
            .windows(item_marker.len())
            .filter(|window| *window == item_marker)
            .count(),
        2
    );
}

fn invalid_extrude_request(root: &Path) -> Value {
    extrude_request(
        root,
        "invalid",
        json!([[0.0, 0.0], [10.0, 10.0], [0.0, 10.0], [10.0, 0.0]]),
        1.0,
    )
}

fn invalid_geometry_is_atomic<F>(root: &Path, mut call: F)
where
    F: FnMut(threeterm_protocol::schema::CommandId, Value) -> Result<Value, String>,
{
    let before_geometry = geometry_evidence(root);
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("transaction log reads");
    let before_revision = Bundle::at(root)
        .open()
        .expect("bundle opens before invalid geometry")
        .revision_hash_hex()
        .to_string();
    let request = invalid_extrude_request(root);
    validate(
        &find(EXTRUDE_COMMAND_ID)
            .expect("extrude command is registered")
            .request_schema,
        &request,
    )
    .expect("invalid geometry request remains schema-valid");
    let error = call(EXTRUDE_COMMAND_ID, request).expect_err("self-intersecting profile must fail");
    assert!(!error.is_empty());
    assert!(error.to_ascii_lowercase().contains("invalid"), "{error}");
    assert!(
        !Bundle::at(root)
            .open()
            .expect("bundle reloads after invalid geometry")
            .graph
            .contains_feature("invalid")
    );
    assert!(!root.join("brep/invalid.brep").exists());
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(
        Bundle::at(root)
            .open()
            .expect("bundle opens after invalid geometry")
            .revision_hash_hex(),
        before_revision
    );
    assert_eq!(geometry_evidence(root), before_geometry);
}

#[test]
fn cli_and_mcp_export_failures_preserve_the_same_semantic_diagnostic() {
    let bundle = root("export-failure");
    let output = root("export-failure-output");
    Bundle::create(&bundle).expect("failure fixture bundle creates");
    let request = export_request(&bundle, &output, "missing", &["stl"]);

    let cli_error = dispatch_registered_command(&Host::new(), EXPORT_COMMAND_ID, request.clone())
        .expect_err("CLI export of a missing feature fails")
        .to_string();
    let mcp = McpServer::new();
    let wire_name = find(EXPORT_COMMAND_ID)
        .expect("MCP export command is registered")
        .schema_version;
    let mcp_error =
        mcp_call(&mcp, wire_name, request).expect_err("MCP export of a missing feature fails");
    assert!(cli_error.contains("reference is lost"), "{cli_error}");
    assert!(mcp_error.contains("reference is lost"), "{mcp_error}");

    let _ = fs::remove_dir_all(bundle);
    let _ = fs::remove_dir_all(output);
}

#[test]
#[ignore = "slow: composes real OCCT and libslvs workflows through every adapter"]
fn box_with_lid_registered_commands_are_equivalent_across_cli_mcp_and_tui() {
    if !require_native_workers(
        "box_with_lid_registered_commands_are_equivalent_across_cli_mcp_and_tui",
    ) {
        return;
    }
    let cli_root = root("cli");
    let mcp_root = root("mcp");
    let tui_root = root("tui");

    let cli_host = Host::new();
    let cli_evidence = build_workflow(&cli_root, |command, request| {
        dispatch_registered_command(&cli_host, command, request)
            .map_err(|error| format!("CLI command failed: {error:?}"))
    });
    let mcp = McpServer::new();
    let mcp_evidence = build_workflow(&mcp_root, |command, request| {
        let wire_name = find(command)
            .expect("MCP command is registered")
            .schema_version;
        mcp_call(&mcp, wire_name, request)
    });
    let tui_host = Host::new();
    let tui_evidence = build_workflow(&tui_root, |command, request| {
        tui_call(&tui_host, command, request)
    });

    for root in [&cli_root, &mcp_root, &tui_root] {
        replay_and_assert(root);
    }

    invalid_geometry_is_atomic(&cli_root, |command, request| {
        dispatch_registered_command(&cli_host, command, request)
            .map_err(|error| format!("{error:?}"))
    });
    let extrude_wire_name = find(EXTRUDE_COMMAND_ID)
        .expect("MCP extrude command is registered")
        .schema_version;
    invalid_geometry_is_atomic(&mcp_root, |command, request| {
        assert_eq!(command, EXTRUDE_COMMAND_ID);
        mcp_call(&mcp, extrude_wire_name, request)
    });
    invalid_geometry_is_atomic(&tui_root, |command, request| {
        tui_call(&tui_host, command, request)
    });

    let cli_output = root("cli-output");
    export_and_assert(&cli_root, &cli_output, |request| {
        dispatch_registered_command(&cli_host, EXPORT_COMMAND_ID, request)
            .map_err(|error| format!("{error:?}"))
    });
    let mcp_output = root("mcp-output");
    let export_wire_name = find(EXPORT_COMMAND_ID)
        .expect("MCP export command is registered")
        .schema_version;
    export_and_assert(&mcp_root, &mcp_output, |request| {
        mcp_call(&mcp, export_wire_name, request)
    });
    let tui_output = root("tui-output");
    export_and_assert(&tui_root, &tui_output, |request| {
        tui_call(&tui_host, EXPORT_COMMAND_ID, request)
    });

    let cli_geometry = geometry_evidence(&cli_root);
    let mcp_geometry = geometry_evidence(&mcp_root);
    let tui_geometry = geometry_evidence(&tui_root);
    assert_eq!(cli_geometry, mcp_geometry, "canonical geometry differs");
    assert_eq!(cli_geometry, tui_geometry, "canonical geometry differs");
    assert_eq!(cli_evidence.0["feature_id"], "box");
    assert_eq!(cli_evidence.1["feature_id"], "lid");
    assert_eq!(mcp_evidence.0["brep_sha256"], cli_evidence.0["brep_sha256"]);
    assert_eq!(mcp_evidence.1["brep_sha256"], cli_evidence.1["brep_sha256"]);
    assert_eq!(tui_evidence.0["brep_sha256"], cli_evidence.0["brep_sha256"]);
    assert_eq!(tui_evidence.1["brep_sha256"], cli_evidence.1["brep_sha256"]);

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
    let _ = fs::remove_dir_all(cli_output);
    let _ = fs::remove_dir_all(mcp_output);
    let _ = fs::remove_dir_all(tui_output);
}

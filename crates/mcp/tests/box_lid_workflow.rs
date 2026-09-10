use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_cli::dispatch::{DispatchError, dispatch_registered_command};
use threeterm_host::{Host, HostError};
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::{Bundle, CanonicalIntent, EXTRUDE_INTENT_SCHEMA_VERSION};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::schema::{
    EXPORT_COMMAND_ID, EXTRUDE_COMMAND_ID, FIT_DIMENSION_COMMAND_ID, LOAD_COMMAND_ID,
    SKETCH_SOLVE_COMMAND_ID, find,
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
    std::env::temp_dir().join(format!("threeterm-box-lid-{label}-{suffix}"))
}

fn required_workers(test_name: &str) -> bool {
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

fn mcp_try_call(server: &McpServer, wire_name: &str, arguments: Value) -> Result<Value, String> {
    let response = server.handle_request(&JsonRpcRequest {
        id: json!(wire_name),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": wire_name,
            "arguments": arguments,
        }),
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

fn portable_extrude_response(value: &Value) -> Value {
    let mut portable = value.clone();
    let object = portable
        .as_object_mut()
        .expect("extrude response is an object");
    for field in ["request_id", "generation_id", "brep_path"] {
        object.remove(field);
    }
    if let Some(derived) = object
        .get_mut("derived_result")
        .and_then(Value::as_object_mut)
    {
        derived.remove("request_id");
    }
    portable
}

enum AdapterSession {
    Cli { root: PathBuf, host: Host },
    Mcp { root: PathBuf, server: McpServer },
    Tui { root: PathBuf, host: Host },
}

impl AdapterSession {
    fn cli(root: PathBuf) -> Self {
        threeterm_persistence::Bundle::create(&root).expect("CLI bundle creates");
        Self::Cli {
            root,
            host: Host::new(),
        }
    }

    fn mcp(root: PathBuf) -> Self {
        threeterm_persistence::Bundle::create(&root).expect("MCP bundle creates");
        Self::Mcp {
            root,
            server: McpServer::new(),
        }
    }

    fn tui(root: PathBuf) -> Self {
        threeterm_persistence::Bundle::create(&root).expect("TUI bundle creates");
        Self::Tui {
            root,
            host: Host::new(),
        }
    }

    fn root(&self) -> &Path {
        match self {
            Self::Cli { root, .. } | Self::Mcp { root, .. } | Self::Tui { root, .. } => root,
        }
    }

    fn execute(
        &self,
        command: threeterm_protocol::schema::CommandId,
        wire_name: &str,
        request: Value,
    ) -> Value {
        self.try_execute(command, wire_name, request)
            .unwrap_or_else(|error| panic!("adapter {wire_name} command fails: {error}"))
    }

    fn try_execute(
        &self,
        command: threeterm_protocol::schema::CommandId,
        wire_name: &str,
        request: Value,
    ) -> Result<Value, String> {
        match self {
            Self::Cli { host, .. } => dispatch_registered_command(host, command, request)
                .map_err(|error| match error {
                    DispatchError::Host(HostError::DraftInputConflict {
                        draft_id,
                        source_revision,
                        current_revision,
                        recovery,
                    }) => format!(
                        "draft_input_conflict draft={draft_id} source={source_revision} current={current_revision} recovery={recovery}"
                    ),
                    error => format!("{error:?}"),
                }),
            Self::Mcp { server, .. } => mcp_try_call(server, wire_name, request),
            Self::Tui { host, .. } => {
                if command == SKETCH_SOLVE_COMMAND_ID {
                    tui_call(host, command, request)
                } else {
                    execute_domain_command(host, command, request)
                        .map_err(|error| match error {
                            ExecutionError::Handler(HostError::DraftInputConflict {
                                draft_id,
                                source_revision,
                                current_revision,
                                recovery,
                            }) => format!(
                                "draft_input_conflict draft={draft_id} source={source_revision} current={current_revision} recovery={recovery}"
                            ),
                            error => format!("{error:?}"),
                        })
                }
            }
        }
    }

    fn sketch(&self, feature_id: &str, dimension_id: &str, value: f64) -> Value {
        let request = sketch_request(self.root(), feature_id, dimension_id, value);
        let wire = find(SKETCH_SOLVE_COMMAND_ID)
            .expect("sketch-solve is registered")
            .schema_version
            .to_string();
        let response = self.execute(SKETCH_SOLVE_COMMAND_ID, &wire, request);
        assert_response(SKETCH_SOLVE_COMMAND_ID, &response);
        response
    }

    fn extrude(&self, feature_id: &str, profile: Value, height: f64) -> Value {
        let request = extrude_request(self.root(), feature_id, profile, height);
        let wire = find(EXTRUDE_COMMAND_ID)
            .expect("extrude is registered")
            .schema_version
            .to_string();
        let response = self.execute(EXTRUDE_COMMAND_ID, &wire, request);
        assert_response(EXTRUDE_COMMAND_ID, &response);
        response
    }

    fn fit(&self, expected_revision: &str, clearance: f64) -> Value {
        let request = fit_request(self.root(), expected_revision, clearance);
        let wire = find(FIT_DIMENSION_COMMAND_ID)
            .expect("fit-dimension is registered")
            .schema_version
            .to_string();
        let response = self.execute(FIT_DIMENSION_COMMAND_ID, &wire, request);
        assert_response(FIT_DIMENSION_COMMAND_ID, &response);
        response
    }

    fn build_workflow(&self) -> (Value, Value, Value) {
        self.sketch("box-sketch", "box-width", 10.0);
        self.sketch("lid-sketch", "lid-width", 9.6);
        let box_result = self.extrude(
            "box",
            json!([[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]]),
            4.0,
        );
        let lid = self.extrude(
            "lid",
            json!([[0.2, 0.2], [9.8, 0.2], [9.8, 7.8], [0.2, 7.8]]),
            1.0,
        );
        let revision = Bundle::at(self.root())
            .open()
            .expect("workflow bundle opens")
            .revision_hash_hex()
            .to_string();
        let fit = self.fit(&revision, 0.2);
        assert_eq!(fit["fit"]["source_value"], 10.0);
        assert_eq!(fit["fit"]["target_value"], 9.6);
        (box_result, lid, fit)
    }

    fn load(&self) -> Value {
        let wire = find(LOAD_COMMAND_ID)
            .expect("load is registered")
            .schema_version
            .to_string();
        self.execute(
            LOAD_COMMAND_ID,
            &wire,
            json!({"bundle_path": self.root().to_string_lossy()}),
        )
    }

    fn export(&self, feature_id: &str) -> Value {
        let wire = find(EXPORT_COMMAND_ID)
            .expect("export is registered")
            .schema_version
            .to_string();
        self.execute(
            EXPORT_COMMAND_ID,
            &wire,
            json!({
                "bundle_path": self.root().to_string_lossy(),
                "feature_id": feature_id,
                "body_ids": ["box", "lid"],
                "formats": if feature_id == "box" { json!(["stl", "3mf"]) } else { json!(["stl"]) },
                "output_dir": self.root().join("exports").to_string_lossy(),
                "tessellation_deflection": 0.1,
                "override_warnings": false,
                "accept_stale_geometry": false,
            }),
        )
    }
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

#[test]
fn box_lid_adapter_parity() {
    if !required_workers("box_lid_adapter_parity") {
        return;
    }
    let mut sessions = vec![
        AdapterSession::cli(root("parity-cli")),
        AdapterSession::mcp(root("parity-mcp")),
        AdapterSession::tui(root("parity-tui")),
    ];
    let mut outcomes = Vec::new();

    for session in &sessions {
        let (box_result, lid, fit) = session.build_workflow();
        assert_eq!(box_result["feature_id"], "box");
        assert_eq!(lid["feature_id"], "lid");
        assert_eq!(lid["operation"], box_result["operation"]);
        assert_eq!(box_result["worker_fingerprint"]["worker_kind"], "occt");
        assert_eq!(fit["fit"]["source_value"], 10.0);
        assert_eq!(fit["fit"]["target_value"], 9.6);
        assert!(session.root().join("brep/box.brep").is_file());
        assert!(session.root().join("brep/lid.brep").is_file());
        outcomes.push((box_result, lid, fit, geometry_evidence(session.root())));
    }

    assert_eq!(
        outcomes[0].0["brep_sha256"], outcomes[1].0["brep_sha256"],
        "CLI and MCP box geometry differ"
    );
    assert_eq!(
        outcomes[0].0["brep_sha256"], outcomes[2].0["brep_sha256"],
        "CLI and TUI box geometry differ"
    );
    assert_eq!(
        outcomes[0].1["brep_sha256"], outcomes[1].1["brep_sha256"],
        "CLI and MCP lid geometry differ"
    );
    assert_eq!(
        outcomes[0].1["brep_sha256"], outcomes[2].1["brep_sha256"],
        "CLI and TUI lid geometry differ"
    );
    assert_eq!(
        portable_extrude_response(&outcomes[0].0),
        portable_extrude_response(&outcomes[1].0),
        "CLI and MCP box domain results differ"
    );
    assert_eq!(
        portable_extrude_response(&outcomes[0].0),
        portable_extrude_response(&outcomes[2].0),
        "CLI and TUI box domain results differ"
    );
    assert_eq!(
        portable_extrude_response(&outcomes[0].1),
        portable_extrude_response(&outcomes[1].1),
        "CLI and MCP lid domain results differ"
    );
    assert_eq!(
        portable_extrude_response(&outcomes[0].1),
        portable_extrude_response(&outcomes[2].1),
        "CLI and TUI lid domain results differ"
    );
    assert_eq!(outcomes[0].3, outcomes[1].3, "canonical geometry differs");
    assert_eq!(outcomes[0].3, outcomes[2].3, "canonical geometry differs");

    for session in sessions.drain(..) {
        let _ = fs::remove_dir_all(session.root());
    }
}

fn is_revision_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn assert_extrude_intent(
    root: &Path,
    feature_id: &str,
    expected_profile: &[[f64; 2]],
    expected_height: f64,
) -> String {
    let bundle = Bundle::at(root).open().expect("intent bundle opens");
    let entry = bundle
        .log
        .entries()
        .iter()
        .find(|entry| {
            entry.feature_id == feature_id
                && matches!(entry.intent.as_ref(), Some(CanonicalIntent::Extrude(_)))
        })
        .expect("canonical extrude entry exists");
    let Some(CanonicalIntent::Extrude(intent)) = entry.intent.as_ref() else {
        panic!("canonical extrude entry carries an extrude intent");
    };
    assert_eq!(intent.schema_version, EXTRUDE_INTENT_SCHEMA_VERSION);
    assert_eq!(intent.command, "extrude");
    assert_eq!(intent.operation, "additive");
    assert_eq!(intent.mode, "additive");
    assert!(intent.target_feature_id.is_none());
    assert!(!intent.request_id.is_empty());
    assert_eq!(intent.deterministic_inputs.height, expected_height);
    assert_eq!(
        intent.deterministic_inputs.profile,
        expected_profile.to_vec()
    );
    assert_eq!(intent.affected_semantic_ids, vec![feature_id.to_string()]);
    assert!(is_revision_hex(&intent.source_revision));
    assert_eq!(intent.worker_requirements.worker_kind, "occt");
    assert!(!intent.worker_requirements.worker_schema_version.is_empty());
    assert!(
        !intent
            .worker_requirements
            .protocol_schema_version
            .is_empty()
    );
    intent
        .validate(feature_id)
        .expect("stored extrude intent validates");
    intent.request_id.clone()
}

#[test]
fn box_lid_canonical_intent() {
    if !required_workers("box_lid_canonical_intent") {
        return;
    }
    let box_profile = [[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]];
    let lid_profile = [[0.2, 0.2], [9.8, 0.2], [9.8, 7.8], [0.2, 7.8]];
    let mut sessions = vec![
        AdapterSession::cli(root("intent-cli")),
        AdapterSession::mcp(root("intent-mcp")),
        AdapterSession::tui(root("intent-tui")),
    ];
    let mut observations = Vec::new();

    for session in &sessions {
        let (box_result, lid, fit) = session.build_workflow();
        let box_request_id = assert_extrude_intent(session.root(), "box", &box_profile, 4.0);
        let lid_request_id = assert_extrude_intent(session.root(), "lid", &lid_profile, 1.0);
        assert_ne!(box_request_id, lid_request_id);
        for (response, request_id) in [(&box_result, &box_request_id), (&lid, &lid_request_id)] {
            if let Some(derived) = response.get("derived_result") {
                assert_eq!(derived["request_id"].as_str(), Some(request_id.as_str()));
            }
        }

        let loaded = Bundle::at(session.root())
            .open()
            .expect("intent bundle reloads");
        assert!(loaded.graph.contains_feature("box"));
        assert!(loaded.graph.contains_feature("lid"));
        assert!(loaded.graph.sketch("box-sketch").is_some());
        assert!(loaded.graph.sketch("lid-sketch").is_some());
        let fits = loaded.graph.fit_dimensions().cloned().collect::<Vec<_>>();
        assert_eq!(fits.len(), 1);
        assert_eq!(
            fits[0].id,
            "fit:box-sketch:lid-sketch:width:box-width:lid-width"
        );
        assert_eq!(fits[0].source_value, 10.0);
        assert_eq!(fits[0].target_value, 9.6);
        assert_eq!(fits[0].clearance, 0.2);
        assert_eq!(fit["fit"]["source_value"], 10.0);
        assert_eq!(fit["fit"]["target_value"], 9.6);
        let extrude_entries = loaded
            .log
            .entries()
            .iter()
            .filter(|entry| matches!(entry.intent.as_ref(), Some(CanonicalIntent::Extrude(_))))
            .count();
        assert_eq!(extrude_entries, 2);

        observations.push(json!({
            "box_request_id": box_request_id,
            "lid_request_id": lid_request_id,
            "graph_hash": loaded.feature_graph_hash_hex(),
            "revision": loaded.revision_hash_hex(),
            "transaction_count": loaded.log.len(),
            "terminal_log_digest": loaded.manifest.terminal_log_digest,
            "worker_fingerprint": serde_json::to_value(&loaded.manifest.occt_worker).expect("worker fingerprint serializes"),
            "fits": serde_json::to_value(&fits).expect("fits serialize"),
            "box_brep_sha256": box_result["brep_sha256"],
            "lid_brep_sha256": lid["brep_sha256"],
        }));
    }

    assert_eq!(observations.len(), 3);
    // Request ids are per-bundle worker request identities; the durable
    // semantic shape (hashes, counts, fits, geometry) must be equivalent.
    // Worker request ids must at least be well-formed on every adapter.
    for observation in &observations {
        assert!(
            !observation["box_request_id"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
        assert!(
            !observation["lid_request_id"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
    }
    for key in [
        "graph_hash",
        "revision",
        "transaction_count",
        "terminal_log_digest",
        "worker_fingerprint",
        "fits",
        "box_brep_sha256",
        "lid_brep_sha256",
    ] {
        assert_eq!(
            observations[0][key], observations[1][key],
            "CLI and MCP canonical intent differ at {key}"
        );
        assert_eq!(
            observations[0][key], observations[2][key],
            "CLI and TUI canonical intent differ at {key}"
        );
    }

    for session in sessions.drain(..) {
        let _ = fs::remove_dir_all(session.root());
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

fn viewport_shape(scene: &ViewportScene) -> Value {
    json!({
        "features": scene.features.iter().map(|feature| json!({
            "id": feature.id,
            "kind": feature.kind,
        })).collect::<Vec<_>>(),
        "solids": scene.solids.iter().map(|solid| json!({
            "feature_id": solid.feature_id,
            "triangles": solid.triangles.iter().map(|triangle| triangle.vertices).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "selected_id": scene.selected_id,
        "layer1_references": scene.layer1_references,
        "fit_relationships": scene.fit_relationships,
    })
}

fn portable_export_response(value: &Value) -> Value {
    let mut portable = value.clone();
    if let Some(object) = portable.as_object_mut() {
        object.remove("generation_id");
    }
    if let Some(artifacts) = portable["artifacts"].as_array_mut() {
        for artifact in artifacts {
            let path = artifact
                .as_str()
                .and_then(|path| Path::new(path).file_name())
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            *artifact = json!(path);
        }
    }
    if let Some(derived) = portable["derived_artifacts"].as_array_mut() {
        for artifact in derived {
            artifact
                .as_object_mut()
                .expect("derived export artifact is an object")
                .remove("request_id");
        }
    }
    portable
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

#[test]
fn box_lid_artifact_discard_replay() {
    if !required_workers("box_lid_artifact_discard_replay") {
        return;
    }
    let mut sessions = vec![
        AdapterSession::cli(root("replay-cli")),
        AdapterSession::mcp(root("replay-mcp")),
        AdapterSession::tui(root("replay-tui")),
    ];
    let mut observations = Vec::new();

    for session in &sessions {
        session.build_workflow();
        let before = Bundle::at(session.root())
            .open()
            .expect("bundle opens before reload");
        let expected_revision = before.revision_hash_hex().to_string();
        let expected_graph = before.feature_graph_hash_hex().to_string();
        let expected_box =
            sha256_hex(&fs::read(session.root().join("brep/box.brep")).expect("box BREP reads"));
        let expected_lid =
            sha256_hex(&fs::read(session.root().join("brep/lid.brep")).expect("lid BREP reads"));
        let expected_terminal_log_digest = before.manifest.terminal_log_digest.clone();
        let expected_worker_fingerprint = serde_json::to_value(&before.manifest.occt_worker)
            .expect("reload worker fingerprint serializes");
        let expected_transaction_count = before.log.len();
        let transient_export_stage = session.root().join("exports/.threeterm-export-fixture");
        fs::create_dir_all(&transient_export_stage).expect("transient export stage creates");
        fs::write(transient_export_stage.join("partial.brep"), b"discardable")
            .expect("transient export artifact writes");
        let manifest_before = fs::read(session.root().join("manifest.json"))
            .expect("manifest reads before derived deletion");
        let transactions_before = fs::read(session.root().join("transactions.log"))
            .expect("transactions read before derived deletion");

        fs::remove_dir_all(session.root().join("brep")).expect("derived BREP directory deletes");
        for disposable in ["cache", ".derived"] {
            let path = session.root().join(disposable);
            if path.exists() {
                fs::remove_dir_all(path).expect("disposable derived directory removes");
            }
        }
        if let Ok(entries) = fs::read_dir(session.root().join("exports")) {
            for entry in entries {
                let entry = entry.expect("export staging entry reads");
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".threeterm-export-")
                {
                    fs::remove_dir_all(entry.path()).expect("transient export stage removes");
                }
            }
        }
        assert!(!transient_export_stage.exists());

        let loaded = session.load();
        assert_eq!(
            loaded["schema_version"],
            "threeterm.command.load.response/2"
        );
        assert_eq!(loaded["feature_graph_hash"], expected_graph);
        assert_eq!(loaded["revision_hash"], expected_revision);
        assert!(loaded["recovered_from_previous"].is_boolean());
        assert_eq!(
            fs::read(session.root().join("manifest.json")).expect("manifest reads after reload"),
            manifest_before
        );
        assert_eq!(
            fs::read(session.root().join("transactions.log"))
                .expect("transactions read after reload"),
            transactions_before
        );
        assert!(session.root().join("brep/box.brep").is_file());
        assert!(session.root().join("brep/lid.brep").is_file());
        assert_eq!(
            sha256_hex(
                &fs::read(session.root().join("brep/box.brep"))
                    .expect("box BREP reads after reload")
            ),
            expected_box
        );
        assert_eq!(
            sha256_hex(
                &fs::read(session.root().join("brep/lid.brep"))
                    .expect("lid BREP reads after reload")
            ),
            expected_lid
        );
        let after = Bundle::at(session.root())
            .open()
            .expect("bundle opens after reload");
        assert_eq!(after.log.len(), expected_transaction_count);
        assert_eq!(
            after.manifest.terminal_log_digest,
            expected_terminal_log_digest
        );
        assert_eq!(
            serde_json::to_value(&after.manifest.occt_worker)
                .expect("reloaded worker fingerprint serializes"),
            expected_worker_fingerprint
        );

        let loaded_again = session.load();
        assert_eq!(
            loaded_again["schema_version"],
            "threeterm.command.load.response/2"
        );
        assert_eq!(loaded_again["feature_graph_hash"], expected_graph);
        assert_eq!(loaded_again["revision_hash"], expected_revision);
        assert_eq!(
            sha256_hex(
                &fs::read(session.root().join("brep/box.brep"))
                    .expect("box BREP reads after second reload")
            ),
            expected_box
        );
        assert_eq!(
            sha256_hex(
                &fs::read(session.root().join("brep/lid.brep"))
                    .expect("lid BREP reads after second reload")
            ),
            expected_lid
        );

        let host = Host::new();
        host.load(session.root())
            .expect("host reloads fitted box and lid");
        assert_viewport_evidence(&host);
        let scene = host
            .presentation_viewport_scene()
            .expect("production viewport scene builds");

        let box_export = session.export("box");
        assert_eq!(box_export["status"], "ok");
        assert_response(EXPORT_COMMAND_ID, &box_export);
        let lid_export = session.export("lid");
        assert_eq!(lid_export["status"], "ok");
        assert_response(EXPORT_COMMAND_ID, &lid_export);
        let mut file_shapes = Vec::new();
        let box_stl = fs::read(session.root().join("exports/box.stl")).expect("box STL publishes");
        let lid_stl = fs::read(session.root().join("exports/lid.stl")).expect("lid STL publishes");
        let model = fs::read(session.root().join("exports/box.3mf")).expect("box 3MF publishes");
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
        for (bytes, format) in [(&box_stl, "stl"), (&model, "3mf")] {
            file_shapes.push(json!({
                "format": format,
                "bytes": bytes.len(),
                "sha256": sha256_hex(bytes),
            }));
        }
        file_shapes.push(json!({
            "format": "lid-stl",
            "bytes": lid_stl.len(),
            "sha256": sha256_hex(&lid_stl),
        }));

        observations.push(json!({
            "box_sha256": expected_box,
            "lid_sha256": expected_lid,
            "transaction_count": expected_transaction_count,
            "terminal_log_digest": expected_terminal_log_digest,
            "worker_fingerprint": expected_worker_fingerprint,
            "viewport": viewport_shape(&scene),
            "box_export": portable_export_response(&box_export),
            "lid_export": portable_export_response(&lid_export),
            "files": file_shapes,
        }));
    }

    assert_eq!(observations.len(), 3);
    assert!(observations.iter().all(|observation| {
        observation["box_sha256"] == observations[0]["box_sha256"]
            && observation["lid_sha256"] == observations[0]["lid_sha256"]
            && observation["transaction_count"] == observations[0]["transaction_count"]
            && observation["terminal_log_digest"] == observations[0]["terminal_log_digest"]
            && observation["worker_fingerprint"] == observations[0]["worker_fingerprint"]
            && observation["viewport"] == observations[0]["viewport"]
            && observation["box_export"] == observations[0]["box_export"]
            && observation["lid_export"] == observations[0]["lid_export"]
            && observation["files"] == observations[0]["files"]
    }));

    for session in sessions.drain(..) {
        let _ = fs::remove_dir_all(session.root());
    }
}

#[test]
fn box_lid_failure_atomicity() {
    if !required_workers("box_lid_failure_atomicity") {
        return;
    }
    let mut sessions = vec![
        AdapterSession::cli(root("failure-cli")),
        AdapterSession::mcp(root("failure-mcp")),
        AdapterSession::tui(root("failure-tui")),
    ];
    let mut summaries = Vec::new();

    for session in &sessions {
        session.build_workflow();
        let manifest_before = fs::read(session.root().join("manifest.json"))
            .expect("failure manifest reads before invalid input");
        let transactions_before = fs::read(session.root().join("transactions.log"))
            .expect("failure transactions read before invalid input");
        let box_before = fs::read(session.root().join("brep/box.brep"))
            .expect("failure box BREP reads before invalid input");
        let lid_before = fs::read(session.root().join("brep/lid.brep"))
            .expect("failure lid BREP reads before invalid input");
        let revision_before = Bundle::at(session.root())
            .open()
            .expect("failure bundle opens before invalid input")
            .revision_hash_hex()
            .to_string();
        let geometry_before = geometry_evidence(session.root());

        let fit_wire = find(FIT_DIMENSION_COMMAND_ID)
            .expect("fit-dimension is registered")
            .schema_version
            .to_string();
        let extrude_wire = find(EXTRUDE_COMMAND_ID)
            .expect("extrude is registered")
            .schema_version
            .to_string();
        let export_wire = find(EXPORT_COMMAND_ID)
            .expect("export is registered")
            .schema_version
            .to_string();

        // Invalid fit: clearance 0.3 violates target == source - 2 * clearance
        // (10.0 - 0.6 = 9.4 != 9.6).
        let invalid_fit = session.try_execute(
            FIT_DIMENSION_COMMAND_ID,
            &fit_wire,
            fit_request(session.root(), &revision_before, 0.3),
        );
        let invalid_fit_error = invalid_fit.expect_err("invalid fit clearance must fail");
        assert!(
            invalid_fit_error.to_ascii_lowercase().contains("fit"),
            "{invalid_fit_error}"
        );

        // Stale expected revision.
        let stale_fit = session.try_execute(
            FIT_DIMENSION_COMMAND_ID,
            &fit_wire,
            fit_request(session.root(), &"f".repeat(64), 0.2),
        );
        let stale_fit_error = stale_fit.expect_err("stale fit revision must fail");
        assert!(
            stale_fit_error.to_ascii_lowercase().contains("revision"),
            "{stale_fit_error}"
        );

        // Invalid geometry: self-intersecting profile must be rejected by the
        // supervised worker path without promoting a Derived Result.
        let invalid_extrude = session.try_execute(
            EXTRUDE_COMMAND_ID,
            &extrude_wire,
            extrude_request(
                session.root(),
                "invalid",
                json!([[0.0, 0.0], [10.0, 10.0], [0.0, 10.0], [10.0, 0.0]]),
                1.0,
            ),
        );
        let invalid_extrude_error =
            invalid_extrude.expect_err("self-intersecting profile must fail");
        assert!(
            invalid_extrude_error
                .to_ascii_lowercase()
                .contains("invalid"),
            "{invalid_extrude_error}"
        );
        assert!(
            !Bundle::at(session.root())
                .open()
                .expect("bundle reloads after invalid geometry")
                .graph
                .contains_feature("invalid")
        );
        assert!(!session.root().join("brep/invalid.brep").exists());

        // Missing-feature export must fail without mutating canonical state.
        let missing_export = session.try_execute(
            EXPORT_COMMAND_ID,
            &export_wire,
            json!({
                "bundle_path": session.root().to_string_lossy(),
                "feature_id": "missing",
                "body_ids": ["box", "lid"],
                "formats": ["stl"],
                "output_dir": session.root().join("exports").to_string_lossy(),
                "tessellation_deflection": 0.1,
                "override_warnings": false,
                "accept_stale_geometry": false,
            }),
        );
        assert!(
            missing_export.is_err(),
            "export of a missing feature must fail"
        );

        assert_eq!(
            fs::read(session.root().join("manifest.json")).unwrap(),
            manifest_before
        );
        assert_eq!(
            fs::read(session.root().join("transactions.log")).unwrap(),
            transactions_before
        );
        assert_eq!(
            fs::read(session.root().join("brep/box.brep")).unwrap(),
            box_before
        );
        assert_eq!(
            fs::read(session.root().join("brep/lid.brep")).unwrap(),
            lid_before
        );
        assert_eq!(
            Bundle::at(session.root())
                .open()
                .expect("bundle opens after invalid input")
                .revision_hash_hex(),
            revision_before
        );
        assert_eq!(geometry_evidence(session.root()), geometry_before);

        // Current state still exports after the rejected inputs.
        let box_export = session.export("box");
        assert_eq!(box_export["status"], "ok");

        summaries.push(json!({
            "invalid_fit_is_fit_error": invalid_fit_error.to_ascii_lowercase().contains("fit"),
            "stale_fit_is_revision_error": stale_fit_error.to_ascii_lowercase().contains("revision"),
            "invalid_extrude_is_invalid_error": invalid_extrude_error.to_ascii_lowercase().contains("invalid"),
        }));
    }

    assert_eq!(summaries.len(), 3);
    assert!(summaries.iter().all(|summary| summary == &summaries[0]));

    for session in sessions.drain(..) {
        let _ = fs::remove_dir_all(session.root());
    }
}

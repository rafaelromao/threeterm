use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_cli::dispatch::{DispatchError, dispatch_registered_command};
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::{Bundle, write_fresh};
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, BOOLEAN_COMMON_COMMAND_ID, BOOLEAN_CUT_COMMAND_ID, CommandSchema,
    EXPORT_COMMAND_ID, EXTRUDE_COMMAND_ID, HISTORICAL_EDIT_COMMAND_ID, HOLE_COMMAND_ID,
    IDENTITY_COMMAND_ID, LOAD_COMMAND_ID, RESTORE_REVISION_COMMAND_ID, SKETCH_SOLVE_COMMAND_ID,
    iter,
};
use threeterm_protocol::schema_validator::validate;
use threeterm_slvs_worker::SlvsWorker;

fn root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-parity-{label}-{suffix}"))
}

fn schema_example(schema: &Value) -> Value {
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    if let Some(value) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return value.clone();
    }
    if let Some(alternatives) = schema.get("oneOf").and_then(Value::as_array) {
        let parent_is_object = schema.get("type").and_then(Value::as_str) == Some("object")
            || schema.get("properties").is_some();
        for alternative in alternatives {
            let mut candidate = if parent_is_object {
                schema_example_without_combinators(schema)
            } else {
                schema_example(alternative)
            };
            if parent_is_object {
                apply_schema_alternative(&mut candidate, schema, alternative);
            }
            if validate(schema, &candidate).is_ok() {
                return candidate;
            }
        }
        panic!("schema has no valid fixture alternative: {schema}");
    }
    schema_example_without_combinators(schema)
}

fn schema_example_without_combinators(schema: &Value) -> Value {
    let Some(schema_object) = schema.as_object() else {
        return Value::Null;
    };
    let kind = schema_object.get("type").and_then(Value::as_str);
    if kind == Some("object") || (kind.is_none() && schema_object.contains_key("properties")) {
        let mut object = serde_json::Map::new();
        if let Some(required) = schema_object.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                let property = schema_object
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|properties| properties.get(key))
                    .unwrap_or_else(|| panic!("required fixture property is missing: {key}"));
                object.insert(key.to_string(), schema_example(property));
            }
        }
        return Value::Object(object);
    }
    match kind {
        Some("array") => {
            let count = schema_object
                .get("minItems")
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize;
            let item_schema = schema_object.get("items");
            Value::Array(
                (0..count)
                    .map(|_| item_schema.map_or(Value::Null, schema_example))
                    .collect(),
            )
        }
        Some("string") => {
            if schema_object
                .get("pattern")
                .and_then(Value::as_str)
                .is_some_and(|pattern| pattern.contains("[0-9a-f]"))
            {
                Value::String("0".repeat(64))
            } else {
                Value::String("fixture".to_string())
            }
        }
        Some("number") => {
            let minimum = schema_object
                .get("minimum")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let exclusive_minimum = schema_object
                .get("exclusiveMinimum")
                .and_then(Value::as_f64)
                .unwrap_or(minimum);
            json!(minimum.max(exclusive_minimum) + 1.0)
        }
        Some("integer") => json!(1),
        Some("boolean") => json!(false),
        Some("null") => Value::Null,
        Some("object") => Value::Object(serde_json::Map::new()),
        None => Value::Null,
        Some(other) => panic!("unsupported fixture schema type: {other}"),
    }
}

fn apply_schema_alternative(candidate: &mut Value, schema: &Value, alternative: &Value) {
    let object = candidate
        .as_object_mut()
        .expect("oneOf request fixture is an object");
    let alternative_object = alternative
        .as_object()
        .expect("oneOf alternative is an object");
    if let Some(required) = alternative_object.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                let property = alternative_object
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|properties| properties.get(key))
                    .or_else(|| {
                        schema
                            .get("properties")
                            .and_then(Value::as_object)
                            .and_then(|properties| properties.get(key))
                    })
                    .unwrap_or_else(|| panic!("oneOf fixture property is missing: {key}"));
                object.insert(key.to_string(), schema_example(property));
            }
        }
    }
    if let Some(properties) = alternative_object
        .get("properties")
        .and_then(Value::as_object)
    {
        for (key, property) in properties {
            if property == &Value::Bool(false) {
                object.remove(key);
            } else {
                object.insert(key.clone(), schema_example(property));
            }
        }
    }
}

fn registry_request(schema: &CommandSchema, root: &std::path::Path) -> Value {
    let mut request = schema_example(&schema.request_schema);
    let hash = "0".repeat(64);
    rewrite_fixture_paths(&mut request, root, &hash);
    validate(&schema.request_schema, &request).unwrap_or_else(|error| {
        panic!(
            "generated fixture for {} violates its request schema: {error}",
            schema.name
        )
    });
    request
}

fn rewrite_fixture_paths(value: &mut Value, root: &std::path::Path, hash: &str) {
    let Some(object) = value.as_object_mut() else {
        if let Some(items) = value.as_array_mut() {
            for item in items {
                rewrite_fixture_paths(item, root, hash);
            }
        }
        return;
    };
    for (key, item) in object {
        match key.as_str() {
            "bundle_path" => *item = json!(root.to_string_lossy().into_owned()),
            "destination" => {
                *item = json!(root.join("new-project").to_string_lossy().into_owned());
            }
            "output_dir" => {
                *item = json!(root.join("rehearsal").to_string_lossy().into_owned());
            }
            "expected_revision" | "source_revision_id" => *item = json!(hash),
            _ => rewrite_fixture_paths(item, root, hash),
        }
    }
}

fn prepare_registry_bundle(request: &Value) {
    if let Some(path) = request.get("bundle_path").and_then(Value::as_str) {
        Bundle::create(path).expect("registry adapter fixture bundle creates");
    }
}

fn assert_cli_registry_result(schema: &CommandSchema, result: Result<Value, DispatchError>) {
    match result {
        Ok(response) => validate(&schema.response_schema, &response).unwrap_or_else(|error| {
            panic!(
                "CLI response for {} violates its response schema: {error}",
                schema.name
            )
        }),
        Err(DispatchError::UnknownCommand(command)) => {
            panic!("CLI registry command {} was not recognized", command.0)
        }
        Err(DispatchError::UnsupportedTool { .. }) => {
            panic!("CLI registry command {} was unsupported", schema.name)
        }
        Err(error) => {
            let detail = error.diagnostic_detail();
            assert!(
                !detail.contains("not handled by the domain executor"),
                "CLI registry command {} has no executable handler: {detail}",
                schema.name
            );
        }
    }
}

fn assert_mcp_registry_result(
    schema: &CommandSchema,
    response: &threeterm_mcp::server::JsonRpcResponse,
) {
    if let Some(error) = &response.error {
        assert_ne!(
            error.code,
            threeterm_mcp::server::ERROR_METHOD_NOT_FOUND,
            "MCP registry command {} was not recognized: {}",
            schema.name,
            error.message
        );
        assert_ne!(
            error.code,
            threeterm_mcp::server::ERROR_INTERNAL,
            "MCP registry command {} violated its response contract: {}",
            schema.name,
            error.message
        );
        return;
    }

    let result = response
        .result
        .as_ref()
        .expect("MCP response has result or error");
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| content.first())
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !text.contains("not handled by the domain executor")
                && !text.contains("unsupported tool"),
            "MCP registry command {} has no executable handler: {text}",
            schema.name
        );
        return;
    }

    let value = result
        .get("structuredContent")
        .cloned()
        .or_else(|| {
            result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str(text).ok())
        })
        .expect("MCP success contains the structured response");
    validate(&schema.response_schema, &value).unwrap_or_else(|error| {
        panic!(
            "MCP response for {} violates its response schema: {error}",
            schema.name
        )
    });
}

#[test]
fn executable_registry_extrude_reaches_cli_and_mcp_executor_and_validates_response() {
    let root = root("registry-adapter-parity");

    for schema in iter() {
        let cli_root = root.join("cli").join(schema.name);
        let mcp_root = root.join("mcp").join(schema.name);
        let cli_request = registry_request(schema, &cli_root);
        let mcp_request = registry_request(schema, &mcp_root);
        prepare_registry_bundle(&cli_request);
        prepare_registry_bundle(&mcp_request);

        assert_cli_registry_result(
            schema,
            dispatch_registered_command(&threeterm_host::Host::new(), schema.id, cli_request),
        );

        let mcp = McpServer::new().handle_request(&JsonRpcRequest {
            id: json!(schema.name),
            is_notification: false,
            method: "tools/call".to_string(),
            params: json!({
                "name": schema.schema_version,
                "arguments": mcp_request,
            }),
        });
        assert_mcp_registry_result(schema, &mcp);
    }

    let _ = fs::remove_dir_all(&root);
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

fn historical_edit_request(root: &std::path::Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "l-bracket-base",
        "parameter": "length",
        "value": 0.0
    })
}

fn export_request(root: &std::path::Path, override_warnings: bool, accept_stale: bool) -> Value {
    export_request_for_feature(root, "l-bracket", override_warnings, accept_stale)
}

fn export_request_for_feature(
    root: &std::path::Path,
    feature_id: &str,
    override_warnings: bool,
    accept_stale: bool,
) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": feature_id,
        "formats": ["stl"],
        "output_dir": root.join("export").to_string_lossy(),
        "tessellation_deflection": 0.1,
        "override_warnings": override_warnings,
        "accept_stale_geometry": accept_stale
    })
}

fn extrude_request(root: &std::path::Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "extrude",
        "profile": [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
        "height": 2.0,
        "mode": "additive"
    })
}

fn subtractive_extrude_request(root: &std::path::Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "cut",
        "profile": [[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]],
        "height": 2.0,
        "mode": "subtractive",
        "target_feature_id": "base"
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

fn lost_edge_reference(revision: &str) -> Value {
    let mut reference = edge_reference(revision);
    reference["semantic_id"] = json!("missing-edge");
    reference["provenance"]["source_edge_id"] = json!("missing-edge");
    reference
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

fn setup_attached_sketch_root(root: &std::path::Path) -> Option<Value> {
    write_fresh(
        root,
        threeterm_domain::ProjectGeneration::with_id("attached-sketch-parity"),
    )
    .expect("sketch parity bundle creates");
    let source = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/research/rehearsal-evidence/l-bracket/run-2/project/brep/l-bracket.brep"
    ))
    .expect("fixture BREP reads");
    let bundle = Bundle::at(root);
    let revision = bundle
        .open()
        .expect("sketch parity bundle opens")
        .revision_hash_hex()
        .to_string();
    bundle
        .append_feature_with_brep_if_revision("solid", "brep:solid", &revision, &source)
        .expect("sketch parity BREP appends");
    let candidates = threeterm_host::Host::new()
        .planar_face_candidates(root, "solid")
        .expect("OCCT derives sketch parity faces");
    let candidate = candidates
        .into_iter()
        .find(|candidate| candidate.evidence.normal[2].abs() < 0.5)?;
    let placement = json!({
        "origin": candidate.evidence.origin,
        "normal": candidate.evidence.normal,
        "x_axis": candidate.evidence.x_axis,
        "y_axis": candidate.evidence.y_axis
    });
    Some(json!({
        "bundle_path": root.to_string_lossy(),
        "request_id": "attached-sketch-parity-request",
        "feature_id": "attached-sketch",
        "phase": "commit",
        "entities": [
            {"kind": "point", "id": "p0", "x": 1.0, "y": 1.0},
            {"kind": "point", "id": "p1", "x": 0.0, "y": 1.0},
            {"kind": "point", "id": "p2", "x": 2.0, "y": 1.0},
            {"kind": "line_segment", "id": "line", "start": "p1", "end": "p2"},
            {"kind": "circle", "id": "circle", "center": "p0", "radius": 1.0}
        ],
        "constraints": [
            {"id": "fixed-p0", "kind": "fixed", "entities": ["p0"]},
            {"id": "fixed-p1", "kind": "fixed", "entities": ["p1"]},
            {"id": "fixed-p2", "kind": "fixed", "entities": ["p2"]}
        ],
        "support": {
            "semantic_id": candidate.semantic_id,
            "provenance": candidate.provenance,
            "role": candidate.role,
            "evidence": candidate.evidence
        },
        "placement": placement
    }))
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

fn restore_request(root: &Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": "l-bracket-base",
        "name": "recovered-before-historical-edit-2"
    })
}

fn mcp_restore(root: &Path) -> Value {
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.restore-revision/1",
            "arguments": restore_request(root)
        }),
    });
    assert!(
        response.error.is_none(),
        "MCP restore failed: {:?}",
        response.error
    );
    response.result.expect("MCP restore has result")["structuredContent"].clone()
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

fn mcp_cursor_move(root: &std::path::Path, tool: &str) -> Value {
    let server = McpServer::new();
    let response = server.handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": tool,
            "arguments": json!({ "bundle_path": root.to_string_lossy() })
        }),
    });
    assert!(
        response.error.is_none(),
        "MCP {tool} failed: {:?}",
        response.error
    );
    response.result.expect("MCP has result")["structuredContent"].clone()
}

#[test]
fn cli_mcp_and_tui_load_rebuild_disposable_geometry_without_canonical_mutation() {
    let cli_root = root("reload-cli");
    let mcp_root = root("reload-mcp");
    let tui_root = root("reload-tui");
    let Some(worker) = required_worker(
        "cli_mcp_and_tui_load_rebuild_disposable_geometry_without_canonical_mutation",
    ) else {
        return;
    };
    for (path, label) in [(&cli_root, "cli"), (&mcp_root, "mcp"), (&tui_root, "tui")] {
        Bundle::create(path).expect("reload bundle creates");
        threeterm_host::Host::new()
            .extrude(
                path,
                ExtrudeRequest::new(
                    format!("reload-{label}"),
                    vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
                    3.0,
                )
                .with_feature_id("base"),
                &worker,
            )
            .expect("reload seed commits");
    }
    let before = [&cli_root, &mcp_root, &tui_root].map(|path| {
        let bundle = Bundle::at(path).open().expect("reload bundle opens");
        (
            threeterm_host::Host::new()
                .identity(path)
                .expect("reload identity reads"),
            bundle.history,
            bundle.graph,
            fs::read(path.join("manifest.json")).expect("reload manifest reads"),
            fs::read(path.join("transactions.log")).expect("reload log reads"),
            fs::read(path.join("brep/base.brep")).expect("reload BREP reads"),
            file_inventory(path),
        )
    });

    for path in [&cli_root, &mcp_root, &tui_root] {
        fs::remove_file(path.join("brep/base.brep")).expect("reload BREP removes");
        for directory in ["cache", ".derived", "stage"] {
            let _ = fs::remove_dir_all(path.join(directory));
        }
    }
    let loads = [
        cli_load(&cli_root),
        mcp_load(&mcp_root),
        threeterm_tui::execute_domain_command(
            &threeterm_host::Host::new(),
            LOAD_COMMAND_ID,
            json!({"bundle_path": tui_root.to_string_lossy()}),
        )
        .expect("TUI load rebuilds disposable geometry"),
    ];
    for ((path, state), load_result) in [&cli_root, &mcp_root, &tui_root]
        .into_iter()
        .zip(before.iter())
        .zip(loads)
    {
        let (identity, history, graph, manifest, log, brep, inventory) = state;
        assert_eq!(
            load_result["feature_graph_hash"],
            identity.feature_graph_hash
        );
        assert_eq!(load_result["revision_hash"], identity.revision_hash);
        assert_eq!(
            threeterm_host::Host::new().identity(path).unwrap(),
            *identity
        );
        let reloaded = Bundle::at(path).open().expect("reloaded bundle opens");
        assert_eq!(reloaded.history, *history);
        assert_eq!(reloaded.graph, *graph);
        assert_eq!(fs::read(path.join("manifest.json")).unwrap(), *manifest);
        assert_eq!(fs::read(path.join("transactions.log")).unwrap(), *log);
        assert_eq!(fs::read(path.join("brep/base.brep")).unwrap(), *brep);
        assert_eq!(file_inventory(path), *inventory);
        assert!(!reloaded.graph.contains_feature("invalid-cut"));
    }

    for path in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(path);
    }
}

#[test]
fn mcp_undo_and_redo_move_the_active_snapshot_like_the_cli() {
    let path = root("cursor");
    let host = threeterm_host::Host::new();
    host.save_bracket(&path, "l-bracket", 60.0, 30.0, 40.0, 3.0)
        .expect("history initializes");
    host.historical_edit(&path, "l-bracket-base", "length", 61.0)
        .expect("historical edit commits");

    let undone = mcp_cursor_move(&path, "threeterm.command.undo/1");
    assert_eq!(undone["active_revision"], "history-revision-1");
    let redone = mcp_cursor_move(&path, "threeterm.command.redo/1");
    assert_eq!(redone["active_revision"], "history-revision-2");
    assert_eq!(
        redone["active_revision"],
        host.history(&path)
            .expect("history reloads")
            .active_snapshot()
            .revision_id
    );

    let tui_host = threeterm_host::Host::new();
    let tui_undone = threeterm_tui::execute_domain_command(
        &tui_host,
        threeterm_protocol::schema::UNDO_COMMAND_ID,
        json!({ "bundle_path": path.to_string_lossy() }),
    )
    .expect("TUI undo moves through the shared host boundary");
    assert_eq!(tui_undone["active_revision"], "history-revision-1");
    assert_eq!(tui_undone["operation"], "undo");
    let tui_redone = threeterm_tui::execute_domain_command(
        &tui_host,
        threeterm_protocol::schema::REDO_COMMAND_ID,
        json!({ "bundle_path": path.to_string_lossy() }),
    )
    .expect("TUI redo moves through the shared host boundary");
    assert_eq!(tui_redone["active_revision"], "history-revision-2");

    let _ = fs::remove_dir_all(path);
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
fn cli_mcp_and_tui_commit_the_same_attached_sketch_result() {
    let cli_root = root("attached-sketch-cli");
    let mcp_root = root("attached-sketch-mcp");
    let tui_root = root("attached-sketch-tui");
    let Some(_) = required_worker("cli_mcp_and_tui_commit_the_same_attached_sketch_result") else {
        return;
    };
    match SlvsWorker::locate() {
        Ok(_) => {}
        Err(error) if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() => {
            panic!("libslvs worker is required: {error}");
        }
        Err(_) => {
            eprintln!("attached sketch parity: libslvs worker unavailable; skipping");
            return;
        }
    }
    let Some(cli_request) = setup_attached_sketch_root(&cli_root) else {
        panic!("L-bracket fixture has no non-XY planar face");
    };
    let Some(mcp_request) = setup_attached_sketch_root(&mcp_root) else {
        panic!("L-bracket fixture has no non-XY planar face");
    };
    let Some(mut tui_request) = setup_attached_sketch_root(&tui_root) else {
        panic!("L-bracket fixture has no non-XY planar face");
    };

    let cli = threeterm_cli::dispatch::dispatch_registered_command(
        &threeterm_host::Host::new(),
        SKETCH_SOLVE_COMMAND_ID,
        cli_request,
    )
    .expect("CLI attached sketch commits");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.sketch-solve/1",
            "arguments": mcp_request
        }),
    });
    assert!(
        mcp.error.is_none(),
        "MCP attached sketch failed: {:?}",
        mcp.error
    );
    let mcp = mcp.result.expect("MCP attached sketch has result")["structuredContent"].clone();

    let tui_host = threeterm_host::Host::new();
    let preview = tui_host
        .preview_domain_command(SKETCH_SOLVE_COMMAND_ID, tui_request.clone())
        .expect("TUI attached sketch preview succeeds");
    tui_request["preview_revision"] = Value::String(preview.preview_revision);
    let tui =
        threeterm_tui::execute_domain_command(&tui_host, SKETCH_SOLVE_COMMAND_ID, tui_request)
            .expect("TUI attached sketch commits");

    for result in [&cli, &mcp, &tui] {
        assert_eq!(result["status"], "solved");
        assert_eq!(result["dof"], 0);
        assert_eq!(result["reattachment_outcome"], "resolved");
        assert_eq!(
            result["entity_ids"],
            json!(["p0", "p1", "p2", "line", "circle"])
        );
        assert_eq!(
            result["solved_coordinates"].as_array().map(Vec::len),
            Some(3)
        );
        assert_eq!(result["support"]["role"], "sketch-support");
    }
    assert_eq!(cli, mcp, "CLI and MCP attached sketch results differ");
    assert_eq!(cli, tui, "CLI and TUI attached sketch results differ");

    for path in [&cli_root, &mcp_root, &tui_root] {
        let loaded = Bundle::at(path)
            .open()
            .expect("attached sketch bundle reloads");
        let sketch = loaded
            .graph
            .sketch("attached-sketch")
            .expect("attached sketch intent persists");
        assert_eq!(
            sketch.support.as_ref().map(|support| support.role.as_str()),
            Some("sketch-support")
        );
        assert!(sketch.placement.is_some());
        let scene = threeterm_viewport::ViewportScene::from_feature_graph(
            loaded.revision_hash_hex(),
            &loaded.graph,
            None,
        );
        assert!(
            scene
                .features
                .iter()
                .any(|feature| feature.kind.starts_with("sketch-segment3:"))
        );
        assert!(
            scene
                .features
                .iter()
                .any(|feature| feature.kind.starts_with("sketch-circle3:"))
        );
    }

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
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
fn cli_mcp_and_tui_preserve_historical_failure_recovery_context() {
    let Some(_worker) =
        required_worker("cli_mcp_and_tui_preserve_historical_failure_recovery_context")
    else {
        return;
    };
    let cli_root = root("historical-failure-cli");
    let mcp_root = root("historical-failure-mcp");
    let tui_root = root("historical-failure-tui");
    for path in [&cli_root, &mcp_root, &tui_root] {
        threeterm_host::Host::new()
            .save_bracket(path, "l-bracket", 60.0, 30.0, 40.0, 3.0)
            .expect("history fixture creates");
        fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/research/rehearsal-evidence/l-bracket/run-2/project/brep/l-bracket.brep"
            ),
            path.join("brep/l-bracket.brep"),
        )
        .expect("history fixture BREP copies");
    }

    let cli = dispatch_registered_command(
        &threeterm_host::Host::new(),
        HISTORICAL_EDIT_COMMAND_ID,
        historical_edit_request(&cli_root),
    )
    .expect("CLI historical edit commits degraded snapshot");
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        HISTORICAL_EDIT_COMMAND_ID,
        historical_edit_request(&tui_root),
    )
    .expect("TUI historical edit commits degraded snapshot");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.historical-edit/1",
            "arguments": historical_edit_request(&mcp_root)
        }),
    });
    let mcp =
        mcp.result.expect("MCP historical edit returns a result")["structuredContent"].clone();

    assert_eq!(cli, mcp);
    assert_eq!(cli, tui);
    assert_eq!(cli["status"], "degraded");
    assert_eq!(
        cli["dirty_features"],
        json!(["l-bracket-base", "l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(
        cli["blocked_features"],
        json!(["l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(
        cli["diagnostics"][0]["affected_ids"],
        json!(["l-bracket-base", "l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(
        cli["diagnostics"][0]["recovery"],
        "correct_geometry_or_restore_revision"
    );
    assert!(
        cli["named_revisions"]
            .as_array()
            .expect("recovery revision list")
            .iter()
            .any(|revision| revision["name"] == "recovered-before-historical-edit-2")
    );

    let history = threeterm_host::Host::new()
        .history(&tui_root)
        .expect("degraded history reloads");
    let active_revision = cli["active_revision"]
        .as_str()
        .expect("active revision is a string");
    let mut session = threeterm_tui::TuiSession::new([], active_revision);
    session.refresh_stale_last_valid_geometry(&history, "l-bracket");
    let overlay = session
        .stale_last_valid_geometry_overlay()
        .expect("TUI exposes stale recovery geometry");
    assert!(overlay.contains("l-bracket-base"));
    assert!(overlay.contains(active_revision));

    let cli_restore = dispatch_registered_command(
        &threeterm_host::Host::new(),
        RESTORE_REVISION_COMMAND_ID,
        restore_request(&cli_root),
    )
    .expect("CLI restores the pre-failure named revision");
    let tui_restore = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        RESTORE_REVISION_COMMAND_ID,
        restore_request(&tui_root),
    )
    .expect("TUI restores the pre-failure named revision");
    let mcp_restore = mcp_restore(&mcp_root);
    assert_eq!(cli_restore, tui_restore);
    assert_eq!(cli_restore, mcp_restore);
    assert_eq!(cli_restore["status"], "ok");
    assert_eq!(
        cli_restore["features"][0]["stale_last_valid_geometry"],
        false
    );

    let cli_export = dispatch_registered_command(
        &threeterm_host::Host::new(),
        EXPORT_COMMAND_ID,
        export_request_for_feature(&cli_root, "l-bracket-base", false, false),
    )
    .expect("CLI exports restored current geometry");
    let tui_export = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        EXPORT_COMMAND_ID,
        export_request_for_feature(&tui_root, "l-bracket-base", false, false),
    )
    .expect("TUI exports restored current geometry");
    let mcp_export_response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.export/1",
            "arguments": export_request_for_feature(&mcp_root, "l-bracket-base", false, false)
        }),
    });
    let mcp_export = mcp_export_response
        .result
        .expect("MCP exports restored current geometry")["structuredContent"]
        .clone();
    for export in [&cli_export, &tui_export, &mcp_export] {
        assert_eq!(export["status"], "ok");
        assert_eq!(export["accepted_stale_last_valid_geometry"], false);
    }

    for path in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(path);
    }
}

#[test]
fn stale_geometry_export_is_fatal_and_equivalent_for_every_override_combination() {
    let cli_root = root("stale-export-cli");
    let mcp_root = root("stale-export-mcp");
    let tui_root = root("stale-export-tui");
    for path in [&cli_root, &mcp_root, &tui_root] {
        let host = threeterm_host::Host::new();
        host.save_bracket(path, "l-bracket", 60.0, 30.0, 40.0, 3.0)
            .expect("history fixture creates");
        host.historical_edit(path, "l-bracket-base", "length", 0.0)
            .expect("historical failure commits stale state");
    }

    let manifest_before = [
        fs::read(cli_root.join("manifest.json")).expect("CLI manifest reads"),
        fs::read(mcp_root.join("manifest.json")).expect("MCP manifest reads"),
        fs::read(tui_root.join("manifest.json")).expect("TUI manifest reads"),
    ];
    let log_before = [
        fs::read(cli_root.join("transactions.log")).expect("CLI log reads"),
        fs::read(mcp_root.join("transactions.log")).expect("MCP log reads"),
        fs::read(tui_root.join("transactions.log")).expect("TUI log reads"),
    ];

    for override_warnings in [false, true] {
        for accept_stale in [false, true] {
            let cli_error = dispatch_registered_command(
                &threeterm_host::Host::new(),
                EXPORT_COMMAND_ID,
                export_request(&cli_root, override_warnings, accept_stale),
            )
            .expect_err("CLI refuses stale geometry export");
            let threeterm_cli::dispatch::DispatchError::Host(cli_error) = cli_error else {
                panic!("CLI returned a non-host export failure");
            };
            let cli = threeterm_host::domain_command_failure_value(&cli_error);

            let tui_error = threeterm_tui::execute_domain_command(
                &threeterm_host::Host::new(),
                EXPORT_COMMAND_ID,
                export_request(&tui_root, override_warnings, accept_stale),
            )
            .expect_err("TUI refuses stale geometry export");
            let threeterm_protocol::command_execution::ExecutionError::Handler(tui_error) =
                tui_error
            else {
                panic!("TUI returned a non-handler export failure");
            };
            let tui = threeterm_tui::domain_command_failure_value(&tui_error);

            let mcp = McpServer::new().handle_request(&JsonRpcRequest {
                id: json!(1),
                is_notification: false,
                method: "tools/call".to_string(),
                params: json!({
                    "name": "threeterm.command.export/1",
                    "arguments": export_request(&mcp_root, override_warnings, accept_stale)
                }),
            });
            let mcp =
                mcp.result.expect("MCP returns stale export result")["structuredContent"].clone();

            assert_eq!(cli, tui);
            assert_eq!(cli, mcp);
            assert_eq!(cli["severity"], "error");
            assert_eq!(cli["code"], "stale_last_valid_geometry");
            assert_eq!(cli["override_eligible"], false);
        }
    }

    for (index, path) in [&cli_root, &mcp_root, &tui_root].into_iter().enumerate() {
        assert_eq!(
            fs::read(path.join("manifest.json")).unwrap(),
            manifest_before[index]
        );
        assert_eq!(
            fs::read(path.join("transactions.log")).unwrap(),
            log_before[index]
        );
        assert!(!path.join("export").exists());
    }
    for path in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(path);
    }
}

fn cli_extrude(root: &Path, profile_file: &Path) -> Value {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let root = root.to_string_lossy().into_owned();
    let profile_file = profile_file.to_string_lossy().into_owned();
    let status = threeterm_cli::dispatch::dispatch(
        [
            "--machine",
            "extrude",
            "--bundle",
            root.as_str(),
            "--feature-id",
            "extrude",
            "--profile-file",
            profile_file.as_str(),
            "--height",
            "2.0",
            "--mode",
            "additive",
        ]
        .into_iter()
        .map(OsString::from),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(
        status,
        0,
        "CLI extrude failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    serde_json::from_slice(&stdout).expect("CLI extrude returns JSON")
}

fn normalized_extrude_response(response: &Value) -> Value {
    let mut normalized = response.clone();
    normalized["brep_path"] = json!("<derived-brep>");
    normalized
}

fn canonical_extrude_intent(root: &Path) -> Value {
    let bundle = Bundle::at(root).open().expect("parity bundle opens");
    serde_json::to_value(
        bundle
            .log
            .entries()
            .last()
            .expect("extrude transaction exists")
            .intent
            .as_ref()
            .expect("extrude intent persists"),
    )
    .expect("canonical intent serializes")
}

#[test]
fn adapter_command_parity() {
    let cli_root = root("extrude-cli");
    let mcp_root = root("extrude-mcp");
    let tui_root = root("extrude-tui");
    let lua_root = root("extrude-lua");
    let profile_file = root("extrude-profile").with_extension("json");
    OcctWorker::locate().expect("adapter_command_parity requires the real OCCT worker");
    for path in [&cli_root, &mcp_root, &tui_root, &lua_root] {
        Bundle::create_for_test(path, "11".repeat(16).as_str()).expect("bundle creates");
    }

    fs::write(&profile_file, "[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]]").expect("CLI profile writes");
    let cli = cli_extrude(&cli_root, &profile_file);
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        EXTRUDE_COMMAND_ID,
        extrude_request(&tui_root),
    )
    .expect("TUI extrude executes");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.extrude/2",
            "arguments": extrude_request(&mcp_root)
        }),
    });
    assert!(mcp.error.is_none(), "MCP extrude returns no JSON-RPC error");
    let mcp = mcp.result.expect("MCP extrude executes")["structuredContent"].clone();
    let lua_source = format!(
        r#"keymap.bind("F2", "extrude", {{
            bundle_path = "{}",
            feature_id = "extrude",
            profile = {{ {{0.0, 0.0}}, {{4.0, 0.0}}, {{0.0, 4.0}} }},
            height = 2.0,
            mode = "additive"
        }})"#,
        lua_root.display()
    );
    let lua =
        threeterm_cli::dispatch::dispatch_lua_key(&lua_source, "F2", &threeterm_host::Host::new())
            .expect("Lua extrude executes");

    for result in [&cli, &tui, &mcp, &lua] {
        assert_eq!(result["status"], "ok");
        assert_eq!(result["operation"], "extrude");
        assert_eq!(result["feature_id"], "extrude");
        assert_eq!(result["mode"], "additive");
        assert_eq!(result["worker_fingerprint"]["worker_kind"], "occt");
    }
    let normalized = [
        normalized_extrude_response(&cli),
        normalized_extrude_response(&tui),
        normalized_extrude_response(&mcp),
        normalized_extrude_response(&lua),
    ];
    for response in &normalized[1..] {
        assert_eq!(response, &normalized[0]);
    }
    let roots = [&cli_root, &mcp_root, &tui_root, &lua_root];
    let identities = roots.map(|path| {
        threeterm_host::Host::new()
            .identity(path)
            .expect("adapter identity reads")
    });
    for identity in &identities[1..] {
        assert_eq!(identity, &identities[0]);
    }
    let intents = roots.map(|path| canonical_extrude_intent(path));
    for intent in &intents[1..] {
        assert_eq!(intent, &intents[0]);
    }
    let breps = roots.map(|path| fs::read(path.join("brep/extrude.brep")).expect("BREP reads"));
    for brep in &breps[1..] {
        assert_eq!(brep, &breps[0]);
    }

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
    let _ = fs::remove_dir_all(lua_root);
    let _ = fs::remove_file(profile_file);
}

#[test]
fn extrude_adapter_parity_commits_equivalent_subtractive_extrusions() {
    let cli_root = root("subtractive-cli");
    let mcp_root = root("subtractive-mcp");
    let tui_root = root("subtractive-tui");
    let Some(worker) = required_worker("cli_mcp_and_tui_commit_equivalent_subtractive_extrusions")
    else {
        let _ = fs::remove_dir_all(&cli_root);
        let _ = fs::remove_dir_all(&mcp_root);
        let _ = fs::remove_dir_all(&tui_root);
        return;
    };
    for path in [&cli_root, &mcp_root, &tui_root] {
        Bundle::create(path).expect("bundle creates");
        threeterm_host::Host::new()
            .extrude(
                path,
                ExtrudeRequest::new(
                    "base",
                    vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)],
                    2.0,
                )
                .with_feature_id("base"),
                &worker,
            )
            .expect("base solid commits");
    }

    let cli = threeterm_cli::dispatch::dispatch_registered_command(
        &threeterm_host::Host::new(),
        EXTRUDE_COMMAND_ID,
        subtractive_extrude_request(&cli_root),
    )
    .expect("CLI subtractive extrude executes");
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        EXTRUDE_COMMAND_ID,
        subtractive_extrude_request(&tui_root),
    )
    .expect("TUI subtractive extrude executes");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.extrude/2",
            "arguments": subtractive_extrude_request(&mcp_root)
        }),
    });
    let mcp = mcp.result.expect("MCP subtractive extrude executes")["structuredContent"].clone();

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["status"], "ok");
        assert_eq!(result["mode"], "subtractive");
        assert_eq!(result["target_feature_id"], "base");
        assert_eq!(result["feature_id"], "cut");
    }
    assert_eq!(cli["brep_sha256"], tui["brep_sha256"]);
    assert_eq!(cli["brep_sha256"], mcp["brep_sha256"]);
    for field in [
        "request_id",
        "feature_graph_hash",
        "revision_hash",
        "transaction_count",
        "terminal_log_digest",
    ] {
        assert_eq!(cli[field], tui[field], "TUI {field} matches CLI");
        assert_eq!(cli[field], mcp[field], "MCP {field} matches CLI");
    }

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

fn required_worker(test_name: &str) -> Option<OcctWorker> {
    match OcctWorker::locate() {
        Ok(worker) => Some(worker),
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some()
                || std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() =>
        {
            panic!("{test_name}: OCCT worker is required: {error}")
        }
        Err(_) => {
            eprintln!("{test_name}: OCCT worker unavailable; skipping");
            None
        }
    }
}

#[test]
fn extrude_adapter_failure_parity_reports_the_same_invalid_subtractive_target_diagnostic() {
    let cli_root = root("invalid-subtractive-cli");
    let mcp_root = root("invalid-subtractive-mcp");
    let tui_root = root("invalid-subtractive-tui");
    for path in [&cli_root, &mcp_root, &tui_root] {
        Bundle::create(path).expect("bundle creates");
    }
    let before = [&cli_root, &mcp_root, &tui_root].map(|path| {
        (
            fs::read(path.join("manifest.json")).expect("manifest reads"),
            fs::read(path.join("transactions.log")).expect("transaction log reads"),
            threeterm_host::Host::new()
                .identity(path)
                .expect("Revision Snapshot identity reads"),
        )
    });
    let cli = threeterm_cli::dispatch::dispatch_registered_command(
        &threeterm_host::Host::new(),
        EXTRUDE_COMMAND_ID,
        subtractive_extrude_request(&cli_root),
    )
    .expect_err("CLI must reject a missing subtractive target");
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        EXTRUDE_COMMAND_ID,
        subtractive_extrude_request(&tui_root),
    )
    .expect_err("TUI must reject a missing subtractive target");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.extrude/2",
            "arguments": subtractive_extrude_request(&mcp_root)
        }),
    });
    let mcp = mcp.result.expect("MCP returns a tool error result");
    let DispatchError::Host(cli_error) = cli else {
        panic!("CLI returned a non-host failure");
    };
    let threeterm_protocol::command_execution::ExecutionError::Handler(tui_error) = tui else {
        panic!("TUI returned a non-handler failure");
    };
    let cli_diagnostic =
        serde_json::to_value(threeterm_host::domain_command_diagnostic(&cli_error))
            .expect("CLI diagnostic serializes");
    let tui_diagnostic =
        serde_json::to_value(threeterm_host::domain_command_diagnostic(&tui_error))
            .expect("TUI diagnostic serializes");
    let mcp_diagnostic = mcp["structuredContent"].clone();
    assert_eq!(cli_diagnostic, tui_diagnostic);
    assert_eq!(cli_diagnostic, mcp_diagnostic);
    assert_eq!(cli_diagnostic["code"], "invalid_request");
    assert_eq!(cli_diagnostic["affected_ids"], json!(["cut", "base"]));
    assert_eq!(
        cli_diagnostic["recovery"],
        "choose_existing_target_or_restore_revision"
    );
    assert!(
        cli_diagnostic["arg"]
            .as_str()
            .expect("diagnostic detail is text")
            .contains("subtractive extrude target feature is missing: base")
    );
    assert_eq!(mcp["isError"], true);
    for (index, path) in [cli_root, mcp_root, tui_root].into_iter().enumerate() {
        assert_eq!(
            fs::read(path.join("manifest.json")).unwrap(),
            before[index].0
        );
        assert_eq!(
            fs::read(path.join("transactions.log")).unwrap(),
            before[index].1
        );
        assert_eq!(Bundle::at(&path).open().unwrap().log.len(), 0);
        assert_eq!(
            threeterm_host::Host::new()
                .identity(&path)
                .expect("Revision Snapshot identity remains unchanged"),
            before[index].2
        );
        let _ = fs::remove_dir_all(path);
    }
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
        assert_eq!(result["affected_ids"][1], "base");
        assert_eq!(
            result["recovery"],
            "choose_compatible_edge_or_restore_revision"
        );
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
fn cli_mcp_and_tui_report_a_lost_edge_reference_without_commit() {
    let cli_root = root("edge-lost-cli");
    let mcp_root = root("edge-lost-mcp");
    let tui_root = root("edge-lost-tui");
    let Some(cli_revision) = setup_edge_root(&cli_root, "lost-cli") else {
        return;
    };
    let Some(tui_revision) = setup_edge_root(&tui_root, "lost-tui") else {
        return;
    };
    let Some(mcp_revision) = setup_edge_root(&mcp_root, "lost-mcp") else {
        return;
    };

    let cli = cli_reattach_edge(
        &cli_root,
        &cli_revision,
        lost_edge_reference(&cli_revision),
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
        lost_edge_reference(&tui_revision),
        edge_edit_target(&tui_revision),
    )
    .expect("TUI edge command reports a lost reference");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.reattach-edge/2",
            "arguments": edge_request_with_target(
                &mcp_root,
                &mcp_revision,
                lost_edge_reference(&mcp_revision),
                edge_edit_target(&mcp_revision),
            )
        }),
    });
    let mcp = mcp
        .result
        .expect("MCP edge command reports a lost reference")["structuredContent"]
        .clone();

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["outcome"], "lost");
        assert!(result["candidate_edge_ids"].as_array().unwrap().is_empty());
        assert_eq!(result["affected_ids"][1], "base");
        assert_eq!(result["recovery"], "reattach_edge_or_restore_revision");
        assert_eq!(result["committed"], false);
    }
    for path in [&cli_root, &mcp_root, &tui_root] {
        assert_eq!(Bundle::at(path).open().unwrap().log.len(), 1);
        assert!(!path.join("brep/fillet-after-edge.brep").exists());
        let _ = fs::remove_dir_all(path);
    }
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
        assert_eq!(result["affected_ids"][1], "base");
        assert_eq!(
            result["recovery"],
            "choose_candidate_edge_or_restore_revision"
        );
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

fn boolean_request(root: &std::path::Path, feature_id: &str) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": feature_id,
        "base_feature_id": "bool-base",
        "tool_feature_id": "bool-tool"
    })
}

fn setup_boolean_operands(path: &std::path::Path, worker: &OcctWorker) {
    Bundle::create(path).expect("bundle creates");
    let host = threeterm_host::Host::new();
    for (feature_id, x0) in [("bool-base", 0.0), ("bool-tool", 5.0)] {
        host.extrude(
            path,
            ExtrudeRequest::new(
                format!("bool-seed-{feature_id}"),
                vec![(x0, 0.0), (x0 + 10.0, 0.0), (x0 + 10.0, 5.0), (x0, 5.0)],
                3.0,
            )
            .with_feature_id(feature_id),
            worker,
        )
        .expect("boolean operand commits");
    }
}

fn mcp_boolean(
    command: threeterm_protocol::schema::CommandId,
    tool: &str,
    root: &std::path::Path,
    feature_id: &str,
) -> Value {
    let entry = threeterm_protocol::schema::find(command).expect("boolean command is registered");
    assert_eq!(entry.schema_version, tool);
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": tool,
            "arguments": boolean_request(root, feature_id)
        }),
    });
    response.result.expect("MCP boolean executes")["structuredContent"].clone()
}

#[test]
fn cli_mcp_and_tui_commit_equivalent_boolean_cut_solids() {
    let cli_root = root("booleancut-cli");
    let mcp_root = root("booleancut-mcp");
    let tui_root = root("booleancut-tui");
    let Some(worker) = required_worker("cli_mcp_and_tui_commit_equivalent_boolean_cut_solids")
    else {
        for path in [&cli_root, &mcp_root, &tui_root] {
            let _ = fs::remove_dir_all(path);
        }
        return;
    };
    for path in [&cli_root, &mcp_root, &tui_root] {
        setup_boolean_operands(path, &worker);
    }

    let cli = threeterm_cli::dispatch::dispatch_registered_command(
        &threeterm_host::Host::new(),
        BOOLEAN_CUT_COMMAND_ID,
        boolean_request(&cli_root, "bool-cut"),
    )
    .expect("CLI boolean-cut executes");
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        BOOLEAN_CUT_COMMAND_ID,
        boolean_request(&tui_root, "bool-cut"),
    )
    .expect("TUI boolean-cut executes");
    let mcp = mcp_boolean(
        BOOLEAN_CUT_COMMAND_ID,
        "threeterm.command.boolean-cut/1",
        &mcp_root,
        "bool-cut",
    );

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["status"], "ok");
        assert_eq!(result["operation"], "boolean_cut");
        assert_eq!(result["feature_id"], "bool-cut");
        assert_eq!(
            result["schema_version"],
            "threeterm.command.boolean-cut.response/1"
        );
    }
    assert_eq!(cli["brep_sha256"], tui["brep_sha256"]);
    assert_eq!(cli["brep_sha256"], mcp["brep_sha256"]);

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

#[test]
fn cli_mcp_and_tui_commit_equivalent_boolean_common_solids() {
    let cli_root = root("booleancommon-cli");
    let mcp_root = root("booleancommon-mcp");
    let tui_root = root("booleancommon-tui");
    let Some(worker) = required_worker("cli_mcp_and_tui_commit_equivalent_boolean_common_solids")
    else {
        for path in [&cli_root, &mcp_root, &tui_root] {
            let _ = fs::remove_dir_all(path);
        }
        return;
    };
    for path in [&cli_root, &mcp_root, &tui_root] {
        setup_boolean_operands(path, &worker);
    }

    let cli = threeterm_cli::dispatch::dispatch_registered_command(
        &threeterm_host::Host::new(),
        BOOLEAN_COMMON_COMMAND_ID,
        boolean_request(&cli_root, "bool-common"),
    )
    .expect("CLI boolean-common executes");
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        BOOLEAN_COMMON_COMMAND_ID,
        boolean_request(&tui_root, "bool-common"),
    )
    .expect("TUI boolean-common executes");
    let mcp = mcp_boolean(
        BOOLEAN_COMMON_COMMAND_ID,
        "threeterm.command.boolean-common/1",
        &mcp_root,
        "bool-common",
    );

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["status"], "ok");
        assert_eq!(result["operation"], "boolean_common");
        assert_eq!(result["feature_id"], "bool-common");
        assert_eq!(
            result["schema_version"],
            "threeterm.command.boolean-common.response/1"
        );
    }
    assert_eq!(cli["brep_sha256"], tui["brep_sha256"]);
    assert_eq!(cli["brep_sha256"], mcp["brep_sha256"]);

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

fn hole_request(root: &std::path::Path, feature_id: &str) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "feature_id": feature_id,
        "base_feature_id": "hole-base",
        "position": [1.5, 1.5, 0.0],
        "direction": [0.0, 0.0, 1.0],
        "diameter": 1.0,
        "hole_kind": "drilled"
    })
}

fn tapped_hole_request(root: &std::path::Path, feature_id: &str) -> Value {
    let mut request = hole_request(root, feature_id);
    request["hole_kind"] = json!("tapped");
    request["thread_designation"] = json!("M6x1");
    request["thread_pitch"] = json!(1.0);
    request["thread_depth"] = json!(2.0);
    request
}

fn setup_hole_base(path: &std::path::Path, worker: &OcctWorker) {
    Bundle::create(path).expect("bundle creates");
    threeterm_host::Host::new()
        .extrude(
            path,
            ExtrudeRequest::new(
                format!("hole-seed-{}", path.to_string_lossy()),
                vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
                3.0,
            )
            .with_feature_id("hole-base"),
            worker,
        )
        .expect("hole base commits");
}

fn cli_hole(root: &std::path::Path, feature_id: &str, tapped: bool) -> Value {
    let path = root.to_string_lossy().into_owned();
    let kind = if tapped { "tapped" } else { "drilled" };
    let mut args = vec![
        "--machine".to_string(),
        "hole".to_string(),
        "--bundle".to_string(),
        path,
        "--feature-id".to_string(),
        feature_id.to_string(),
        "--base".to_string(),
        "hole-base".to_string(),
        "--position".to_string(),
        "1.5,1.5,0.0".to_string(),
        "--direction".to_string(),
        "0.0,0.0,1.0".to_string(),
        "--diameter".to_string(),
        "1.0".to_string(),
        "--hole-kind".to_string(),
        kind.to_string(),
    ];
    if tapped {
        args.extend([
            "--thread-designation".to_string(),
            "M6x1".to_string(),
            "--thread-pitch".to_string(),
            "1.0".to_string(),
            "--thread-depth".to_string(),
            "2.0".to_string(),
        ]);
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = threeterm_cli::dispatch::dispatch(
        args.into_iter().map(OsString::from),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(
        status,
        0,
        "CLI hole failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    serde_json::from_slice(&stdout).expect("CLI hole returns JSON")
}

fn mcp_hole(root: &std::path::Path, feature_id: &str) -> Value {
    let entry = threeterm_protocol::schema::find(HOLE_COMMAND_ID).expect("hole is registered");
    assert_eq!(entry.schema_version, "threeterm.command.hole/1");
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.hole/1",
            "arguments": hole_request(root, feature_id)
        }),
    });
    response.result.expect("MCP hole executes")["structuredContent"].clone()
}

fn mcp_tapped_hole(root: &std::path::Path, feature_id: &str) -> Value {
    let entry = threeterm_protocol::schema::find(HOLE_COMMAND_ID).expect("hole is registered");
    let response = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": entry.schema_version,
            "arguments": tapped_hole_request(root, feature_id)
        }),
    });
    response.result.expect("MCP tapped hole executes")["structuredContent"].clone()
}

#[test]
fn cli_mcp_and_tui_commit_equivalent_drilled_holes() {
    let cli_root = root("hole-cli");
    let mcp_root = root("hole-mcp");
    let tui_root = root("hole-tui");
    let Some(worker) = required_worker("cli_mcp_and_tui_commit_equivalent_drilled_holes") else {
        for path in [&cli_root, &mcp_root, &tui_root] {
            let _ = fs::remove_dir_all(path);
        }
        return;
    };
    for path in [&cli_root, &mcp_root, &tui_root] {
        setup_hole_base(path, &worker);
    }

    let cli = cli_hole(&cli_root, "hole-1", false);
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        HOLE_COMMAND_ID,
        hole_request(&tui_root, "hole-1"),
    )
    .expect("TUI hole executes");
    let mcp = mcp_hole(&mcp_root, "hole-1");

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["status"], "ok");
        assert_eq!(result["operation"], "hole");
        assert_eq!(result["feature_id"], "hole-1");
        assert_eq!(
            result["schema_version"],
            "threeterm.command.hole.response/1"
        );
    }
    assert_eq!(cli["brep_sha256"], tui["brep_sha256"]);
    assert_eq!(cli["brep_sha256"], mcp["brep_sha256"]);

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

#[test]
fn cli_mcp_and_tui_commit_equivalent_tapped_holes() {
    let cli_root = root("tapped-hole-cli");
    let mcp_root = root("tapped-hole-mcp");
    let tui_root = root("tapped-hole-tui");
    let Some(worker) = required_worker("cli_mcp_and_tui_commit_equivalent_tapped_holes") else {
        for path in [&cli_root, &mcp_root, &tui_root] {
            let _ = fs::remove_dir_all(path);
        }
        return;
    };
    for path in [&cli_root, &mcp_root, &tui_root] {
        setup_hole_base(path, &worker);
    }

    let cli = cli_hole(&cli_root, "hole-1", true);
    let tui = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        HOLE_COMMAND_ID,
        tapped_hole_request(&tui_root, "hole-1"),
    )
    .expect("TUI tapped hole executes");
    let mcp = mcp_tapped_hole(&mcp_root, "hole-1");

    for result in [&cli, &tui, &mcp] {
        assert_eq!(result["status"], "ok");
        assert_eq!(result["operation"], "hole");
        assert_eq!(result["feature_id"], "hole-1");
    }
    assert_eq!(cli["brep_sha256"], tui["brep_sha256"]);
    assert_eq!(cli["brep_sha256"], mcp["brep_sha256"]);
    for path in [&cli_root, &mcp_root, &tui_root] {
        let loaded = Bundle::at(path).open().expect("tapped bundle opens");
        let entry = loaded
            .log
            .entries()
            .last()
            .expect("tapped transaction exists");
        let intent = entry.intent.as_ref().expect("tapped intent persists");
        let intent = match intent {
            threeterm_persistence::CanonicalIntent::Hole(intent) => intent,
            other => panic!("unexpected intent: {other:?}"),
        };
        assert_eq!(intent.hole_kind, "tapped");
        assert_eq!(
            intent.deterministic_inputs.thread_designation.as_deref(),
            Some("M6x1")
        );
    }

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(mcp_root);
    let _ = fs::remove_dir_all(tui_root);
}

#[test]
fn cli_mcp_and_tui_report_the_same_invalid_hole_diagnostic() {
    let cli_root = root("invalid-hole-cli");
    let mcp_root = root("invalid-hole-mcp");
    let tui_root = root("invalid-hole-tui");
    for path in [&cli_root, &mcp_root, &tui_root] {
        Bundle::create(path).expect("bundle creates");
    }
    let before = fs::read(cli_root.join("manifest.json")).expect("CLI manifest reads");
    let before_log = fs::read(cli_root.join("transactions.log")).expect("CLI log reads");

    let cli_path = cli_root.to_string_lossy().into_owned();
    let mut cli_stdout = Vec::new();
    let mut cli_stderr = Vec::new();
    let status = threeterm_cli::dispatch::dispatch(
        [
            "--machine",
            "hole",
            "--bundle",
            cli_path.as_str(),
            "--feature-id",
            "hole-1",
            "--base",
            "missing-base",
            "--position",
            "1.5,1.5,0.0",
            "--direction",
            "0.0,0.0,1.0",
            "--diameter",
            "1.0",
        ]
        .into_iter()
        .map(OsString::from),
        &mut cli_stdout,
        &mut cli_stderr,
    );
    assert_ne!(status, 0);
    assert!(cli_stdout.is_empty());
    let cli_diagnostic: Value = serde_json::from_slice(&cli_stderr).expect("CLI diagnostic JSON");
    assert_eq!(cli_diagnostic["code"], "invalid_request");

    let mut tui_request = hole_request(&tui_root, "hole-1");
    tui_request["base_feature_id"] = json!("missing-base");
    let tui_error = threeterm_tui::execute_domain_command(
        &threeterm_host::Host::new(),
        HOLE_COMMAND_ID,
        tui_request,
    )
    .expect_err("TUI must reject a missing hole support");
    let tui_text = format!("{tui_error:?}");
    assert!(tui_text.contains("hole base feature is missing: missing-base"));

    let mut mcp_request = hole_request(&mcp_root, "hole-1");
    mcp_request["base_feature_id"] = json!("missing-base");
    let mcp = McpServer::new().handle_request(&JsonRpcRequest {
        id: json!(1),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": "threeterm.command.hole/1",
            "arguments": mcp_request
        }),
    });
    let mcp_result = mcp.result.expect("MCP returns a structured tool error");
    assert_eq!(mcp_result["isError"], true);
    assert_eq!(mcp_result["structuredContent"]["code"], "invalid_request");
    assert_eq!(
        mcp_result["structuredContent"]["detail"],
        cli_diagnostic["detail"]
    );

    assert_eq!(fs::read(cli_root.join("manifest.json")).unwrap(), before);
    assert_eq!(
        fs::read(cli_root.join("transactions.log")).unwrap(),
        before_log
    );
    for path in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(path);
    }
}

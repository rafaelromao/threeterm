use std::cell::Cell;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::{
    Host, HostError, canonical_bracket_request_id, domain_command_diagnostic,
    domain_execution_diagnostic,
};
use threeterm_occt_worker::{BracketRequest, ExtrudeRequest, OcctWorker, new_request_id};
use threeterm_persistence::Bundle;
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, BOOLEAN_FUSE_COMMAND_ID, BOOLEAN_PATTERN_COMMAND_ID, BRACKET_COMMAND_ID,
    CHAMFER_COMMAND_ID, CIRCULAR_PATTERN_COMMAND_ID, DRAFT_COMMAND_ID, EXTRUDE_COMMAND_ID,
    FILLET_COMMAND_ID, HOLE_COMMAND_ID, IDENTITY_COMMAND_ID, LINEAR_PATTERN_COMMAND_ID,
    LIST_COMMAND_ID, LOAD_COMMAND_ID, LOFT_COMMAND_ID, MIRROR_COMMAND_ID, NEW_PROJECT_COMMAND_ID,
    REHEARSE_COMMAND_ID, REVOLVE_COMMAND_ID, SAVE_COMMAND_ID, SHELL_COMMAND_ID,
};
use threeterm_protocol::schema_validator::validate;

const BASELINE_COMMANDS: [threeterm_protocol::schema::CommandId; 16] = [
    LIST_COMMAND_ID,
    NEW_PROJECT_COMMAND_ID,
    SAVE_COMMAND_ID,
    LOAD_COMMAND_ID,
    EXTRUDE_COMMAND_ID,
    BOOLEAN_FUSE_COMMAND_ID,
    FILLET_COMMAND_ID,
    CHAMFER_COMMAND_ID,
    HOLE_COMMAND_ID,
    REVOLVE_COMMAND_ID,
    MIRROR_COMMAND_ID,
    LINEAR_PATTERN_COMMAND_ID,
    CIRCULAR_PATTERN_COMMAND_ID,
    SHELL_COMMAND_ID,
    DRAFT_COMMAND_ID,
    LOFT_COMMAND_ID,
];

#[derive(Debug)]
struct ResultRow {
    command_id: String,
    request_schema_version: String,
    response_schema_version: String,
    result_kind: String,
    diagnostic_code: Option<String>,
    canonical_revision_before: String,
    canonical_revision_after: String,
    log_length_before: usize,
    log_length_after: usize,
    artifact_path: Option<String>,
}

const BASELINE_SCHEMA_CONTRACTS: [(&str, &str, &str, &str, &str, &str); 16] = [
    (
        "list",
        "threeterm.command.list/1",
        "threeterm.command.list.request/1",
        "threeterm.command.list.response/1",
        "99334726611ccf58a148b0814696bfa6fe08c1b2d027e946beccf5a74331c9aa",
        "0c83504d70205aa702803d139e2ca37d54c0a7e654148637e07946c830b8f58e",
    ),
    (
        "new-project",
        "threeterm.command.new-project/1",
        "threeterm.command.new-project.request/1",
        "threeterm.command.new-project.response/1",
        "47253ca99da82b00965e962534a6a7942af7f00e2952997b9b97a3437c7064c7",
        "14402e6af7365248d96e5b01256b83fa6965d975f5754fec8aab3505326ba18c",
    ),
    (
        "save",
        "threeterm.command.save/1",
        "threeterm.command.save.request/1",
        "threeterm.command.save.response/1",
        "849b03bf4fbdbc5ff292252db290bf2320f8bfe5afbacbfe5141309ae1b60019",
        "127fedb41c0c183f5c79fddbf91a6ebab39d58ab454e9c3b403a16ec4839d0d9",
    ),
    (
        "load",
        "threeterm.command.load/1",
        "threeterm.command.load.request/1",
        "threeterm.command.load.response/2",
        "bdd2fcc2f3e3d9007185880a3f57d3b82925a63d1861ae73f316f6ae6b27c541",
        "31cd08b935d28b0ebde673b72ff9914fe26a028b6b212dd4d9e113f3f2cdd193",
    ),
    (
        "extrude",
        "threeterm.command.extrude/2",
        "threeterm.command.extrude.request/2",
        "threeterm.command.extrude.response/4",
        "976e758d121a1337a55dfcc19a50f457f2c115f6a34b93275fe210df521347c4",
        "a99f912f4bc5af6080140a80855ec12655a84eb86fb650ea2d7411fa9865560a",
    ),
    (
        "boolean-fuse",
        "threeterm.command.boolean-fuse/1",
        "threeterm.command.boolean-fuse.request/1",
        "threeterm.command.boolean-fuse.response/1",
        "a2a2150f6667b14f8bc7deaa62fbc0b21e2e54700cffce9f0de50da77ef82bd7",
        "1f47faa8f45285e924e257c69b6152e611236eb810ffc67509e0badc8df0888a",
    ),
    (
        "fillet",
        "threeterm.command.fillet/1",
        "threeterm.command.fillet.request/1",
        "threeterm.command.fillet.response/1",
        "dfa226e5e1f5b77ac28c54d892b0e1acf5d409239dd11a86b1ae4f4132f7d5a6",
        "e32c1f65995737071298d52ff675877ebae59a8b0438df815093eb07440f3ec5",
    ),
    (
        "chamfer",
        "threeterm.command.chamfer/1",
        "threeterm.command.chamfer.request/1",
        "threeterm.command.chamfer.response/1",
        "9f240a86a6e2c2620575213f2cef30218a8c3ffadb3048458eeabbf6e065e67c",
        "e32c1f65995737071298d52ff675877ebae59a8b0438df815093eb07440f3ec5",
    ),
    (
        "hole",
        "threeterm.command.hole/1",
        "threeterm.command.hole.request/1",
        "threeterm.command.hole.response/1",
        "e334102473422ff7d774eceb8b5b5c82f51f8b965430c36bee4c3f3f0b8cb803",
        "1f47faa8f45285e924e257c69b6152e611236eb810ffc67509e0badc8df0888a",
    ),
    (
        "revolve",
        "threeterm.command.revolve/1",
        "threeterm.command.revolve.request/1",
        "threeterm.command.revolve.response/1",
        "cdf071216038d971bed8e85714b91e9f07e12638cc09d6f129891e628fe68d3f",
        "1f47faa8f45285e924e257c69b6152e611236eb810ffc67509e0badc8df0888a",
    ),
    (
        "mirror",
        "threeterm.command.mirror/1",
        "threeterm.command.mirror.request/1",
        "threeterm.command.mirror.response/1",
        "317c67381a96e0f6bbcbd5ab0b08520f838150e767507c05bb02faddd0fd1e6c",
        "1f47faa8f45285e924e257c69b6152e611236eb810ffc67509e0badc8df0888a",
    ),
    (
        "linear-pattern",
        "threeterm.command.linear-pattern/1",
        "threeterm.command.linear-pattern.request/1",
        "threeterm.command.linear-pattern.response/1",
        "af98d3e8be22c0caa683bf232c2f5c4f05822bd48046a67dc213117b1de078fe",
        "1f47faa8f45285e924e257c69b6152e611236eb810ffc67509e0badc8df0888a",
    ),
    (
        "circular-pattern",
        "threeterm.command.circular-pattern/1",
        "threeterm.command.circular-pattern.request/1",
        "threeterm.command.circular-pattern.response/1",
        "d4bd981cf94980121136a7372420a6da1e90acdf0416e6cab5ec3d873c6de5ab",
        "1f47faa8f45285e924e257c69b6152e611236eb810ffc67509e0badc8df0888a",
    ),
    (
        "shell",
        "threeterm.command.shell/1",
        "threeterm.command.shell.request/1",
        "threeterm.command.shell.response/1",
        "4705622ec38b335bb8f6140b0e7d7f2d489ec9fb4449c7a733ad3882997042f3",
        "0d4ed1be13a185f455b70b71802bf142373d58f34c53a775292a32744abc07f6",
    ),
    (
        "draft",
        "threeterm.command.draft/1",
        "threeterm.command.draft.request/1",
        "threeterm.command.draft.response/1",
        "4978ede88fc8cd9af6f9e3ca3bcff4e206af61a7ea5756410e01324ae2218483",
        "0d4ed1be13a185f455b70b71802bf142373d58f34c53a775292a32744abc07f6",
    ),
    (
        "loft",
        "threeterm.command.loft/1",
        "threeterm.command.loft.request/1",
        "threeterm.command.loft.response/1",
        "03d424e489b849d6e993a9c0be7d214ca6dfb14414ea00281e0572807dc32416",
        "0d4ed1be13a185f455b70b71802bf142373d58f34c53a775292a32744abc07f6",
    ),
];

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|key| {
                        format!(
                            "{}:{}",
                            serde_json::to_string(key).expect("schema key serializes"),
                            canonical_json(&object[key])
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => serde_json::to_string(value).expect("schema value serializes"),
    }
}

fn schema_hash(schema: &Value) -> String {
    sha256_hex(canonical_json(schema).as_bytes())
}

fn valid_schema_fixture(schema: &Value) -> Value {
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
    if let Some(value) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .and_then(|schemas| schemas.first())
    {
        return valid_schema_fixture(value);
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let properties = schema.get("properties").and_then(Value::as_object);
            let required = schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            let mut object = serde_json::Map::new();
            for name in required {
                let value = properties
                    .and_then(|properties| properties.get(name))
                    .map(valid_schema_fixture)
                    .unwrap_or(Value::Null);
                object.insert(name.to_string(), value);
            }
            Value::Object(object)
        }
        Some("array") => {
            let count = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            let item_schema = schema.get("items").unwrap_or(&Value::Null);
            Value::Array(
                (0..count)
                    .map(|index| {
                        let mut value = valid_schema_fixture(item_schema);
                        if schema.get("uniqueItems") == Some(&Value::Bool(true))
                            && let Value::String(text) = &mut value
                        {
                            text.push_str(&format!("-{index}"));
                        }
                        value
                    })
                    .collect(),
            )
        }
        Some("string") => {
            if schema.get("pattern").and_then(Value::as_str) == Some("^[0-9a-f]{64}$") {
                Value::String("0".repeat(64))
            } else {
                Value::String("fixture-value".to_string())
            }
        }
        Some("integer") => {
            let minimum = schema
                .get("minimum")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .ceil() as i64;
            json!(minimum.max(1))
        }
        Some("number") => {
            let minimum = schema.get("minimum").and_then(Value::as_f64).unwrap_or(1.0);
            json!(minimum.max(1.0))
        }
        Some("boolean") => Value::Bool(false),
        Some("null") => Value::Null,
        Some(_) | None => Value::Null,
    }
}

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-domain-executor-{label}-{suffix}"))
}

fn filesystem_snapshot(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn visit(path: &std::path::Path, root: &std::path::Path, entries: &mut Vec<(String, Vec<u8>)>) {
        let metadata = fs::symlink_metadata(path).expect("filesystem metadata");
        let relative = path
            .strip_prefix(root)
            .expect("snapshot path is under root")
            .to_string_lossy()
            .into_owned();
        if metadata.file_type().is_symlink() {
            entries.push((
                relative,
                format!(
                    "symlink:{}",
                    fs::read_link(path).expect("symlink target").display()
                )
                .into_bytes(),
            ));
        } else if metadata.is_dir() {
            entries.push((relative.clone(), b"directory".to_vec()));
            let mut children = fs::read_dir(path)
                .expect("directory reads")
                .map(|entry| entry.expect("directory entry reads").path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                visit(&child, root, entries);
            }
        } else {
            entries.push((relative, fs::read(path).expect("file contents")));
        }
    }

    let mut entries = Vec::new();
    if path.exists() {
        visit(path, path, &mut entries);
    }
    entries.sort();
    entries
}

fn identity_request(path: &std::path::Path) -> Value {
    json!({"bundle_path": path.to_string_lossy()})
}

fn apply_request(path: &std::path::Path, revision: &str, kind: Option<&str>) -> Value {
    let mut request = json!({
        "bundle_path": path.to_string_lossy(),
        "expected_revision": revision,
        "operation": "add",
        "feature_id": "box",
    });
    if let Some(kind) = kind {
        request["kind"] = kind.into();
    }
    request
}

fn extrude_request(path: &std::path::Path, revision: Option<&str>) -> Value {
    let mut request = json!({
        "bundle_path": path.to_string_lossy(),
        "feature_id": "keyboard-extrude",
        "profile": [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]],
        "height": 3.0,
        "mode": "additive",
    });
    if let Some(revision) = revision {
        request["expected_revision"] = revision.into();
    }
    request
}

fn transform_request(
    path: &std::path::Path,
    feature_id: &str,
    base_feature_id: Option<&str>,
    revision: &str,
) -> Value {
    let mut request = match base_feature_id {
        None => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "profile": [[0.0, 0.0], [4.0, 0.0], [2.0, 4.0]],
            "axis_point": [0.0, 0.0, 0.0],
            "axis_direction": [0.0, 1.0, 0.0],
            "angle": std::f64::consts::PI,
        }),
        Some(base_feature_id) if feature_id.starts_with("mirror") => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "base_feature_id": base_feature_id,
            "plane_point": [0.0, 0.0, 0.0],
            "plane_normal": [1.0, 0.0, 0.0],
        }),
        Some(base_feature_id) if feature_id.starts_with("linear") => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "base_feature_id": base_feature_id,
            "direction": [1.0, 0.0, 0.0],
            "count": 2,
            "spacing": 5.0,
        }),
        Some(base_feature_id) => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "base_feature_id": base_feature_id,
            "axis_point": [0.0, 0.0, 0.0],
            "axis_normal": [0.0, 0.0, 1.0],
            "angle_step": std::f64::consts::FRAC_PI_2,
            "count": 2,
        }),
    };
    request["expected_revision"] = revision.into();
    request
}

fn registry_request(name: &str, path: &std::path::Path, revision: &str) -> Value {
    let bundle_path = path.to_string_lossy();
    let edge = json!({
        "semantic_id": "edge",
        "provenance": {
            "source_feature_id": "base",
            "source_revision_id": revision,
            "source_edge_id": "edge"
        },
        "role": "outer-perimeter",
        "evidence": {
            "midpoint": [0.0, 0.0, 0.0],
            "tangent": [1.0, 0.0, 0.0],
            "length": 1.0
        }
    });

    match name {
        "list" => json!({}),
        "new-project" => json!({
            "destination": path.join("new-project").to_string_lossy()
        }),
        "identity" | "load" | "component-state" | "replay-verify" | "undo" | "redo" => {
            json!({"bundle_path": bundle_path})
        }
        "rehearse" => json!({
            "output_dir": path.join("rehearsal").to_string_lossy(),
            "release_candidate": "rc-1"
        }),
        "apply" => apply_request(path, revision, Some("cube")),
        "save" => json!({
            "bundle_path": bundle_path,
            "feature_id": "box",
            "kind": "cube"
        }),
        "bracket" => json!({
            "bundle_path": bundle_path,
            "bracket_id": "bracket",
            "length": 60.0,
            "width": 30.0,
            "height": 40.0,
            "thickness": 3.0
        }),
        "define-component" => json!({
            "bundle_path": bundle_path,
            "definition_id": "definition",
            "feature_id": "box",
            "length": 10.0,
            "width": 5.0,
            "height": 3.0,
            "thickness": 1.0
        }),
        "create-component-instance" => json!({
            "bundle_path": bundle_path,
            "instance_id": "instance",
            "definition_id": "definition",
            "transform": [0.0, 0.0, 0.0]
        }),
        "transform-component-instance" => json!({
            "bundle_path": bundle_path,
            "instance_id": "instance",
            "transform": [1.0, 0.0, 0.0]
        }),
        "make-component-independent" => json!({
            "bundle_path": bundle_path,
            "source_instance_id": "instance",
            "definition_id": "definition",
            "instance_id": "independent",
            "feature_id": "independent-feature"
        }),
        "edit-component-parameter" => json!({
            "bundle_path": bundle_path,
            "definition_id": "definition",
            "parameter": "length",
            "value": 11.0
        }),
        "sketch-solve" => json!({
            "bundle_path": bundle_path,
            "feature_id": "sketch",
            "phase": "preview",
            "entities": [{"kind": "point", "id": "point", "x": 0.0, "y": 0.0}],
            "constraints": []
        }),
        "bracket-edit" => json!({
            "phase": "discard",
            "bundle_path": bundle_path,
            "draft_id": "draft",
            "bracket_id": "bracket",
            "length": 60.0,
            "width": 30.0,
            "height": 40.0,
            "thickness": 3.0
        }),
        "capture-component" => json!({
            "bundle_path": bundle_path,
            "definition_id": "definition",
            "selected_feature_ids": ["box"]
        }),
        "historical-edit" => json!({
            "bundle_path": bundle_path,
            "feature_id": "box",
            "parameter": "length",
            "value": 11.0
        }),
        "create-revision" => json!({
            "bundle_path": bundle_path,
            "name": "checkpoint"
        }),
        "restore-revision" => json!({
            "bundle_path": bundle_path,
            "feature_id": "box",
            "name": "checkpoint"
        }),
        "timeline" => json!({
            "bundle_path": bundle_path,
            "feature_id": "box"
        }),
        "extrude" => json!({
            "bundle_path": bundle_path,
            "feature_id": "extrude",
            "profile": [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            "height": 2.0,
            "mode": "additive"
        }),
        "fit-dimension" => json!({
            "bundle_path": bundle_path,
            "expected_revision": revision,
            "source_feature_id": "source",
            "target_feature_id": "target",
            "source_dimension_id": "source-length",
            "target_dimension_id": "target-length",
            "dimension": "length",
            "clearance": 1.0
        }),
        "boolean-fuse" | "boolean-cut" | "boolean-common" => json!({
            "bundle_path": bundle_path,
            "feature_id": "result",
            "base_feature_id": "base",
            "tool_feature_id": "tool"
        }),
        "boolean-pattern" => json!({
            "bundle_path": bundle_path,
            "feature_id": "pattern",
            "base_feature_id": "base",
            "origin": [0.0, 0.0, 0.0],
            "spacing": [1.0, 1.0],
            "columns": 1,
            "rows": 1,
            "diameter": 1.0
        }),
        "fillet" => json!({
            "bundle_path": bundle_path,
            "feature_id": "fillet",
            "base_feature_id": "base",
            "radius": 0.2,
            "expected_revision": revision,
            "selected_edge": edge
        }),
        "chamfer" => json!({
            "bundle_path": bundle_path,
            "feature_id": "chamfer",
            "base_feature_id": "base",
            "distance": 0.2,
            "expected_revision": revision,
            "selected_edge": edge
        }),
        "reattach-edge" => json!({
            "bundle_path": bundle_path,
            "expected_revision": revision,
            "edit_feature_id": "edge-edit",
            "edit_kind": "split",
            "base_feature_id": "base",
            "radius": 0.2,
            "plane_point": [0.0, 0.0, 0.0],
            "plane_normal": [1.0, 0.0, 0.0],
            "reference": edge,
            "edit_target": edge
        }),
        "hole" => json!({
            "bundle_path": bundle_path,
            "feature_id": "hole",
            "base_feature_id": "base",
            "position": [0.0, 0.0, 0.0],
            "direction": [0.0, 0.0, 1.0],
            "diameter": 1.0,
            "hole_kind": "drilled"
        }),
        "revolve" => json!({
            "bundle_path": bundle_path,
            "feature_id": "revolve",
            "profile": [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            "axis_point": [0.0, 0.0, 0.0],
            "axis_direction": [0.0, 1.0, 0.0],
            "angle": std::f64::consts::PI
        }),
        "mirror" => json!({
            "bundle_path": bundle_path,
            "feature_id": "mirror",
            "base_feature_id": "base",
            "plane_point": [0.0, 0.0, 0.0],
            "plane_normal": [1.0, 0.0, 0.0]
        }),
        "linear-pattern" => json!({
            "bundle_path": bundle_path,
            "feature_id": "linear-pattern",
            "base_feature_id": "base",
            "direction": [1.0, 0.0, 0.0],
            "count": 2,
            "spacing": 1.0
        }),
        "circular-pattern" => json!({
            "bundle_path": bundle_path,
            "feature_id": "circular-pattern",
            "base_feature_id": "base",
            "axis_point": [0.0, 0.0, 0.0],
            "axis_normal": [0.0, 0.0, 1.0],
            "angle_step": 1.0,
            "count": 2
        }),
        "shell" => json!({
            "bundle_path": bundle_path,
            "feature_id": "shell",
            "base_feature_id": "base",
            "thickness": 0.2,
            "expected_revision": revision
        }),
        "draft" => json!({
            "bundle_path": bundle_path,
            "feature_id": "draft",
            "base_feature_id": "base",
            "angle": 0.2,
            "pull_direction": [0.0, 0.0, 1.0],
            "expected_revision": revision
        }),
        "loft" => json!({
            "bundle_path": bundle_path,
            "feature_id": "loft",
            "profiles": [
                [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]]
            ],
            "expected_revision": revision,
            "is_solid": true,
            "ruled": false
        }),
        "export" => json!({
            "bundle_path": bundle_path,
            "feature_id": "base",
            "formats": ["step"],
            "output_dir": path.join("export").to_string_lossy(),
            "tessellation_deflection": 0.1,
            "override_warnings": false,
            "accept_stale_geometry": false
        }),
        other => panic!("registry fixture is missing for {other}"),
    }
}

fn rehearsal_response_fixture() -> Value {
    let classes = [
        "project_create",
        "bracket_create",
        "edit_open",
        "edit_update",
        "edit_preview",
        "edit_commit",
        "reload",
        "export",
        "catalog",
    ];
    let run = |prefix: &str| {
        json!({
            "schema_version": "threeterm.command.rehearse.run.response/1",
            "release_candidate": "rc-1",
            "project_path": format!("{prefix}/project"),
            "export_path": format!("{prefix}/export"),
            "catalog_path": format!("{prefix}/sha256-manifest.json"),
            "timings": classes.iter().map(|class| json!({
                "class": class,
                "unit": "ms",
                "sample_count": 1,
                "samples_ms": [1.0],
                "p50_ms": 1.0,
                "p95_ms": 1.0,
                "p99_ms": 1.0
            })).collect::<Vec<_>>(),
            "artifacts": []
        })
    };
    let comparison = |class: &str| {
        json!({
            "class": class,
            "run_1": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0},
            "run_2": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0},
            "same_order_of_magnitude": true
        })
    };
    json!({
        "schema_version": "threeterm.command.rehearse.response/2",
        "release_candidates": ["rc-1", "rc-2"],
        "fixture": "l-bracket",
        "run_count": 2,
        "sample_policy": "nearest-rank",
        "promoted": false,
        "runs": [run("run-1"), run("run-2")],
        "comparisons": classes.iter().map(|class| comparison(class)).collect::<Vec<_>>()
    })
}

#[test]
fn public_dispatcher_routes_sixteen_baseline_commands_and_preserves_lifecycle_contract() {
    let registry = threeterm_protocol::schema::iter().collect::<Vec<_>>();
    let baseline_ids = BASELINE_COMMANDS.into_iter().collect::<HashSet<_>>();
    assert_eq!(baseline_ids.len(), BASELINE_COMMANDS.len());
    assert_eq!(
        registry
            .iter()
            .filter(|entry| baseline_ids.contains(&entry.id))
            .count(),
        BASELINE_COMMANDS.len()
    );
    let baseline_projection = registry
        .iter()
        .filter(|entry| baseline_ids.contains(&entry.id))
        .map(|entry| entry.id)
        .collect::<Vec<_>>();
    let mut expected_baseline_projection = BASELINE_COMMANDS.to_vec();
    expected_baseline_projection.sort_unstable();
    assert_eq!(
        baseline_projection, expected_baseline_projection,
        "the registry must contain exactly the qualified baseline IDs"
    );
    let extras = registry
        .iter()
        .filter(|entry| !baseline_ids.contains(&entry.id))
        .map(|entry| entry.id.0)
        .collect::<Vec<_>>();
    if !extras.is_empty() {
        eprintln!("registry entries outside the baseline projection: {extras:?}");
    }
    for command in BASELINE_COMMANDS {
        let schema = threeterm_protocol::schema::find(command)
            .unwrap_or_else(|| panic!("baseline command {} is not registered", command.0));
        assert!(
            registry.iter().any(|entry| entry.id == command),
            "baseline command {} is missing from the enumerated registry",
            command.0
        );
        assert!(!schema.request_schema.is_null());
        assert!(!schema.response_schema.is_null());
        assert!(!schema.request_schema_version.is_empty());
        assert!(!schema.response_schema_version.is_empty());
    }

    let lifecycle_parent = root("public-dispatcher-lifecycle");
    let lifecycle_root = lifecycle_parent.join("new-project");
    let mut saved_hashes = None;
    let native_worker_required = std::env::var_os("THREETERM_REQUIRE_OCCT").is_some();
    let mut rows = Vec::new();
    for command in BASELINE_COMMANDS {
        let command_root = match command {
            NEW_PROJECT_COMMAND_ID => lifecycle_parent.clone(),
            SAVE_COMMAND_ID | LOAD_COMMAND_ID => lifecycle_root.clone(),
            _ => root(&format!("public-dispatcher-{}", command.0)),
        };
        let revision = match command {
            LIST_COMMAND_ID | NEW_PROJECT_COMMAND_ID => String::new(),
            SAVE_COMMAND_ID | LOAD_COMMAND_ID => Bundle::at(&command_root)
                .open()
                .expect("lifecycle fixture opens")
                .revision_hash_hex()
                .to_string(),
            _ => Bundle::create(&command_root)
                .expect("isolated baseline fixture creates")
                .open()
                .expect("isolated baseline fixture opens")
                .revision_hash_hex()
                .to_string(),
        };
        let request = registry_request(command.0, &command_root, &revision);
        let schema = threeterm_protocol::schema::find(command).expect("baseline schema exists");
        validate(&schema.request_schema, &request).unwrap_or_else(|error| {
            panic!("baseline fixture for {} is invalid: {error}", command.0)
        });
        let before_entries = filesystem_snapshot(&command_root);
        let before_manifest = fs::read(command_root.join("manifest.json")).ok();
        let before_log = fs::read(command_root.join("transactions.log")).ok();
        let before = Bundle::at(&command_root).open().ok();
        let before_revision = before
            .as_ref()
            .map(|snapshot| snapshot.revision_hash_hex().to_string())
            .unwrap_or_default();
        let before_log_length = before.as_ref().map_or(0, |snapshot| snapshot.log.len());
        let before_artifacts = before_entries
            .iter()
            .filter(|(path, _)| path.starts_with("brep/") && path.ends_with(".brep"))
            .count();
        let result_kind;
        let mut diagnostic_code = None;
        let mut artifact_path = None;
        let result = Host::new().execute_domain_command(command, request);
        match result {
            Ok(response) => {
                assert!(
                    !matches!(
                        command,
                        BOOLEAN_FUSE_COMMAND_ID
                            | FILLET_COMMAND_ID
                            | CHAMFER_COMMAND_ID
                            | HOLE_COMMAND_ID
                            | MIRROR_COMMAND_ID
                            | LINEAR_PATTERN_COMMAND_ID
                            | CIRCULAR_PATTERN_COMMAND_ID
                            | SHELL_COMMAND_ID
                            | DRAFT_COMMAND_ID
                    ),
                    "base-dependent command {} unexpectedly succeeded: {response:?}",
                    command.0
                );
                result_kind = "success".to_string();
                validate(&schema.response_schema, &response).unwrap_or_else(|error| {
                    panic!("response for {} fails its schema: {error}", command.0)
                });
                if command == LIST_COMMAND_ID {
                    let expected = registry
                        .iter()
                        .map(|entry| {
                            serde_json::to_value(entry).expect("registry entry serializes")
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(response, Value::Array(expected));
                    assert_eq!(filesystem_snapshot(&command_root), before_entries);
                } else if command == NEW_PROJECT_COMMAND_ID {
                    let created = Bundle::at(&lifecycle_root)
                        .open()
                        .expect("new project opens");
                    assert!(created.graph.features().next().is_none());
                    assert_eq!(created.log.len(), 0);
                } else if command == SAVE_COMMAND_ID {
                    let saved = Bundle::at(&lifecycle_root)
                        .open()
                        .expect("saved project opens");
                    assert_eq!(saved.log.len(), 1);
                    saved_hashes = Some((
                        response["feature_graph_hash"]
                            .as_str()
                            .expect("save response has graph hash")
                            .to_string(),
                        response["revision_hash"]
                            .as_str()
                            .expect("save response has revision hash")
                            .to_string(),
                    ));
                } else if command == LOAD_COMMAND_ID {
                    let (feature_graph_hash, revision_hash) =
                        saved_hashes.as_ref().expect("load follows save");
                    assert_eq!(
                        response["feature_graph_hash"].as_str(),
                        Some(feature_graph_hash.as_str())
                    );
                    assert_eq!(
                        response["revision_hash"].as_str(),
                        Some(revision_hash.as_str())
                    );
                }
                if matches!(
                    command,
                    EXTRUDE_COMMAND_ID | REVOLVE_COMMAND_ID | LOFT_COMMAND_ID
                ) {
                    assert!(response.get("brep_path").is_some());
                    let derived = response
                        .get("derived_result")
                        .and_then(Value::as_object)
                        .expect("geometry response includes derived-result provenance");
                    assert_eq!(derived["artifact_kind"], "brep");
                    assert_eq!(derived["artifact_name"], response["artifact_name"]);
                    assert_eq!(derived["byte_count"], response["brep_bytes"]);
                    assert_eq!(derived["sha256"], response["brep_sha256"]);
                }
                if response.get("brep_path").is_some() {
                    let brep_path = response["brep_path"]
                        .as_str()
                        .expect("geometry response has a string BREP path");
                    artifact_path = Some(brep_path.to_string());
                    assert!(std::path::Path::new(brep_path).is_file());
                    assert!(
                        response["brep_sha256"]
                            .as_str()
                            .is_some_and(|hash| !hash.is_empty())
                    );
                    assert!(
                        response["brep_bytes"]
                            .as_u64()
                            .is_some_and(|bytes| bytes > 0)
                    );
                    let after = Bundle::at(&command_root)
                        .open()
                        .expect("successful geometry bundle reopens");
                    assert_eq!(
                        after.log.len(),
                        before
                            .as_ref()
                            .expect("successful geometry starts from a bundle")
                            .log
                            .len()
                            + 1
                    );
                    assert_ne!(
                        after.revision_hash_hex(),
                        before
                            .as_ref()
                            .expect("successful geometry starts from a bundle")
                            .revision_hash_hex()
                    );
                }
            }
            Err(ExecutionError::Handler(error)) => {
                result_kind = "handler".to_string();
                let native_worker_unavailable =
                    matches!(&error, HostError::WorkerUnavailable { .. });
                if matches!(
                    command,
                    EXTRUDE_COMMAND_ID | REVOLVE_COMMAND_ID | LOFT_COMMAND_ID
                ) {
                    assert!(matches!(
                        error,
                        HostError::WorkerUnavailable { .. } | HostError::UnsupportedGeometry { .. }
                    ));
                } else if matches!(
                    command,
                    BOOLEAN_FUSE_COMMAND_ID
                        | FILLET_COMMAND_ID
                        | CHAMFER_COMMAND_ID
                        | HOLE_COMMAND_ID
                        | MIRROR_COMMAND_ID
                        | LINEAR_PATTERN_COMMAND_ID
                        | CIRCULAR_PATTERN_COMMAND_ID
                        | SHELL_COMMAND_ID
                        | DRAFT_COMMAND_ID
                ) {
                    let expected_detail = match command {
                        BOOLEAN_FUSE_COMMAND_ID => "boolean operand feature is missing: base",
                        FILLET_COMMAND_ID | CHAMFER_COMMAND_ID | SHELL_COMMAND_ID
                        | DRAFT_COMMAND_ID => "finishing base feature is missing: base",
                        HOLE_COMMAND_ID => "hole base feature is missing: base",
                        MIRROR_COMMAND_ID
                        | LINEAR_PATTERN_COMMAND_ID
                        | CIRCULAR_PATTERN_COMMAND_ID => "base feature is missing: base",
                        _ => unreachable!(),
                    };
                    assert!(
                        matches!(&error, HostError::Validation { detail } if detail == expected_detail),
                        "expected {expected_detail:?} for {}: {error:?}",
                        command.0
                    );
                } else {
                    assert!(
                        matches!(
                            error,
                            HostError::Validation { ref detail }
                                if detail.contains("missing") && detail.contains("base")
                        ),
                        "expected missing-base validation for {}: {error:?}",
                        command.0
                    );
                }
                let diagnostic = domain_command_diagnostic(&error);
                diagnostic_code = Some(
                    serde_json::to_value(diagnostic.code)
                        .expect("diagnostic code serializes")
                        .as_str()
                        .expect("diagnostic code is a string")
                        .to_string(),
                );
                assert_eq!(
                    diagnostic.schema_version,
                    threeterm_protocol::schema_version()
                );
                let expected_code = if matches!(
                    command,
                    EXTRUDE_COMMAND_ID | REVOLVE_COMMAND_ID | LOFT_COMMAND_ID
                ) {
                    matches!(
                        diagnostic.code,
                        threeterm_protocol::diagnostic::DiagnosticCode::WorkerFailure
                            | threeterm_protocol::diagnostic::DiagnosticCode::UnsupportedGeometry
                    )
                } else {
                    diagnostic.code
                        == threeterm_protocol::diagnostic::DiagnosticCode::InvalidRequest
                };
                assert!(expected_code, "unexpected diagnostic for {}", command.0);
                assert_eq!(filesystem_snapshot(&command_root), before_entries);
                assert_eq!(
                    fs::read(command_root.join("manifest.json")).ok(),
                    before_manifest
                );
                assert_eq!(
                    fs::read(command_root.join("transactions.log")).ok(),
                    before_log
                );
                if native_worker_unavailable {
                    assert!(!native_worker_required, "native OCCT worker is required");
                    eprintln!("{} skipped: native OCCT worker is unavailable", command.0);
                }
            }
            Err(error) => panic!(
                "baseline command {} bypassed the host handler: {error:?}",
                command.0
            ),
        }
        let observed_root = if command == NEW_PROJECT_COMMAND_ID {
            &lifecycle_root
        } else {
            &command_root
        };
        let after = Bundle::at(observed_root).open().ok();
        let after_revision = after
            .as_ref()
            .map(|snapshot| snapshot.revision_hash_hex().to_string())
            .unwrap_or_default();
        let after_log_length = after.as_ref().map_or(0, |snapshot| snapshot.log.len());
        let after_artifacts = filesystem_snapshot(observed_root)
            .into_iter()
            .filter(|(path, _)| path.starts_with("brep/") && path.ends_with(".brep"))
            .count();
        if result_kind == "success"
            && matches!(
                command,
                EXTRUDE_COMMAND_ID | REVOLVE_COMMAND_ID | LOFT_COMMAND_ID
            )
        {
            assert_eq!(
                after_artifacts,
                before_artifacts + 1,
                "successful geometry must promote exactly one BREP"
            );
        }
        rows.push(ResultRow {
            command_id: command.0.to_string(),
            request_schema_version: schema.request_schema_version.to_string(),
            response_schema_version: schema.response_schema_version.to_string(),
            result_kind,
            diagnostic_code,
            canonical_revision_before: before_revision,
            canonical_revision_after: after_revision,
            log_length_before: before_log_length,
            log_length_after: after_log_length,
            artifact_path,
        });
        if !matches!(
            command,
            NEW_PROJECT_COMMAND_ID | SAVE_COMMAND_ID | LOAD_COMMAND_ID
        ) {
            let _ = fs::remove_dir_all(&command_root);
        }
    }

    let _ = fs::remove_dir_all(&lifecycle_parent);
    assert_eq!(rows.len(), BASELINE_COMMANDS.len());
    assert_eq!(
        rows.iter()
            .map(|row| row.command_id.as_str())
            .collect::<Vec<_>>(),
        BASELINE_COMMANDS
            .iter()
            .map(|command| command.0)
            .collect::<Vec<_>>()
    );
    for row in &rows {
        assert!(!row.request_schema_version.is_empty());
        assert!(!row.response_schema_version.is_empty());
        match row.result_kind.as_str() {
            "success" => assert!(row.diagnostic_code.is_none()),
            "handler" => assert!(matches!(
                row.diagnostic_code.as_deref(),
                Some("invalid_request") | Some("worker_failure") | Some("unsupported_geometry")
            )),
            other => panic!("unexpected result kind {other}"),
        }
        if row.command_id == "new-project" {
            assert!(!row.canonical_revision_after.is_empty());
            assert_eq!(row.log_length_after, 0);
        } else if row.command_id == "save" {
            assert_eq!(row.log_length_after, row.log_length_before + 1);
            assert_ne!(row.canonical_revision_after, row.canonical_revision_before);
        } else if row.command_id == "load" || row.command_id == "list" {
            assert_eq!(row.canonical_revision_after, row.canonical_revision_before);
            assert_eq!(row.log_length_after, row.log_length_before);
        } else if row.result_kind == "handler" {
            assert_eq!(row.canonical_revision_after, row.canonical_revision_before);
            assert_eq!(row.log_length_after, row.log_length_before);
            assert!(row.artifact_path.is_none());
        }
        if matches!(row.command_id.as_str(), "extrude" | "revolve" | "loft")
            && row.result_kind == "success"
        {
            assert!(row.artifact_path.is_some());
        }
    }
}

#[test]
fn public_dispatcher_preserves_the_canonical_bracket_request_identity() {
    match OcctWorker::locate() {
        Ok(_) => {}
        Err(error) if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() => {
            panic!("bracket identity integration requires the native worker: {error}");
        }
        Err(error) => {
            eprintln!("bracket identity integration skipped: {error}");
            return;
        }
    };
    let root = root("bracket-identity");
    let bracket_id = "identity-bracket";
    let dimensions = (60.0, 30.0, 40.0, 3.0);
    let expected_request_id = canonical_bracket_request_id(
        bracket_id,
        dimensions.0,
        dimensions.1,
        dimensions.2,
        dimensions.3,
    );
    let view = Host::new()
        .execute_bracket_command(
            &root,
            bracket_id,
            dimensions.0,
            dimensions.1,
            dimensions.2,
            dimensions.3,
        )
        .expect("bracket command executes");
    assert_eq!(view.result.request_id, expected_request_id);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn public_dispatcher_rejects_unknown_commands_at_the_public_boundary() {
    let error = Host::new()
        .execute_domain_command(threeterm_protocol::schema::CommandId("missing"), json!({}))
        .expect_err("unknown command must be rejected");
    assert!(matches!(
        error,
        ExecutionError::UnknownCommand(threeterm_protocol::schema::CommandId("missing"))
    ));
    let diagnostic = domain_execution_diagnostic(&error);
    assert_eq!(
        diagnostic.code,
        threeterm_protocol::diagnostic::DiagnosticCode::UnknownCommand
    );
    assert_eq!(diagnostic.arg, "missing");
}

#[test]
fn baseline_schema_contracts_match_the_immutable_snapshot() {
    for (
        name,
        schema_version,
        request_schema_version,
        response_schema_version,
        request_hash,
        response_hash,
    ) in BASELINE_SCHEMA_CONTRACTS
    {
        let schema = threeterm_protocol::schema::find(threeterm_protocol::schema::CommandId(name))
            .expect("baseline schema exists");
        assert_eq!(
            schema.schema_version, schema_version,
            "{name} command schema drifted"
        );
        assert_eq!(
            schema.request_schema_version, request_schema_version,
            "{name} request schema version drifted"
        );
        assert_eq!(
            schema.response_schema_version, response_schema_version,
            "{name} response schema version drifted"
        );
        assert_eq!(
            schema_hash(&schema.request_schema),
            request_hash,
            "{name} request schema drifted"
        );
        assert_eq!(
            schema_hash(&schema.response_schema),
            response_hash,
            "{name} response schema drifted"
        );
    }
}

#[test]
fn baseline_invalid_requests_preserve_canonical_state() {
    for command in BASELINE_COMMANDS {
        let command_root = root(&format!("invalid-{}", command.0));
        let request = if command == LIST_COMMAND_ID {
            assert!(!command_root.exists());
            json!({"unexpected": true})
        } else if command == NEW_PROJECT_COMMAND_ID {
            json!({"destination": ""})
        } else {
            Bundle::create(&command_root).expect("invalid-request fixture creates");
            json!({})
        };
        let before_entries = filesystem_snapshot(&command_root);
        let before_manifest = fs::read(command_root.join("manifest.json")).ok();
        let before_log = fs::read(command_root.join("transactions.log")).ok();

        let result = Host::new().execute_domain_command(command, request);
        assert!(
            matches!(result, Err(ExecutionError::InvalidRequest(_))),
            "{} must be rejected by the shared request validator: {result:?}",
            command.0
        );

        assert_eq!(filesystem_snapshot(&command_root), before_entries);
        assert_eq!(
            fs::read(command_root.join("manifest.json")).ok(),
            before_manifest
        );
        assert_eq!(
            fs::read(command_root.join("transactions.log")).ok(),
            before_log
        );
        let _ = fs::remove_dir_all(&command_root);
    }
}

#[test]
fn every_baseline_id_reaches_the_injected_handler_with_a_valid_response() {
    let host = Host::new();
    for command in BASELINE_COMMANDS {
        let command_root = root(&format!("handler-{}", command.0));
        let revision = if matches!(command, LIST_COMMAND_ID | NEW_PROJECT_COMMAND_ID) {
            String::new()
        } else {
            Bundle::create(&command_root)
                .expect("handler fixture bundle creates")
                .open()
                .expect("handler fixture bundle opens")
                .revision_hash_hex()
                .to_string()
        };
        let request = registry_request(command.0, &command_root, &revision);
        let schema = threeterm_protocol::schema::find(command).expect("baseline schema exists");
        validate(&schema.request_schema, &request).unwrap_or_else(|error| {
            panic!("handler fixture for {} is invalid: {error}", command.0)
        });
        let called = Cell::new(false);
        let response = valid_schema_fixture(&schema.response_schema);
        let result = host.execute_domain_command_with_handler(command, request, |_| {
            called.set(true);
            Ok::<Value, ()>(response)
        });
        assert!(
            result.is_ok(),
            "handler fixture for {} failed: {result:?}",
            command.0
        );
        assert!(called.get(), "handler was not called for {}", command.0);
        let _ = fs::remove_dir_all(&command_root);
    }
}

#[test]
fn shared_executor_validates_before_and_after_the_injected_handler() {
    let host = Host::new();
    let request_called = Cell::new(false);
    let invalid_request = host.execute_domain_command_with_handler(
        LIST_COMMAND_ID,
        json!({"unexpected": true}),
        |_| {
            request_called.set(true);
            Ok::<Value, ()>(json!([]))
        },
    );
    assert!(matches!(
        invalid_request,
        Err(ExecutionError::InvalidRequest(_))
    ));
    assert!(
        !request_called.get(),
        "invalid requests must not reach handlers"
    );

    let root = root("malformed-response");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let before = filesystem_snapshot(&root);
    let response_called = Cell::new(false);
    let invalid_response = host.execute_domain_command_with_handler(
        APPLY_COMMAND_ID,
        apply_request(&root, &revision, Some("cube")),
        |_| {
            response_called.set(true);
            Ok::<Value, ()>(json!({}))
        },
    );
    let Err(ExecutionError::InvalidResponse(detail)) = invalid_response else {
        panic!("malformed handler response must be rejected: {invalid_response:?}");
    };
    assert!(response_called.get());
    assert!(!detail.is_empty());
    let diagnostic = domain_execution_diagnostic(&ExecutionError::<HostError>::InvalidResponse(
        detail.clone(),
    ));
    assert_eq!(
        diagnostic.code,
        threeterm_protocol::diagnostic::DiagnosticCode::IntegrityFailure
    );
    assert_eq!(diagnostic.arg, detail);
    assert_eq!(filesystem_snapshot(&root), before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn domain_execution_diagnostic_maps_all_shared_failures() {
    let persistence_error = Bundle::at(root("diagnostic-missing"))
        .open()
        .expect_err("missing diagnostic bundle must fail");
    let termination = threeterm_protocol::supervisor::TerminationRecord {
        request_id: "req-cancel".to_string(),
        stage: "boolean_pattern".to_string(),
        cancel_reason: Some("cancelled by host".to_string()),
        elapsed: Duration::from_millis(4),
        last_progress: None,
        last_artifact_error: None,
        exit_signal: None,
        exit_code: Some(0),
        stderr_tail: "worker stderr".to_string(),
        failed_code: None,
        failed_detail: None,
        protocol_diagnostic: None,
        termination_error: None,
        exit_kind: threeterm_protocol::supervisor::ExitKind::Cooperative,
    };
    let cases = vec![
        (
            ExecutionError::UnknownCommand(threeterm_protocol::schema::CommandId("missing")),
            threeterm_protocol::diagnostic::DiagnosticCode::UnknownCommand,
            Some("missing"),
            Vec::<&str>::new(),
            None,
        ),
        (
            ExecutionError::InvalidRequest("request is invalid".to_string()),
            threeterm_protocol::diagnostic::DiagnosticCode::InvalidRequest,
            Some("request is invalid"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::InvalidResponse("response is invalid".to_string()),
            threeterm_protocol::diagnostic::DiagnosticCode::IntegrityFailure,
            Some("response is invalid"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::Validation {
                detail: "handler rejected request".to_string(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::InvalidRequest,
            Some("handler rejected request"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::BundlePathMissing {
                path: "/missing-bundle".into(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::PersistenceFailure,
            Some("/missing-bundle"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::BundlePathNotDirectory {
                path: "/not-a-directory".into(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::PersistenceFailure,
            Some("/not-a-directory"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::Persistence(persistence_error)),
            threeterm_protocol::diagnostic::DiagnosticCode::PersistenceFailure,
            None,
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::BrepFileMissing {
                path: "/missing.brep".into(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::PersistenceFailure,
            Some("/missing.brep"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::BrepIo {
                detail: "BREP read failed".to_string(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::PersistenceFailure,
            Some("BREP read failed"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::WorkerFailure {
                request_id: Some("req-worker".to_string()),
                detail: "worker failed".to_string(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::WorkerFailure,
            Some("worker failed"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::WorkerUnavailable {
                detail: "worker missing".to_string(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::WorkerFailure,
            Some("worker missing"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::UnsupportedGeometry {
                request_id: None,
                detail: "unsupported solid".to_string(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::UnsupportedGeometry,
            Some("unsupported solid"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::BrepInvalid {
                request_id: None,
                detail: "invalid BREP".to_string(),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::BrepInvalid,
            Some("invalid BREP"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::WorkerTerminated {
                record: Box::new(termination),
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::WorkerFailure,
            Some("req-cancel"),
            Vec::new(),
            None,
        ),
        (
            ExecutionError::Handler(HostError::InvalidEdit {
                detail: "edge changed".to_string(),
                affected_ids: vec!["edge".to_string(), "edge".to_string(), "face".to_string()],
                recovery: "reselect the edge",
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::BrepInvalid,
            Some("edge changed"),
            vec!["edge", "face"],
            Some("reselect the edge"),
        ),
        (
            ExecutionError::Handler(HostError::InvalidReference {
                detail: "reference lost".to_string(),
                affected_ids: vec!["edge".to_string(), "edge".to_string(), "face".to_string()],
                recovery: "choose another reference",
            }),
            threeterm_protocol::diagnostic::DiagnosticCode::InvalidRequest,
            Some("reference lost"),
            vec!["edge", "face"],
            Some("choose another reference"),
        ),
    ];

    for (error, expected_code, expected_arg, expected_ids, expected_recovery) in cases {
        let diagnostic = domain_execution_diagnostic(&error);
        assert_eq!(diagnostic.code, expected_code);
        assert_eq!(
            diagnostic.schema_version,
            threeterm_protocol::schema_version()
        );
        if let Some(expected_arg) = expected_arg {
            assert!(
                diagnostic.arg.contains(expected_arg),
                "diagnostic arg {:?} does not contain {:?}",
                diagnostic.arg,
                expected_arg
            );
        }
        assert_eq!(diagnostic.affected_ids, expected_ids);
        assert_eq!(diagnostic.recovery.as_deref(), expected_recovery);
        let value = serde_json::to_value(&diagnostic).expect("diagnostic serializes");
        assert_eq!(
            value["schema_version"],
            threeterm_protocol::schema_version()
        );
        assert!(value["code"].is_string());
        assert!(value["arg"].is_string());
    }
}

#[test]
fn public_dispatcher_propagates_cancellation_without_persistence() {
    let root = root("public-dispatcher-cancel");
    let worker_root = root.with_extension("fixture");
    fs::create_dir_all(&worker_root).expect("fixture worker directory creates");
    let worker_script = worker_root.join("worker.sh");
    fs::write(
        &worker_script,
        r##"#!/bin/sh
printf '%s\n' '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"occt"}'
IFS= read -r request || exit 1
request_id=$(printf '%s\n' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
printf '%s\n' "{\"kind\":\"progress\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"$request_id\",\"stage\":\"boolean_pattern:started\",\"percent\":1}"
while :; do sleep 1; done
"##,
    )
    .expect("fixture worker writes");
    fs::set_permissions(&worker_script, fs::Permissions::from_mode(0o700))
        .expect("fixture worker becomes executable");
    let worker = OcctWorker::with_binary_path(worker_script)
        .with_grace(Duration::from_millis(500))
        .with_operation_grace(
            threeterm_occt_worker::Operation::BooleanPattern,
            Duration::from_millis(50),
        );
    let bundle = Bundle::create(&root).expect("cancellation bundle creates");
    let revision = bundle
        .open()
        .expect("cancellation bundle opens")
        .revision_hash_hex()
        .to_string();
    bundle
        .append_feature_with_brep_if_revision("base", "brep:base", &revision, b"base-brep")
        .expect("cancellation base BREP persists");
    let host = Host::new();
    let before = Bundle::at(&root).open().expect("cancellation bundle opens");
    let before_files = filesystem_snapshot(&root);
    let cancel = AtomicBool::new(false);
    let mut progress = Vec::new();
    let result = host.execute_domain_command_with_worker_and_cancel_and_progress(
        BOOLEAN_PATTERN_COMMAND_ID,
        json!({
            "bundle_path": root.to_string_lossy(),
            "feature_id": "cancelled-pattern",
            "base_feature_id": "base",
            "origin": [0.0, 0.0, 0.0],
            "spacing": [1.0, 1.0],
            "columns": 1,
            "rows": 1,
            "diameter": 1.0
        }),
        Some(&worker),
        &cancel,
        &mut |event| {
            progress.push(event.stage.clone());
            cancel.store(true, Ordering::SeqCst);
        },
    );
    let Err(ExecutionError::Handler(HostError::WorkerTerminated { record })) = result else {
        panic!("fixture cancellation must return a structured termination: {result:?}");
    };
    assert!(record.request_id.starts_with("req-"));
    assert!(record.last_progress.is_some());
    assert_eq!(
        record.exit_kind,
        threeterm_protocol::supervisor::ExitKind::ForceAfterGrace
    );
    assert_eq!(
        threeterm_host::domain_execution_diagnostic(&ExecutionError::Handler(
            HostError::WorkerTerminated { record },
        ))
        .code,
        threeterm_protocol::diagnostic::DiagnosticCode::WorkerFailure
    );
    assert_eq!(
        Bundle::at(&root).open().expect("bundle reopens").log.len(),
        before.log.len()
    );
    let after_files = filesystem_snapshot(&root);
    assert_eq!(
        after_files
            .iter()
            .filter(|(path, _)| path != ".derived")
            .collect::<Vec<_>>(),
        before_files.iter().collect::<Vec<_>>()
    );
    assert!(!root.join(".derived/boolean_pattern").exists());
    assert_eq!(progress, ["boolean_pattern:started"]);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&worker_root);
}

#[test]
fn shared_executor_preserves_identity_and_durable_apply_transaction() {
    let root = root("accepted");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();

    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity executes");
    let initial_revision = initial["revision_hash"]
        .as_str()
        .expect("identity has revision hash")
        .to_string();
    let applied = host
        .execute_domain_command(
            APPLY_COMMAND_ID,
            apply_request(&root, &initial_revision, Some("cube")),
        )
        .expect("apply executes");

    assert_eq!(applied["status"], "committed");
    assert_eq!(applied["operation"], "add");
    assert_eq!(applied["feature_id"], "box");
    assert_eq!(applied["transaction_count"], 1);
    assert_ne!(applied["revision_hash"], initial["revision_hash"]);

    let loaded = Bundle::at(&root).open().expect("bundle reloads");
    let entry = &loaded.log.entries()[0];
    assert_eq!(entry.log_index, 0);
    assert_eq!(entry.previous_digest, initial["terminal_log_digest"]);
    assert_eq!(entry.operation.as_deref(), Some("add"));
    assert_eq!(entry.feature_id, "box");
    assert_eq!(entry.kind, "cube");
    assert_eq!(entry.terminal_digest, applied["terminal_log_digest"]);

    let reloaded = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("reloaded identity executes");
    for field in [
        "generation_id",
        "revision_id",
        "feature_graph_hash",
        "revision_hash",
        "transaction_count",
        "terminal_log_digest",
    ] {
        assert_eq!(
            reloaded[field], applied[field],
            "identity field {field} reloads"
        );
    }

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

#[test]
fn shared_executor_distinguishes_schema_semantic_and_stale_rejections() {
    let root = root("rejected");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity executes");
    let revision = initial["revision_hash"].as_str().unwrap();
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");

    let missing_kind =
        host.execute_domain_command(APPLY_COMMAND_ID, apply_request(&root, revision, None));
    assert!(matches!(
        missing_kind,
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let invalid_operation = host.execute_domain_command(
        APPLY_COMMAND_ID,
        json!({
            "bundle_path": root.to_string_lossy(),
            "expected_revision": revision,
            "operation": "rename",
            "feature_id": "box"
        }),
    );
    assert!(matches!(
        invalid_operation,
        Err(ExecutionError::InvalidRequest(_))
    ));

    let applied = host
        .execute_domain_command(
            APPLY_COMMAND_ID,
            apply_request(&root, revision, Some("cube")),
        )
        .expect("apply executes");
    let manifest_after_apply = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_after_apply = fs::read(root.join("transactions.log")).expect("log reads");
    let stale = host.execute_domain_command(
        APPLY_COMMAND_ID,
        apply_request(&root, revision, Some("sphere")),
    );
    assert!(matches!(
        stale,
        Err(ExecutionError::Handler(HostError::Persistence(_)))
    ));
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads after stale rejection"),
        manifest_after_apply
    );
    assert_eq!(applied["transaction_count"], 1);
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads after stale rejection"),
        log_after_apply
    );

    assert_ne!(
        manifest_before,
        fs::read(root.join("manifest.json")).unwrap()
    );
    assert_ne!(log_before, fs::read(root.join("transactions.log")).unwrap());
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

#[test]
fn every_registered_command_reaches_the_shared_executor_and_validates_response() {
    let host = Host::new();
    let root = root("registry-execution");

    for entry in threeterm_protocol::schema::iter() {
        let command_root = root.join(entry.name);
        let revision = if matches!(entry.id.0, "list" | "new-project" | "rehearse") {
            String::new()
        } else {
            Bundle::create(&command_root).expect("registry fixture bundle creates");
            Bundle::at(&command_root)
                .open()
                .expect("registry fixture bundle opens")
                .revision_hash_hex()
                .to_string()
        };
        let request = registry_request(entry.name, &command_root, &revision);
        validate(&entry.request_schema, &request).unwrap_or_else(|error| {
            panic!(
                "registry fixture for {} violates its request schema: {error}",
                entry.name
            )
        });

        if entry.id == REHEARSE_COMMAND_ID {
            let result = host.execute_domain_command_with_handler(entry.id, request, |_| {
                Ok::<Value, ()>(rehearsal_response_fixture())
            });
            assert!(
                result.is_ok(),
                "rehearsal must use the shared registered handler contract: {result:?}"
            );
            continue;
        }

        let result = host.execute_domain_command(entry.id, request);
        match result {
            Ok(response) => validate(&entry.response_schema, &response).unwrap_or_else(|error| {
                panic!(
                    "response for {} violates its response schema: {error}",
                    entry.name
                )
            }),
            Err(ExecutionError::UnknownCommand(command)) => {
                panic!("registered command {} was not recognized", command.0)
            }
            Err(ExecutionError::InvalidRequest(error)) => {
                panic!(
                    "valid registry fixture for {} was rejected: {error}",
                    entry.name
                )
            }
            Err(ExecutionError::InvalidResponse(error)) => {
                panic!(
                    "response for {} violates its response schema: {error}",
                    entry.name
                )
            }
            Err(ExecutionError::Handler(HostError::Validation { detail }))
                if detail.contains("not handled by the domain executor") =>
            {
                panic!(
                    "registered command {} has no executable handler",
                    entry.name
                )
            }
            Err(ExecutionError::Handler(_)) => {}
        }
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn production_reload_recomputes_a_bracket_after_all_brep_results_are_deleted() {
    let Some(worker) = OcctWorker::locate().ok() else {
        return;
    };
    let root = root("brep-free-bracket-reload");
    let host = Host::new();
    let committed = host
        .create_bracket(
            &root,
            BracketRequest::new(new_request_id(), 60.0, 30.0, 40.0, 3.0).with_feature_id("l-1"),
            &worker,
        )
        .expect("bracket commits");
    let identity_before = host.identity(&root).expect("identity loads");

    fs::remove_dir_all(root.join("brep")).expect("derived BREP directory removes");

    let reloaded = host
        .load_with_geometry_replay(&root)
        .expect("canonical state reloads without BREP");
    let identity_after = host.identity(&root).expect("reloaded identity loads");

    assert_eq!(
        reloaded.feature_graph_hash,
        committed.snapshot.feature_graph_hash
    );
    assert_eq!(reloaded.revision_hash, committed.snapshot.revision_hash);
    assert_eq!(identity_after, identity_before);
    assert!(root.join("brep/l-1.brep").is_file());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn derived_geometry_commands_reject_unproven_base_breps_before_worker_execution() {
    let root = root("unproven-base");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let boolean_pattern = host.execute_domain_command(
        BOOLEAN_PATTERN_COMMAND_ID,
        json!({
            "bundle_path": root.to_string_lossy(),
            "feature_id": "pattern",
            "base_feature_id": "missing-base",
            "origin": [0.0, 0.0, 0.0],
            "spacing": [1.0, 1.0],
            "columns": 1,
            "rows": 1,
            "diameter": 1.0
        }),
    );
    let hole = host.execute_domain_command(
        HOLE_COMMAND_ID,
        json!({
            "bundle_path": root.to_string_lossy(),
            "feature_id": "hole",
            "base_feature_id": "missing-base",
            "position": [0.0, 0.0, 0.0],
            "direction": [0.0, 0.0, 1.0],
            "diameter": 1.0
        }),
    );

    for result in [boolean_pattern, hole] {
        let Err(ExecutionError::Handler(HostError::Validation { detail })) = result else {
            panic!("unproven base must be rejected before worker execution: {result:?}");
        };
        assert!(detail.contains("base feature is missing"));
    }

    assert_eq!(Bundle::at(&root).open().expect("bundle opens").log.len(), 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn interactive_shared_command_semantics() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() => {
            panic!("OCCT interactive command semantics require the native worker: {error}");
        }
        Err(error) => {
            eprintln!("OCCT interactive command semantics skipped: {error}");
            return;
        }
    };
    let root = root("interactive-bracket-semantics");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity executes");
    let request = json!({
        "bundle_path": root.to_string_lossy(),
        "bracket_id": "l-bracket",
        "length": 60.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0,
        "expected_revision": initial["revision_hash"],
    });
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");

    let preview = host
        .preview_domain_command(BRACKET_COMMAND_ID, request.clone())
        .expect("bracket preview executes through the shared seam");
    assert_eq!(preview.source_revision, initial["revision_hash"]);
    assert!(!preview.preview_revision.is_empty());
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log_before);
    assert!(!root.join("brep/l-bracket.brep").exists());
    let identity_after_preview = host.identity(&root).expect("identity reads after preview");
    assert_eq!(
        identity_after_preview.revision_hash,
        initial["revision_hash"]
            .as_str()
            .expect("revision is a string")
    );
    assert_eq!(identity_after_preview.transaction_count, 0);

    let mut commit_request = request.clone();
    commit_request["preview_revision"] = json!(preview.preview_revision);
    let committed = host
        .execute_domain_command(BRACKET_COMMAND_ID, commit_request.clone())
        .expect("bracket commit executes through the shared seam");
    assert_eq!(committed["status"], "ok");
    assert_eq!(committed["feature_id"], "l-bracket");
    assert_ne!(committed["revision_hash"], initial["revision_hash"]);
    assert!(root.join("brep/l-bracket.brep").is_file());
    assert_eq!(
        host.identity(&root)
            .expect("identity reloads")
            .transaction_count,
        4,
        "bracket commit records its canonical feature and history entries"
    );

    let manifest_after_commit = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_after_commit = fs::read(root.join("transactions.log")).expect("log reads");
    let stale = host.execute_domain_command(BRACKET_COMMAND_ID, commit_request);
    assert!(matches!(
        stale,
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads after stale commit"),
        manifest_after_commit
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads after stale commit"),
        log_after_commit
    );
    assert_eq!(
        host.identity(&root)
            .expect("identity remains readable")
            .transaction_count,
        4
    );

    drop(worker);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn preview_is_read_only_and_commit_rechecks_the_draft_revision() {
    let Some(_worker) = OcctWorker::locate().ok() else {
        eprintln!("domain preview: OCCT worker unavailable");
        return;
    };
    let root = root("preview");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    host.extrude(
        &root,
        ExtrudeRequest::new(
            new_request_id(),
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
            2.0,
        )
        .with_output_path(root.join("seed-stage"), "seed.brep")
        .with_feature_id("seed"),
        &_worker,
    )
    .expect("canonical seed extrude commits");
    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity executes");
    let revision = initial["revision_hash"].as_str().unwrap().to_string();
    let before_manifest = fs::read(root.join("manifest.json")).unwrap();
    let before_log = fs::read(root.join("transactions.log")).unwrap();
    let before_brep = fs::read(root.join("brep/seed.brep")).unwrap();

    let preview = host
        .preview_domain_command(EXTRUDE_COMMAND_ID, extrude_request(&root, Some(&revision)))
        .expect("preview executes");
    assert_eq!(preview.source_revision, revision);
    assert_ne!(preview.preview_revision, preview.source_revision);
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(fs::read(root.join("brep/seed.brep")).unwrap(), before_brep);

    host.save(&root, "advance", "box")
        .expect("revision advances");
    let after_advance_manifest = fs::read(root.join("manifest.json")).unwrap();
    let after_advance_log = fs::read(root.join("transactions.log")).unwrap();
    let stale =
        host.execute_domain_command(EXTRUDE_COMMAND_ID, extrude_request(&root, Some(&revision)));
    assert!(matches!(
        stale,
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        after_advance_manifest
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).unwrap(),
        after_advance_log
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn transform_validation_rejects_before_canonical_mutation() {
    let root = root("canonical-transform-rejections");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("initial identity executes");
    let revision = initial["revision_hash"].as_str().unwrap();
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");

    let mut too_short = transform_request(&root, "revolve-short", None, revision);
    too_short["profile"] = json!([[0.0, 0.0], [1.0, 0.0]]);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, too_short),
        Err(ExecutionError::InvalidRequest(_))
    ));

    let mut zero_axis = transform_request(&root, "revolve-zero-axis", None, revision);
    zero_axis["axis_direction"] = json!([0.0, 0.0, 0.0]);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, zero_axis),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let mut non_positive_angle = transform_request(&root, "revolve-negative-angle", None, revision);
    non_positive_angle["angle"] = json!(-1.0);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, non_positive_angle),
        Err(ExecutionError::InvalidRequest(_))
    ));

    let invalid_feature = transform_request(&root, "../outside", None, revision);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, invalid_feature),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let missing_base =
        transform_request(&root, "mirror-missing-base", Some("missing-base"), revision);
    assert!(matches!(
        host.execute_domain_command(MIRROR_COMMAND_ID, missing_base),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let stale = transform_request(&root, "revolve-stale", None, &"0".repeat(64));
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, stale),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads after rejection"),
        manifest_before
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads after rejection"),
        log_before
    );
    let after = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity remains readable");
    assert_eq!(after["revision_hash"], initial["revision_hash"]);
    assert_eq!(after["transaction_count"], initial["transaction_count"]);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

#[test]
fn transform_commands_commit_canonical_intent_and_replay_after_brep_deletion() {
    let Some(_worker) = OcctWorker::locate().ok() else {
        eprintln!("canonical transform tracer: OCCT worker unavailable");
        assert_ne!(
            std::env::var("THREETERM_REQUIRE_OCCT").ok().as_deref(),
            Some("1"),
            "canonical transform tracer requires OCCT in the native integration environment"
        );
        return;
    };
    let root = root("canonical-transform-tracer");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();

    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("initial identity executes");
    let revolve_request = transform_request(
        &root,
        "revolve-preview",
        None,
        initial["revision_hash"].as_str().unwrap(),
    );
    let revolve_preview = host
        .preview_domain_command(REVOLVE_COMMAND_ID, revolve_request)
        .expect("revolve preview executes");
    assert_eq!(
        revolve_preview.source_revision,
        initial["revision_hash"].as_str().unwrap()
    );
    let revolve = host
        .execute_domain_command(
            REVOLVE_COMMAND_ID,
            transform_request(
                &root,
                "revolve-feature",
                None,
                initial["revision_hash"].as_str().unwrap(),
            ),
        )
        .expect("revolve executes");
    let mut revision = revolve["revision_hash"].as_str().unwrap().to_string();
    let mut commands = vec![
        (MIRROR_COMMAND_ID, "mirror-feature"),
        (LINEAR_PATTERN_COMMAND_ID, "linear-feature"),
        (CIRCULAR_PATTERN_COMMAND_ID, "circular-feature"),
    ];
    for (command, feature_id) in commands.drain(..) {
        let preview = host
            .preview_domain_command(
                command,
                transform_request(&root, feature_id, Some("revolve-feature"), &revision),
            )
            .expect("transform preview executes");
        assert_eq!(preview.source_revision, revision);
        let response = host
            .execute_domain_command(
                command,
                transform_request(&root, feature_id, Some("revolve-feature"), &revision),
            )
            .expect("transform executes");
        revision = response["revision_hash"].as_str().unwrap().to_string();
    }

    let loaded = Bundle::at(&root).open().expect("bundle opens");
    assert_eq!(loaded.log.entries().len(), 4);
    assert!(
        loaded
            .log
            .entries()
            .iter()
            .all(|entry| entry.intent.is_some())
    );
    assert_eq!(
        loaded.log.entries()[0]
            .intent
            .as_ref()
            .unwrap()
            .affected_semantic_ids(),
        &["revolve-feature".to_string()]
    );
    for (index, feature_id) in ["mirror-feature", "linear-feature", "circular-feature"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            loaded.log.entries()[index + 1]
                .intent
                .as_ref()
                .unwrap()
                .affected_semantic_ids(),
            &[feature_id.to_string(), "revolve-feature".to_string()]
        );
    }
    let original_hashes: Vec<_> = [
        "revolve-feature",
        "mirror-feature",
        "linear-feature",
        "circular-feature",
    ]
    .into_iter()
    .map(|feature_id| {
        fs::read(root.join("brep").join(format!("{feature_id}.brep"))).expect("BREP reads")
    })
    .collect();
    let formats = vec!["stl".to_string(), "step".to_string()];
    let export_before_dir = root.join("export-before");
    fs::create_dir_all(&export_before_dir).expect("export directory creates");
    let exported_before = host
        .export(
            &root,
            "circular-feature",
            &formats,
            &export_before_dir,
            0.1,
            false,
            false,
            &[],
        )
        .expect("pre-replay export succeeds");
    let export_artifacts: Vec<_> = exported_before
        .artifacts
        .iter()
        .map(|path| {
            assert!(
                !fs::metadata(path)
                    .expect("pre-replay export metadata")
                    .is_dir()
            );
            assert!(
                fs::metadata(path)
                    .expect("pre-replay export metadata")
                    .len()
                    > 0
            );
            path.file_name().unwrap().to_owned()
        })
        .collect();
    let viewport_before = host
        .presentation_viewport_scene()
        .expect("pre-replay viewport scene succeeds");
    fs::remove_dir_all(root.join("brep")).expect("derived BREPs delete");

    let before = Bundle::at(&root).open().expect("bundle reopens");
    let log_before = fs::read(root.join("transactions.log")).expect("log snapshot reads");
    let replayed = host
        .load_with_extrude_replay(&root)
        .expect("canonical transforms replay");
    assert_eq!(replayed.revision_hash, before.revision_hash_hex());
    assert_eq!(Bundle::at(&root).open().unwrap().log.entries().len(), 4);
    assert_eq!(
        fs::read(root.join("transactions.log")).unwrap(),
        log_before,
        "replay does not append a transaction"
    );
    for (feature_id, original) in [
        "revolve-feature",
        "mirror-feature",
        "linear-feature",
        "circular-feature",
    ]
    .into_iter()
    .zip(original_hashes)
    {
        assert_eq!(
            fs::read(root.join("brep").join(format!("{feature_id}.brep"))).unwrap(),
            original,
            "replayed geometry for {feature_id}"
        );
    }
    let export_after_dir = root.join("export-after");
    fs::create_dir_all(&export_after_dir).expect("second export directory creates");
    let exported_after = host
        .export(
            &root,
            "circular-feature",
            &formats,
            &export_after_dir,
            0.1,
            false,
            false,
            &[],
        )
        .expect("post-replay export succeeds");
    let export_after_artifacts: Vec<_> = exported_after
        .artifacts
        .iter()
        .map(|path| {
            assert!(
                !fs::metadata(path)
                    .expect("post-replay export metadata")
                    .is_dir()
            );
            assert!(
                fs::metadata(path)
                    .expect("post-replay export metadata")
                    .len()
                    > 0
            );
            path.file_name().unwrap().to_owned()
        })
        .collect();
    assert_eq!(
        export_after_artifacts, export_artifacts,
        "replayed export artifacts"
    );
    let viewport_after = host
        .presentation_viewport_scene()
        .expect("post-replay viewport scene succeeds");
    assert_eq!(viewport_after, viewport_before, "replayed viewport scene");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn fillet_domain_command_resolves_one_source_edge_before_commit() {
    let Some(worker) = OcctWorker::locate().ok() else {
        eprintln!("fillet edge resolution: OCCT worker unavailable");
        return;
    };
    let root = root("fillet-edge-resolution");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    host.extrude(
        &root,
        ExtrudeRequest::new(
            new_request_id(),
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
            2.0,
        )
        .with_output_path(root.join("seed-stage"), "seed.brep")
        .with_feature_id("seed"),
        &worker,
    )
    .expect("canonical seed extrude commits");
    let revision = Bundle::at(&root)
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let mut candidate = worker
        .inspect_edges(
            new_request_id(),
            root.join("brep/seed.brep"),
            "seed",
            &revision,
            serde_json::json!({
                "semantic_id": "requested-edge",
                "source_feature_id": "seed",
                "source_revision_id": revision,
                "source_edge_id": "source-edge",
                "role": "outer-perimeter",
                "midpoint": [0.0, 0.0, 0.0],
                "tangent": [1.0, 0.0, 0.0],
                "length": 1.0
            }),
        )
        .expect("edge inspection returns")
        .edge_candidates
        .into_iter()
        .next()
        .expect("source edge candidate exists");
    let evidence = serde_json::to_vec(&(candidate.midpoint, candidate.tangent, candidate.length))
        .expect("edge evidence serializes");
    candidate.semantic_id = format!("edge-{}", sha256_hex(&evidence));
    let response = host
        .execute_domain_command(
            threeterm_protocol::schema::FILLET_COMMAND_ID,
            json!({
                "bundle_path": root.to_string_lossy(),
                "expected_revision": revision,
                "feature_id": "fillet",
                "base_feature_id": "seed",
                "radius": 0.2,
                "selected_edge": {
                    "semantic_id": candidate.semantic_id,
                    "provenance": {
                        "source_feature_id": candidate.source_feature_id,
                        "source_revision_id": candidate.source_revision_id,
                        "source_edge_id": candidate.source_edge_id
                    },
                    "role": candidate.role,
                    "evidence": {
                        "midpoint": candidate.midpoint,
                        "tangent": candidate.tangent,
                        "length": candidate.length
                    }
                }
            }),
        )
        .expect("fillet commits");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["feature_id"], "fillet");
    assert!(root.join("brep/fillet.brep").is_file());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

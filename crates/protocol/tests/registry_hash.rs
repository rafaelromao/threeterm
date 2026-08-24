//! Asserts the registry's published schema hash is stable across builds.
//!
//! The hash is computed from the canonical JSON encoding of the registry
//! (sorted object keys via `serde_json::Value::Object`'s default
//! `BTreeMap` backing, no whitespace). The constant is recorded once via
//! the first-run commit pattern; if the registry changes unintentionally
//! the test fails with both the actual and the expected hash.

use threeterm_protocol::schema::{
    BOOLEAN_FUSE_COMMAND_ID, CHAMFER_COMMAND_ID, CIRCULAR_PATTERN_COMMAND_ID, DRAFT_COMMAND_ID,
    EXTRUDE_COMMAND_ID, FILLET_COMMAND_ID, HOLE_COMMAND_ID, LINEAR_PATTERN_COMMAND_ID,
    LIST_COMMAND_ID, LOAD_COMMAND_ID, LOFT_COMMAND_ID, MIRROR_COMMAND_ID, REVOLVE_COMMAND_ID,
    SAVE_COMMAND_ID, SHELL_COMMAND_ID, find, registry_hash,
};

#[test]
fn registry_hash_is_a_64_char_lowercase_hex_sha256() {
    let hash = registry_hash();

    assert_eq!(
        hash.len(),
        64,
        "SHA-256 hex digest is 64 chars; got {hash:?}"
    );
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "hash must be lowercase hex; got {hash:?}"
    );
}

#[test]
fn registry_hash_matches_the_published_constant() {
    assert_eq!(
        registry_hash(),
        "2e50479b98a104440ff514f1d4f306bb51812ef42c9d0eb0a9fc19adc74643b2",
        "registry_hash drifted from the published constant. If the registry \
         changed intentionally, update the constant in this test and rerun."
    );
}

#[test]
fn registry_hash_is_deterministic() {
    let first = registry_hash();
    let second = registry_hash();
    let third = registry_hash();

    assert_eq!(first, second);
    assert_eq!(second, third);
}

#[test]
fn registry_contains_versioned_save_and_load_contracts() {
    let save = find(SAVE_COMMAND_ID).expect("save is registered");
    assert_eq!(
        save.response_schema_version,
        "threeterm.command.save.response/1"
    );
    assert_eq!(
        save.request_schema["required"],
        serde_json::json!(["bundle_path", "feature_id", "kind"])
    );
    assert_eq!(save.request_schema["additionalProperties"], false);

    let load = find(LOAD_COMMAND_ID).expect("load is registered");
    assert_eq!(
        load.response_schema_version,
        "threeterm.command.load.response/2"
    );
    assert_eq!(
        load.request_schema["required"],
        serde_json::json!(["bundle_path"])
    );
    assert_eq!(load.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_extrude_and_boolean_fuse_contracts() {
    let extrude = find(EXTRUDE_COMMAND_ID).expect("extrude is registered");
    assert_eq!(
        extrude.response_schema_version,
        "threeterm.command.extrude.response/1"
    );
    assert_eq!(
        extrude.request_schema["required"],
        serde_json::json!(["bundle_path", "feature_id", "profile", "height"])
    );
    assert_eq!(extrude.request_schema["additionalProperties"], false);

    let fuse = find(BOOLEAN_FUSE_COMMAND_ID).expect("boolean-fuse is registered");
    assert_eq!(
        fuse.response_schema_version,
        "threeterm.command.boolean-fuse.response/1"
    );
    assert_eq!(
        fuse.request_schema["required"],
        serde_json::json!([
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "tool_feature_id"
        ])
    );
    assert_eq!(fuse.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_fillet_and_chamfer_contracts() {
    let fillet = find(FILLET_COMMAND_ID).expect("fillet is registered");
    assert_eq!(fillet.name, "fillet");
    assert_eq!(
        fillet.response_schema_version,
        "threeterm.command.fillet.response/1"
    );
    assert_eq!(
        fillet.request_schema["required"],
        serde_json::json!(["bundle_path", "feature_id", "base_feature_id", "radius"])
    );
    assert_eq!(fillet.request_schema["additionalProperties"], false);

    let chamfer = find(CHAMFER_COMMAND_ID).expect("chamfer is registered");
    assert_eq!(chamfer.name, "chamfer");
    assert_eq!(
        chamfer.response_schema_version,
        "threeterm.command.chamfer.response/1"
    );
    assert_eq!(
        chamfer.request_schema["required"],
        serde_json::json!(["bundle_path", "feature_id", "base_feature_id", "distance"])
    );
    assert_eq!(chamfer.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_hole_contract() {
    let hole = find(HOLE_COMMAND_ID).expect("hole is registered");
    assert_eq!(hole.id, HOLE_COMMAND_ID);
    assert_eq!(hole.name, "hole");
    assert_eq!(hole.schema_version, "threeterm.command.hole/1");
    assert_eq!(
        hole.request_schema_version,
        "threeterm.command.hole.request/1"
    );
    assert_eq!(
        hole.response_schema_version,
        "threeterm.command.hole.response/1"
    );
    assert_eq!(
        hole.request_schema["required"],
        serde_json::json!([
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "position",
            "direction",
            "diameter"
        ])
    );
    assert_eq!(hole.request_schema["additionalProperties"], false);
}

#[test]
fn registry_resolves_list_by_command_id() {
    let entry = find(LIST_COMMAND_ID).expect("`list` is the seeded entry");
    assert_eq!(entry.id, LIST_COMMAND_ID);
    assert_eq!(entry.name, "list");
}

#[test]
fn registry_contains_versioned_revolve_contract() {
    let revolve = find(REVOLVE_COMMAND_ID).expect("revolve is registered");
    assert_eq!(revolve.id, REVOLVE_COMMAND_ID);
    assert_eq!(revolve.name, "revolve");
    assert_eq!(revolve.schema_version, "threeterm.command.revolve/1");
    assert_eq!(
        revolve.request_schema_version,
        "threeterm.command.revolve.request/1"
    );
    assert_eq!(
        revolve.response_schema_version,
        "threeterm.command.revolve.response/1"
    );
    assert_eq!(
        revolve.request_schema["required"],
        serde_json::json!([
            "bundle_path",
            "feature_id",
            "profile",
            "axis_point",
            "axis_direction",
            "angle"
        ])
    );
    assert_eq!(revolve.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_mirror_contract() {
    let mirror = find(MIRROR_COMMAND_ID).expect("mirror is registered");
    assert_eq!(mirror.id, MIRROR_COMMAND_ID);
    assert_eq!(mirror.name, "mirror");
    assert_eq!(mirror.schema_version, "threeterm.command.mirror/1");
    assert_eq!(
        mirror.request_schema_version,
        "threeterm.command.mirror.request/1"
    );
    assert_eq!(
        mirror.response_schema_version,
        "threeterm.command.mirror.response/1"
    );
    assert_eq!(
        mirror.request_schema["required"],
        serde_json::json!([
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "plane_point",
            "plane_normal"
        ])
    );
    assert_eq!(mirror.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_linear_pattern_contract() {
    let pattern = find(LINEAR_PATTERN_COMMAND_ID).expect("linear-pattern is registered");
    assert_eq!(pattern.id, LINEAR_PATTERN_COMMAND_ID);
    assert_eq!(pattern.name, "linear-pattern");
    assert_eq!(pattern.schema_version, "threeterm.command.linear-pattern/1");
    assert_eq!(
        pattern.request_schema_version,
        "threeterm.command.linear-pattern.request/1"
    );
    assert_eq!(
        pattern.response_schema_version,
        "threeterm.command.linear-pattern.response/1"
    );
    assert_eq!(
        pattern.request_schema["required"],
        serde_json::json!([
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "direction",
            "count",
            "spacing"
        ])
    );
    assert_eq!(pattern.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_circular_pattern_contract() {
    let pattern = find(CIRCULAR_PATTERN_COMMAND_ID).expect("circular-pattern is registered");
    assert_eq!(pattern.id, CIRCULAR_PATTERN_COMMAND_ID);
    assert_eq!(pattern.name, "circular-pattern");
    assert_eq!(
        pattern.schema_version,
        "threeterm.command.circular-pattern/1"
    );
    assert_eq!(
        pattern.request_schema_version,
        "threeterm.command.circular-pattern.request/1"
    );
    assert_eq!(
        pattern.response_schema_version,
        "threeterm.command.circular-pattern.response/1"
    );
    assert_eq!(
        pattern.request_schema["required"],
        serde_json::json!([
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "axis_point",
            "axis_normal",
            "angle_step",
            "count"
        ])
    );
    assert_eq!(pattern.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_shell_contract() {
    let shell = find(SHELL_COMMAND_ID).expect("shell is registered");
    assert_eq!(shell.id, SHELL_COMMAND_ID);
    assert_eq!(shell.name, "shell");
    assert_eq!(shell.schema_version, "threeterm.command.shell/1");
    assert_eq!(
        shell.request_schema_version,
        "threeterm.command.shell.request/1"
    );
    assert_eq!(
        shell.response_schema_version,
        "threeterm.command.shell.response/1"
    );
    assert_eq!(
        shell.request_schema["required"],
        serde_json::json!(["bundle_path", "feature_id", "base_feature_id", "thickness"])
    );
    assert_eq!(shell.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_draft_contract() {
    let draft = find(DRAFT_COMMAND_ID).expect("draft is registered");
    assert_eq!(draft.id, DRAFT_COMMAND_ID);
    assert_eq!(draft.name, "draft");
    assert_eq!(draft.schema_version, "threeterm.command.draft/1");
    assert_eq!(
        draft.request_schema_version,
        "threeterm.command.draft.request/1"
    );
    assert_eq!(
        draft.response_schema_version,
        "threeterm.command.draft.response/1"
    );
    assert_eq!(
        draft.request_schema["required"],
        serde_json::json!([
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "angle",
            "pull_direction"
        ])
    );
    assert_eq!(draft.request_schema["additionalProperties"], false);
}

#[test]
fn registry_contains_versioned_loft_contract() {
    let loft = find(LOFT_COMMAND_ID).expect("loft is registered");
    assert_eq!(loft.id, LOFT_COMMAND_ID);
    assert_eq!(loft.name, "loft");
    assert_eq!(loft.schema_version, "threeterm.command.loft/1");
    assert_eq!(
        loft.request_schema_version,
        "threeterm.command.loft.request/1"
    );
    assert_eq!(
        loft.response_schema_version,
        "threeterm.command.loft.response/1"
    );
    assert_eq!(
        loft.request_schema["required"],
        serde_json::json!(["bundle_path", "feature_id", "profiles"])
    );
    assert_eq!(loft.request_schema["additionalProperties"], false);
}

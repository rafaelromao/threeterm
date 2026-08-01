//! Asserts the registry's published schema hash is stable across builds.
//!
//! The hash is computed from the canonical JSON encoding of the registry
//! (sorted object keys via `serde_json::Value::Object`'s default
//! `BTreeMap` backing, no whitespace). The constant is recorded once via
//! the first-run commit pattern; if the registry changes unintentionally
//! the test fails with both the actual and the expected hash.

use threeterm_protocol::schema::{
    BOOLEAN_FUSE_COMMAND_ID, EXTRUDE_COMMAND_ID, LIST_COMMAND_ID, LOAD_COMMAND_ID, SAVE_COMMAND_ID,
    find, registry_hash,
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
        "cc6748ba58d873b97b2e1914123a9566cab28db6eec20f47d96e73025b57923e",
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
        "threeterm.command.load.response/1"
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
fn registry_resolves_list_by_command_id() {
    let entry = find(LIST_COMMAND_ID).expect("`list` is the seeded entry");
    assert_eq!(entry.id, LIST_COMMAND_ID);
    assert_eq!(entry.name, "list");
}

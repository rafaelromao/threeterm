//! Asserts the versioned `validate` command contract: one selected
//! current solid resolved through the public dispatcher, with the
//! validity verdict bound to the selected feature plus its revision.

use threeterm_protocol::schema::{VALIDATE_COMMAND_ID, find, find_by_name};
use threeterm_protocol::schema_validator::validate;

#[test]
fn registry_contains_the_versioned_validate_contract() {
    let entry = find(VALIDATE_COMMAND_ID).expect("validate is registered");
    assert_eq!(entry.id, VALIDATE_COMMAND_ID);
    assert_eq!(entry.name, "validate");
    assert_eq!(entry.schema_version, "threeterm.command.validate/1");
    assert_eq!(
        entry.request_schema_version,
        "threeterm.command.validate.request/1"
    );
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.validate.response/2"
    );
    assert_eq!(
        entry.request_schema["required"],
        serde_json::json!(["bundle_path", "feature_id"])
    );
    assert_eq!(entry.request_schema["additionalProperties"], false);
    assert_eq!(
        find_by_name("validate").map(|entry| entry.id),
        Some(VALIDATE_COMMAND_ID)
    );
}

#[test]
fn validate_request_schema_accepts_a_selected_feature_and_rejects_the_rest() {
    let entry = find(VALIDATE_COMMAND_ID).expect("validate is registered");
    validate(
        &entry.request_schema,
        &serde_json::json!({
            "bundle_path": "/tmp/project",
            "feature_id": "l-bracket",
        }),
    )
    .expect("selected feature validates");

    for request in [
        serde_json::json!({
            "bundle_path": "/tmp/project",
            "feature_id": "",
        }),
        serde_json::json!({
            "bundle_path": "/tmp/project",
        }),
        serde_json::json!({
            "feature_id": "l-bracket",
        }),
        serde_json::json!({
            "bundle_path": "/tmp/project",
            "feature_id": "l-bracket",
            "formats": ["stl"],
        }),
    ] {
        assert!(
            validate(&entry.request_schema, &request).is_err(),
            "request must be rejected: {request}"
        );
    }
}

#[test]
fn validate_response_schema_binds_the_verdict_to_feature_plus_revision() {
    let entry = find(VALIDATE_COMMAND_ID).expect("validate is registered");
    validate(
        &entry.response_schema,
        &serde_json::json!({
            "status": "ok",
            "feature_id": "l-bracket",
            "revision_id": "history-revision-1",
            "feature_graph_hash": "0".repeat(64),
            "revision_hash": "f".repeat(64),
            "brep_path": "/tmp/project/brep/l-bracket.brep",
            "brep_sha256": "a".repeat(64),
            "valid": true,
            "schema_version": "threeterm.command.validate.response/2",
        }),
    )
    .expect("bound success verdict validates");

    for response in [
        serde_json::json!({
            "status": "ok",
            "feature_id": "l-bracket",
            "revision_id": "history-revision-1",
            "feature_graph_hash": "0".repeat(64),
            "revision_hash": "f".repeat(64),
            "valid": true,
        }),
        serde_json::json!({
            "status": "ok",
            "feature_id": "l-bracket",
            "revision_id": "history-revision-1",
            "feature_graph_hash": "not-a-hash",
            "revision_hash": "f".repeat(64),
            "valid": true,
            "schema_version": "threeterm.command.validate.response/2",
        }),
        serde_json::json!({
            "status": "ok",
            "feature_id": "l-bracket",
            "feature_graph_hash": "0".repeat(64),
            "revision_hash": "f".repeat(64),
            "valid": true,
            "schema_version": "threeterm.command.validate.response/2",
        }),
    ] {
        assert!(
            validate(&entry.response_schema, &response).is_err(),
            "response must be rejected: {response}"
        );
    }
}

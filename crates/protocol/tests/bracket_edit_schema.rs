use threeterm_protocol::schema::{
    BRACKET_EDIT_COMMAND_ID, BRACKET_EDIT_REQUEST_SCHEMA, BRACKET_EDIT_RESPONSE_SCHEMA, find,
};
use threeterm_protocol::schema_validator::validate;

#[test]
fn bracket_edit_lifecycle_is_registered_with_explicit_phases() {
    let entry = find(BRACKET_EDIT_COMMAND_ID).expect("bracket-edit is registered");
    assert_eq!(entry.name, "bracket-edit");
    assert_eq!(
        entry.request_schema["properties"]["phase"]["enum"],
        serde_json::json!(["open", "update", "preview", "commit", "discard"])
    );
    assert_eq!(entry.request_schema["additionalProperties"], false);
    assert_eq!(entry.response_schema, *BRACKET_EDIT_RESPONSE_SCHEMA);
}

#[test]
fn bracket_edit_request_requires_the_full_semantic_dimension_tuple() {
    let request = serde_json::json!({
        "phase": "preview",
        "bundle_path": "/tmp/project",
        "draft_id": "draft-1",
        "bracket_id": "l-bracket",
        "length": 100.0,
        "width": 60.0,
        "height": 40.0,
        "thickness": 5.0
    });
    validate(&BRACKET_EDIT_REQUEST_SCHEMA, &request).expect("valid edit request");

    let missing = serde_json::json!({
        "phase": "preview",
        "bundle_path": "/tmp/project",
        "draft_id": "draft-1",
        "bracket_id": "l-bracket",
        "length": 100.0,
        "width": 60.0,
        "height": 40.0
    });
    let error = validate(&BRACKET_EDIT_REQUEST_SCHEMA, &missing)
        .expect_err("thickness is required for every lifecycle request");
    assert!(error.contains("thickness"));
}

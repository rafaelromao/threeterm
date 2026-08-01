//! Asserts the command registry exposes a versioned `bracket` command whose
//! request and response JSON Schemas describe the L-bracket creation
//! contract that the CLI and MCP adapters share.

use serde_json::Value;
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, BRACKET_REQUEST_SCHEMA, BRACKET_RESPONSE_SCHEMA, find,
};
use threeterm_protocol::schema_validator::validate;

#[test]
fn bracket_command_is_registered() {
    let entry = find(BRACKET_COMMAND_ID).expect("`bracket` is registered");

    assert_eq!(entry.id, BRACKET_COMMAND_ID);
    assert_eq!(entry.name, "bracket");
    assert_eq!(entry.schema_version, "threeterm.command.bracket/1");
    assert_eq!(
        entry.request_schema_version,
        "threeterm.command.bracket.request/1"
    );
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.bracket.response/1"
    );
}

#[test]
fn bracket_request_schema_requires_positive_dimensions_and_paths() {
    let entry = find(BRACKET_COMMAND_ID).expect("`bracket` is registered");
    let schema: &Value = &entry.request_schema;

    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .expect("request schema declares required fields");
    let required_keys: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
    for key in [
        "bundle_path",
        "bracket_id",
        "length",
        "width",
        "height",
        "thickness",
    ] {
        assert!(
            required_keys.contains(&key),
            "request schema must require {key:?}; required={required_keys:?}"
        );
    }
    assert_eq!(
        schema.get("additionalProperties"),
        Some(&Value::Bool(false)),
        "request schema must reject additional properties"
    );
    assert_eq!(
        schema.get("properties"),
        Some(&BRACKET_REQUEST_SCHEMA["properties"]),
        "request schema properties must match the BRACKET_REQUEST_SCHEMA constant"
    );

    for dimension in ["length", "width", "height", "thickness"] {
        let property = &schema["properties"][dimension];
        assert_eq!(
            property.get("type"),
            Some(&Value::String("number".to_string())),
            "{dimension} must be a number"
        );
        assert_eq!(
            property.get("minimum"),
            Some(&serde_json::json!(0)),
            "{dimension} must have a minimum of 0"
        );
    }
}

#[test]
fn bracket_request_schema_validator_accepts_a_valid_arguments_object() {
    let arguments = serde_json::json!({
        "bundle_path": "/tmp/bracket.bundle",
        "bracket_id": "l-1",
        "length": 60.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0
    });

    validate(&BRACKET_REQUEST_SCHEMA, &arguments)
        .expect("valid arguments must satisfy the registered request schema");
}

#[test]
fn bracket_request_schema_validator_rejects_a_missing_required_field() {
    let arguments = serde_json::json!({
        "bundle_path": "/tmp/bracket.bundle",
        "bracket_id": "l-1",
        "length": 60.0,
        "width": 30.0,
        "height": 40.0
    });

    let error = validate(&BRACKET_REQUEST_SCHEMA, &arguments)
        .expect_err("missing thickness must be rejected");
    assert!(
        error.contains("thickness"),
        "validator message must name the missing field; got {error:?}"
    );
}

#[test]
fn bracket_request_schema_validator_rejects_a_non_numeric_dimension() {
    let arguments = serde_json::json!({
        "bundle_path": "/tmp/bracket.bundle",
        "bracket_id": "l-1",
        "length": "60.0",
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0
    });

    let error = validate(&BRACKET_REQUEST_SCHEMA, &arguments)
        .expect_err("non-numeric length must be rejected");
    assert!(
        error.contains("expected number"),
        "validator must report the type mismatch; got {error:?}"
    );
}

#[test]
fn bracket_response_schema_requires_three_identifier_keys() {
    let entry = find(BRACKET_COMMAND_ID).expect("`bracket` is registered");
    let schema: &Value = &entry.response_schema;

    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .expect("response schema declares required fields");
    let required_keys: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
    for key in ["feature_graph_hash", "revision_hash", "schema_version"] {
        assert!(
            required_keys.contains(&key),
            "response schema must require {key:?}; required={required_keys:?}"
        );
    }
    assert_eq!(
        schema.get("additionalProperties"),
        Some(&Value::Bool(false)),
        "response schema must reject additional properties"
    );
    assert_eq!(
        schema.get("properties"),
        Some(&BRACKET_RESPONSE_SCHEMA["properties"]),
        "response schema properties must match the BRACKET_RESPONSE_SCHEMA constant"
    );
}

#[test]
fn bracket_response_schema_validator_accepts_a_snapshot_response() {
    let response = serde_json::json!({
        "feature_graph_hash": "f".repeat(64),
        "revision_hash": "0".repeat(64),
        "schema_version": "threeterm.command.bracket.response/1"
    });

    validate(&BRACKET_RESPONSE_SCHEMA, &response)
        .expect("a well-formed snapshot response must satisfy the schema");
}
//! Asserts the bracket command is registered with its versioned request and
//! response JSON Schemas. The bracket command is the foundation for the MCP
//! L-bracket end-to-end slice (issue #242).

use serde_json::Value;
use threeterm_protocol::schema::{BRACKET_COMMAND_ID, find};

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
fn bracket_request_schema_requires_six_fields_with_positive_dimensions() {
    let entry = find(BRACKET_COMMAND_ID).expect("`bracket` is registered");
    let request = &entry.request_schema;

    assert_eq!(
        request["required"],
        Value::Array(vec![
            Value::from("bundle_path"),
            Value::from("bracket_id"),
            Value::from("length"),
            Value::from("width"),
            Value::from("height"),
            Value::from("thickness"),
        ]),
        "request schema requires all six L-bracket dimensions and identifiers"
    );
    assert_eq!(request["additionalProperties"], Value::Bool(false));

    let properties = request["properties"]
        .as_object()
        .expect("bracket request schema has properties");

    for required_string_field in ["bundle_path", "bracket_id"] {
        let field = &properties[required_string_field];
        assert_eq!(
            field["type"], "string",
            "{required_string_field} must be a string"
        );
        assert_eq!(
            field["minLength"], 1,
            "{required_string_field} must reject empty strings"
        );
    }

    for numeric_field in ["length", "width", "height", "thickness"] {
        let field = &properties[numeric_field];
        assert_eq!(field["type"], "number", "{numeric_field} must be a number");
        assert_eq!(
            field["minimum"], 0.0,
            "{numeric_field} must enforce a non-negative minimum (with > 0 enforced at the host)"
        );
    }
}

#[test]
fn bracket_response_schema_requires_snapshot_keys() {
    let entry = find(BRACKET_COMMAND_ID).expect("`bracket` is registered");
    let response = &entry.response_schema;

    assert_eq!(
        response["required"],
        Value::Array(vec![
            Value::from("feature_graph_hash"),
            Value::from("revision_hash"),
            Value::from("schema_version"),
        ]),
        "bracket response schema requires the three snapshot keys"
    );
    assert_eq!(response["additionalProperties"], Value::Bool(false));

    let properties = response["properties"]
        .as_object()
        .expect("bracket response schema has properties");
    for hash_field in ["feature_graph_hash", "revision_hash"] {
        assert_eq!(
            properties[hash_field]["type"], "string",
            "{hash_field} must be a string"
        );
        assert_eq!(
            properties[hash_field]["pattern"], "^[0-9a-f]{64}$",
            "{hash_field} must be a 64-char lowercase hex SHA-256"
        );
    }
    assert_eq!(properties["schema_version"]["type"], "string");
}

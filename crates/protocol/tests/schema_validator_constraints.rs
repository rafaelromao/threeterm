use serde_json::json;
use threeterm_protocol::schema_validator::validate;

#[test]
fn rejects_declared_string_constraints() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "minLength": 1 },
            "hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" }
        },
        "required": ["name", "hash"]
    });

    assert!(validate(&schema, &json!({ "name": "", "hash": "0".repeat(64) })).is_err());
    assert!(validate(&schema, &json!({ "name": "part", "hash": "x".repeat(64) })).is_err());
    assert!(validate(&schema, &json!({ "name": 1, "hash": "0".repeat(64) })).is_err());
}

#[test]
fn rejects_declared_array_and_number_constraints() {
    let schema = json!({
        "type": "object",
        "properties": {
            "points": { "type": "array", "minItems": 2, "maxItems": 3 },
            "height": { "type": "number", "exclusiveMinimum": 0, "maximum": 10 },
            "count": { "type": "integer", "minimum": 1 }
        },
        "required": ["points", "height", "count"]
    });

    assert!(validate(&schema, &json!({ "points": [1], "height": 1, "count": 1 })).is_err());
    assert!(
        validate(
            &schema,
            &json!({ "points": [1, 2, 3, 4], "height": 1, "count": 1 })
        )
        .is_err()
    );
    assert!(
        validate(
            &schema,
            &json!({ "points": [1, 2], "height": 0, "count": 1 })
        )
        .is_err()
    );
    assert!(
        validate(
            &schema,
            &json!({ "points": [1, 2], "height": 11, "count": 1 })
        )
        .is_err()
    );
    assert!(
        validate(
            &schema,
            &json!({ "points": [1, 2], "height": 1, "count": 0 })
        )
        .is_err()
    );
}

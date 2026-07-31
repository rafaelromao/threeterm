//! Hand-rolled structural validator for the versioned response schemas.
//!
//! The MVP is intentionally JSON-Schema-draft-light: the response schemas
//! in `schema.rs` are pure JSON documents that describe the shape of the
//! dispatcher's output. We validate the production output against the
//! registered schema by walking the schema and the value in lockstep so
//! the integration tests exercise the contract end-to-end without pulling
//! in a draft-2020-12 validator for the foundation slice.
//!
//! The validator supports the small subset the slice uses:
//! - `type` must be one of `object`, `array`, `string`, `number`, `boolean`, `null`, `integer`.
//! - `required` lists every key that must be present on an object.
//! - `properties` maps object keys to their inner schemas.
//! - `items` schemas array elements.
//! - `additionalProperties` may be `false` to reject extra keys.
//!
//! Anything outside this subset is treated as "no constraint" so the
//! validator can keep pace with the schema documents as the registry grows.

use serde_json::Value;

/// Validate `value` against the structural `schema`. Returns `Ok(())` on
/// success or `Err(reason)` with a human-readable description of the first
/// violation.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    match schema {
        Value::Bool(true) => Ok(()),
        Value::Bool(false) => Err("schema explicitly rejects every value".to_string()),
        Value::Object(_) => validate_object(schema, value),
        _ => Err("schema must be an object or `true`".to_string()),
    }
}

fn validate_object(schema: &Value, value: &Value) -> Result<(), String> {
    let schema_object = schema
        .as_object()
        .expect("schema is an object after the outer match");

    if let Some(expected_type) = schema_object.get("type") {
        let expected_type = expected_type
            .as_str()
            .ok_or_else(|| format!("schema `type` must be a string, got {expected_type}"))?;
        match expected_type {
            "object" => validate_object_type(schema_object, value)?,
            "array" => validate_array_type(schema_object, value)?,
            "string" => {
                if !value.is_string() {
                    return Err(format!("expected string, got {value}"));
                }
            }
            "number" => {
                if !value.is_number() {
                    return Err(format!("expected number, got {value}"));
                }
            }
            "integer" => {
                if !value.is_i64() && !value.is_u64() {
                    return Err(format!("expected integer, got {value}"));
                }
            }
            "boolean" => {
                if !value.is_boolean() {
                    return Err(format!("expected boolean, got {value}"));
                }
            }
            "null" => {
                if !value.is_null() {
                    return Err(format!("expected null, got {value}"));
                }
            }
            other => return Err(format!("unsupported schema type {other:?}")),
        }
    }

    Ok(())
}

fn validate_object_type(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("expected object, got {value}"))?;

    if let Some(required) = schema.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| format!("`required` must be an array, got {required}"))?;
        for key in required {
            let key = key
                .as_str()
                .ok_or_else(|| format!("required keys must be strings, got {key}"))?;
            if !object.contains_key(key) {
                return Err(format!("missing required key {key:?} on object {value}"));
            }
        }
    }

    let properties = if let Some(properties) = schema.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(|| format!("`properties` must be an object, got {properties}"))?;
        for (key, property_schema) in properties {
            if let Some(item) = object.get(key) {
                validate(property_schema, item)?;
            }
        }
        Some(properties)
    } else {
        None
    };

    if let Some(additional_properties) = schema.get("additionalProperties")
        && additional_properties == &Value::Bool(false)
    {
        for key in object.keys() {
            if !properties.map(|p| p.contains_key(key)).unwrap_or(false) {
                return Err(format!(
                    "additional key {key:?} is not allowed by `additionalProperties: false`"
                ));
            }
        }
    }

    Ok(())
}

fn validate_array_type(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
) -> Result<(), String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("expected array, got {value}"))?;

    if let Some(items) = schema.get("items") {
        for (index, item) in array.iter().enumerate() {
            validate(items, item).map_err(|err| format!("items[{index}]: {err}"))?;
        }
    }

    Ok(())
}

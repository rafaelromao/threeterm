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
//! - `minLength` enforces a minimum length on string fields.
//! - `minimum` enforces a lower bound on numeric and integer fields.
//! - `pattern` enforces a regex on string fields.
//! - `oneOf` requires exactly one matching alternative.
//! - `type` may be an array for nullable fields.
//! - `const` and boolean schemas constrain exact values and reject values
//!   when the schema is `false`.
//! - `enum` restricts a value to one of a fixed set of JSON values.
//!
//! Anything outside this subset is treated as "no constraint" so the
//! validator can keep pace with the schema documents as the registry grows.

use regex::Regex;
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

    if let Some(expected) = schema_object.get("const")
        && expected != value
    {
        return Err(format!("value {value} does not match const {expected}"));
    }
    if let Some(expected) = schema_object.get("enum") {
        let values = expected
            .as_array()
            .ok_or_else(|| format!("schema `enum` must be an array, got {expected}"))?;
        if !values.iter().any(|candidate| candidate == value) {
            return Err(format!("value {value} is not one of {expected}"));
        }
    }

    if schema_object.get("type").is_none()
        && (schema_object.contains_key("required") || schema_object.contains_key("properties"))
    {
        validate_object_type(schema_object, value)?;
    }

    if let Some(expected_type) = schema_object.get("type") {
        let expected_types = expected_type
            .as_str()
            .map(|value| vec![value])
            .or_else(|| {
                expected_type
                    .as_array()
                    .map(|values| values.iter().filter_map(Value::as_str).collect())
            })
            .ok_or_else(|| {
                format!("schema `type` must be a string or array, got {expected_type}")
            })?;
        if expected_types.len() > 1 {
            if !expected_types.iter().any(|kind| type_matches(kind, value)) {
                return Err(format!("value {value} does not match type {expected_type}"));
            }
            return Ok(());
        }
        let expected_type = expected_types[0];
        match expected_type {
            "object" => validate_object_type(schema_object, value)?,
            "array" => validate_array_type(schema_object, value)?,
            "string" => {
                validate_string(schema_object, value)?;
            }
            "number" => {
                if !value.is_number() {
                    return Err(format!("expected number, got {value}"));
                }
                validate_number(schema_object, value)?;
            }
            "integer" => {
                if !value.is_i64() && !value.is_u64() {
                    return Err(format!("expected integer, got {value}"));
                }
                validate_number(schema_object, value)?;
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

    if let Some(one_of) = schema_object.get("oneOf") {
        let alternatives = one_of
            .as_array()
            .ok_or_else(|| format!("schema `oneOf` must be an array, got {one_of}"))?;
        let matches = alternatives
            .iter()
            .filter(|alternative| validate(alternative, value).is_ok())
            .count();
        if matches != 1 {
            return Err(format!(
                "value {value} matches {matches} `oneOf` alternatives"
            ));
        }
    }

    Ok(())
}

fn type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        _ => false,
    }
}

fn validate_string(schema: &serde_json::Map<String, Value>, value: &Value) -> Result<(), String> {
    let string = value
        .as_str()
        .ok_or_else(|| format!("expected string, got {value}"))?;
    if let Some(min_length) = schema.get("minLength").and_then(Value::as_u64)
        && string.chars().count() < min_length as usize
    {
        return Err(format!("string must have at least {min_length} characters"));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        let regex =
            Regex::new(pattern).map_err(|error| format!("invalid pattern {pattern:?}: {error}"))?;
        if !regex.is_match(string) {
            return Err(format!(
                "string {string:?} does not match pattern {pattern:?}"
            ));
        }
    }
    Ok(())
}

fn validate_number(schema: &serde_json::Map<String, Value>, value: &Value) -> Result<(), String> {
    let number = value.as_f64().expect("number checked by caller");
    for (keyword, valid) in [
        (
            "minimum",
            schema
                .get("minimum")
                .and_then(Value::as_f64)
                .is_none_or(|minimum| number >= minimum),
        ),
        (
            "maximum",
            schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_none_or(|maximum| number <= maximum),
        ),
        (
            "exclusiveMinimum",
            schema
                .get("exclusiveMinimum")
                .and_then(Value::as_f64)
                .is_none_or(|minimum| number > minimum),
        ),
    ] {
        if !valid {
            return Err(format!("number {number} violates {keyword}"));
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
                validate(property_schema, item)
                    .map_err(|error| format!("property {key:?}: {error}"))?;
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

    if let Some(min_items) = schema.get("minItems").and_then(Value::as_u64)
        && array.len() < min_items as usize
    {
        return Err(format!("array must have at least {min_items} items"));
    }
    if let Some(max_items) = schema.get("maxItems").and_then(Value::as_u64)
        && array.len() > max_items as usize
    {
        return Err(format!("array must have at most {max_items} items"));
    }

    if let Some(items) = schema.get("items") {
        for (index, item) in array.iter().enumerate() {
            validate(items, item).map_err(|err| format!("items[{index}]: {err}"))?;
        }
    }

    Ok(())
}

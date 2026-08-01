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
                if let Some(min_length) = schema_object.get("minLength") {
                    let min_length = min_length.as_u64().ok_or_else(|| {
                        format!("`minLength` must be a non-negative integer, got {min_length}")
                    })?;
                    let actual = value
                        .as_str()
                        .expect("value is a string after the type check")
                        .chars()
                        .count();
                    if actual < min_length as usize {
                        return Err(format!(
                            "string is shorter than the schema's `minLength`: {actual} < {min_length}"
                        ));
                    }
                }
                if let Some(pattern) = schema_object.get("pattern") {
                    let pattern = pattern
                        .as_str()
                        .ok_or_else(|| format!("`pattern` must be a string, got {pattern}"))?;
                    let actual = value
                        .as_str()
                        .expect("value is a string after the type check");
                    if !matches_pattern(pattern, actual) {
                        return Err(format!(
                            "string does not match the schema's `pattern`: {pattern:?}"
                        ));
                    }
                }
            }
            "number" => {
                if !value.is_number() {
                    return Err(format!("expected number, got {value}"));
                }
                if let Some(minimum) = schema_object.get("minimum") {
                    let minimum = minimum
                        .as_f64()
                        .ok_or_else(|| format!("`minimum` must be a number, got {minimum}"))?;
                    let actual = value
                        .as_f64()
                        .expect("value is a number after the type check");
                    if actual < minimum {
                        return Err(format!(
                            "number is below the schema's `minimum`: {actual} < {minimum}"
                        ));
                    }
                }
            }
            "integer" => {
                if !value.is_i64() && !value.is_u64() {
                    return Err(format!("expected integer, got {value}"));
                }
                if let Some(minimum) = schema_object.get("minimum") {
                    let minimum = minimum
                        .as_f64()
                        .ok_or_else(|| format!("`minimum` must be a number, got {minimum}"))?;
                    let actual = if value.is_i64() {
                        value.as_i64().expect("value is i64") as f64
                    } else {
                        value.as_u64().expect("value is u64") as f64
                    };
                    if actual < minimum {
                        return Err(format!(
                            "integer is below the schema's `minimum`: {actual} < {minimum}"
                        ));
                    }
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

/// Minimal pattern matcher: anchored `^[0-9a-f]{64}$`-style patterns are
/// the only ones declared in the registered schemas. The implementation
/// translates `^`/`$` anchors and `{n}`/`{n,m}` quantifiers into a small
/// regex without pulling in the `regex` crate.
fn matches_pattern(pattern: &str, value: &str) -> bool {
    let anchored_left = pattern.starts_with('^');
    let anchored_right = pattern.ends_with('$');
    let body = if anchored_left {
        if anchored_right {
            &pattern[1..pattern.len() - 1]
        } else {
            &pattern[1..]
        }
    } else if anchored_right {
        &pattern[..pattern.len() - 1]
    } else {
        pattern
    };

    anchored_left && anchored_right && char_class_matches(body, value).unwrap_or(false)
}

fn char_class_matches(body: &str, value: &str) -> Option<bool> {
    if let Some(stripped) = body.strip_prefix('[') {
        let end = stripped.find(']')?;
        let chars_in_class = &stripped[..end];
        let mut rest = &stripped[end + 1..];
        let (min, max) = parse_quantifier(&mut rest)?;
        let chars: Vec<char> = value.chars().collect();
        if chars.len() < min || chars.len() > max {
            return Some(false);
        }
        Some(chars.iter().all(|ch| char_in_class(chars_in_class, *ch)))
    } else {
        None
    }
}

fn parse_quantifier(s: &mut &str) -> Option<(usize, usize)> {
    if !s.starts_with('{') {
        return Some((1, usize::MAX));
    }
    let end = s.find('}')?;
    let inner = &s[1..end];
    *s = &s[end + 1..];
    if let Some((lo, hi)) = inner.split_once(',') {
        let lo: usize = lo.parse().ok()?;
        let hi: usize = hi.parse().ok()?;
        Some((lo, hi))
    } else {
        let n: usize = inner.parse().ok()?;
        Some((n, n))
    }
}

fn char_in_class(class: &str, ch: char) -> bool {
    let mut chars = class.chars().peekable();
    let mut negated = false;
    if chars.peek() == Some(&'^') {
        negated = true;
        chars.next();
    }
    let mut matched = false;
    let mut iter = chars.peekable();
    while let Some(c) = iter.next() {
        if c == '\\' {
            iter.next();
            continue;
        }
        if iter.peek() == Some(&'-') {
            iter.next();
            if let Some(end) = iter.next()
                && ch >= c
                && ch <= end
            {
                matched = true;
                break;
            }
        } else if c == ch {
            matched = true;
            break;
        }
    }
    matched ^ negated
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

    if let Some(items) = schema.get("items") {
        for (index, item) in array.iter().enumerate() {
            validate(items, item).map_err(|err| format!("items[{index}]: {err}"))?;
        }
    }

    Ok(())
}

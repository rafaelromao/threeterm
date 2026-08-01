//! Asserts the schema validator enforces `minLength` on string fields and
//! `minimum` on numeric/integer fields. The validator's existing subset
//! (type/required/properties/items/additionalProperties) silently accepts
//! values that violate the string `minLength` and numeric `minimum`
//! constraints declared in the registered schemas. This slice (#242)
//! extends the validator so every schema field declared in the registry
//! is enforced end-to-end.

use serde_json::json;
use threeterm_protocol::schema::BRACKET_REQUEST_SCHEMA;
use threeterm_protocol::schema_validator::validate;

#[test]
fn validator_rejects_an_empty_bracket_id_violating_min_length() {
    let arguments = json!({
        "bundle_path": "/tmp/bracket.bundle",
        "bracket_id": "",
        "length": 60.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0
    });

    let error = validate(&BRACKET_REQUEST_SCHEMA, &arguments)
        .expect_err("empty bracket_id violates the schema's minLength: 1");
    assert!(
        error.contains("bracket_id"),
        "validator must name the offending field; got {error:?}"
    );
}

#[test]
fn validator_rejects_a_negative_length_violating_minimum() {
    let arguments = json!({
        "bundle_path": "/tmp/bracket.bundle",
        "bracket_id": "l-1",
        "length": -1.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0
    });

    let error = validate(&BRACKET_REQUEST_SCHEMA, &arguments)
        .expect_err("negative length violates the schema's minimum: 0");
    assert!(
        error.contains("length"),
        "validator must name the offending field; got {error:?}"
    );
}

#[test]
fn validator_accepts_a_strictly_positive_length_above_the_schema_minimum() {
    let arguments = json!({
        "bundle_path": "/tmp/bracket.bundle",
        "bracket_id": "l-1",
        "length": 0.5,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0
    });

    validate(&BRACKET_REQUEST_SCHEMA, &arguments)
        .expect("a strictly positive length satisfies the schema's exclusiveMinimum: 0 boundary");
}

#[test]
fn validator_rejects_a_zero_length_violating_exclusive_minimum() {
    let arguments = json!({
        "bundle_path": "/tmp/bracket.bundle",
        "bracket_id": "l-1",
        "length": 0.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0
    });

    let error = validate(&BRACKET_REQUEST_SCHEMA, &arguments)
        .expect_err("zero length violates the schema's exclusiveMinimum: 0");
    assert!(
        error.contains("length"),
        "validator must name the offending field; got {error:?}"
    );
}

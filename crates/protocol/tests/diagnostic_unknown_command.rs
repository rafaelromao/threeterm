//! Asserts the diagnostic taxonomy exposes a single `unknown_command` code
//! with the JSON shape consumed by the CLI adapter.

use serde_json::Value;
use threeterm_protocol::diagnostic::{Diagnostic, DiagnosticCode};
use threeterm_protocol::schema_version;

#[test]
fn unknown_command_diagnostic_serializes_to_expected_json_shape() {
    let diag = Diagnostic::unknown_command("bogus");

    let serialized = serde_json::to_value(&diag).expect("diagnostic serializes");
    let object = serialized
        .as_object()
        .expect("diagnostic serializes to a JSON object");

    for key in ["code", "arg", "schema_version"] {
        assert!(
            object.contains_key(key),
            "diagnostic JSON is missing required field {key:?}; got keys {:?}",
            object.keys().collect::<Vec<_>>()
        );
    }

    assert_eq!(object["code"], Value::from("unknown_command"));
    assert_eq!(object["arg"], Value::from("bogus"));
    assert_eq!(
        object["schema_version"],
        Value::from(schema_version()),
        "the diagnostic header schema_version must match the protocol crate's schema_version()"
    );
}

#[test]
fn unknown_command_diagnostic_reports_the_unknown_command_code() {
    let diag = Diagnostic::unknown_command("bogus");
    assert_eq!(diag.code, DiagnosticCode::UnknownCommand);
}

#[test]
fn unknown_command_diagnostic_preserves_the_offending_arg() {
    let diag = Diagnostic::unknown_command("--machine");
    assert_eq!(diag.arg, "--machine");
}

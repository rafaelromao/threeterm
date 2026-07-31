//! Asserts the structured shape of the `integrity_failure` diagnostic.
//!
//! The CLI uses this envelope on stderr for every persistence-layer
//! failure (log digest mismatch, broken chain link, missing files,
//! unsupported schema generation). The `arg` field is the stable
//! lowercase detail string the CLI uses to switch on the failure mode.

use serde_json::Value;
use threeterm_protocol::diagnostic::{Diagnostic, DiagnosticCode};

#[test]
fn integrity_failure_diagnostic_reports_the_integrity_failure_code() {
    let diag = Diagnostic::integrity_failure("log_digest_mismatch");

    assert_eq!(diag.code, DiagnosticCode::IntegrityFailure);
    assert_eq!(diag.arg, "log_digest_mismatch");
    assert_eq!(diag.schema_version, threeterm_protocol::schema_version());
}

#[test]
fn integrity_failure_diagnostic_serializes_to_expected_json_shape() {
    let diag = Diagnostic::integrity_failure("log_missing");
    let value = serde_json::to_value(&diag).expect("serializes");

    assert_eq!(value["code"], "integrity_failure");
    assert_eq!(value["arg"], "log_missing");
    assert_eq!(
        value["schema_version"],
        Value::from(threeterm_protocol::schema_version())
    );
}

#[test]
fn stable_detail_strings_carry_the_failure_mode() {
    let cases = [
        "log_digest_mismatch",
        "log_broken_link",
        "log_missing",
        "manifest_missing",
        "schema_generation_unsupported",
        "bundle_path_missing",
        "bundle_path_not_directory",
        "missing_feature_id",
        "missing_kind",
    ];
    for detail in cases {
        let diag = Diagnostic::integrity_failure(detail);
        assert_eq!(diag.code, DiagnosticCode::IntegrityFailure);
        assert_eq!(diag.arg, detail);
    }
}

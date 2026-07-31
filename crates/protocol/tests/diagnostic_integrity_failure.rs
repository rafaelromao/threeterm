use serde_json::json;
use threeterm_protocol::diagnostic::Diagnostic;

#[test]
fn integrity_failure_has_stable_machine_envelope() {
    let value = serde_json::to_value(Diagnostic::integrity_failure("log_digest_mismatch"))
        .expect("diagnostic serializes");
    assert_eq!(
        value,
        json!({
            "code": "integrity_failure",
            "arg": "log_digest_mismatch",
            "schema_version": "threeterm.protocol/1"
        })
    );
}

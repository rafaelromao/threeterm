use serde_json::json;
use threeterm_protocol::diagnostic::Diagnostic;

#[test]
fn brep_invalid_has_stable_machine_envelope() {
    let value = serde_json::to_value(Diagnostic::brep_invalid("BRepCheck_Analyzer failed"))
        .expect("diagnostic serializes");
    assert_eq!(
        value,
        json!({
            "code": "brep_invalid",
            "arg": "BRepCheck_Analyzer failed",
            "schema_version": "threeterm.protocol/1"
        })
    );
}

#[test]
fn worker_failure_has_stable_machine_envelope() {
    let value = serde_json::to_value(Diagnostic::worker_failure("worker exited with code 7"))
        .expect("diagnostic serializes");
    assert_eq!(
        value,
        json!({
            "code": "worker_failure",
            "arg": "worker exited with code 7",
            "schema_version": "threeterm.protocol/1"
        })
    );
}

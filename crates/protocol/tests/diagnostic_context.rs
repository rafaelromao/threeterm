use serde_json::json;
use threeterm_protocol::diagnostic::Diagnostic;

#[test]
fn diagnostic_context_preserves_first_seen_affected_id_order() {
    let diagnostic = Diagnostic::brep_invalid("invalid edit").with_context(
        ["invalid-cut", "base", "invalid-cut"],
        "correct_geometry_or_restore_revision",
    );
    let value = serde_json::to_value(diagnostic).expect("diagnostic serializes");

    assert_eq!(value["affected_ids"], json!(["invalid-cut", "base"]));
    assert_eq!(value["recovery"], "correct_geometry_or_restore_revision");
}

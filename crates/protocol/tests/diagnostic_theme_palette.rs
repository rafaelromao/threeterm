use serde_json::Value;
use threeterm_protocol::diagnostic::{Diagnostic, DiagnosticCode};
use threeterm_protocol::schema_version;

#[test]
fn theme_palette_failure_serializes_source_reason_and_recovery() {
    let diagnostic = Diagnostic::theme_palette_invalid(
        "not-a-palette",
        "cli",
        "unknown_palette",
        "use --palette with a registered palette",
    );
    let value = serde_json::to_value(&diagnostic).expect("diagnostic serializes");

    assert_eq!(value["code"], "theme_palette_invalid");
    assert_eq!(value["arg"], "not-a-palette");
    assert_eq!(value["source"], "cli");
    assert_eq!(value["detail"], "unknown_palette");
    assert_eq!(value["recovery"], "use --palette with a registered palette");
    assert_eq!(value["schema_version"], schema_version());
}

#[test]
fn theme_palette_failure_uses_the_structured_diagnostic_code() {
    let diagnostic =
        Diagnostic::theme_palette_invalid("", "environment", "missing_value", "recover");

    assert_eq!(diagnostic.code, DiagnosticCode::ThemePaletteInvalid);
    let object = serde_json::to_value(diagnostic)
        .expect("diagnostic serializes")
        .as_object()
        .cloned()
        .expect("diagnostic is an object");
    assert!(object.contains_key("source"));
    assert!(object.contains_key("detail"));
    assert!(object.contains_key("recovery"));
    assert!(Value::from(object).is_object());
}

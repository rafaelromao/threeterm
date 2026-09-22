//! The export response is bound to validated source geometry and output facts.

use threeterm_protocol::schema::{EXPORT_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

#[test]
fn export_response_schema_carries_validation_settings_and_output_identity() {
    let entry = find(EXPORT_COMMAND_ID).expect("export is registered");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.export.response/3"
    );
    validate(
        &entry.response_schema,
        &serde_json::json!({
            "status": "ok",
            "feature_id": "l-bracket",
            "artifacts": ["/tmp/export/l-bracket.stl"],
            "source_revision_id": "f".repeat(64),
            "validation": {
                "feature_id": "l-bracket",
                "revision_id": "history-revision-1",
                "feature_graph_hash": "0".repeat(64),
                "revision_hash": "f".repeat(64),
                "brep_path": "/tmp/project/brep/l-bracket.brep",
                "brep_sha256": "a".repeat(64),
                "valid": true,
            },
            "tessellation": {
                "units": "millimetres",
                "deflection": 0.1,
                "relative": false,
                "angular_deflection_radians": 0.5,
            },
            "derived_artifacts": [{
                "request_id": "export",
                "source_revision_id": "f".repeat(64),
                "operation": "export",
                "feature_id": "l-bracket",
                "artifact_kind": "stl",
                "artifact_name": "l-bracket.stl",
                "output_path": "/tmp/export/l-bracket.stl",
                "byte_count": 128,
                "sha256": "b".repeat(64),
            }],
            "accepted_stale_last_valid_geometry": false,
            "stale_last_valid_geometry": {
                "feature_id": "l-bracket",
                "active_revision": "history-revision-1",
                "stale_features": [],
            },
            "schema_version": "threeterm.command.export.response/3",
        }),
    )
    .expect("validated export response satisfies the registered schema");
}

#[test]
fn export_response_schema_rejects_missing_validation_binding() {
    let entry = find(EXPORT_COMMAND_ID).expect("export is registered");
    let response = serde_json::json!({
        "status": "ok",
        "feature_id": "l-bracket",
        "artifacts": [],
        "accepted_stale_last_valid_geometry": false,
        "stale_last_valid_geometry": {
            "feature_id": "l-bracket",
            "active_revision": "history-revision-1",
            "stale_features": [],
        },
        "schema_version": "threeterm.command.export.response/3",
    });
    assert!(validate(&entry.response_schema, &response).is_err());
}

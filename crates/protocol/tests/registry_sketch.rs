use serde_json::json;
use threeterm_protocol::schema::{
    SKETCH_SOLVE_COMMAND_ID, SKETCH_SOLVE_RESPONSE_SCHEMA_VERSION, find,
};
use threeterm_protocol::schema_validator::validate;

#[test]
fn sketch_solve_command_exposes_versioned_entity_and_status_contract() {
    let command = find(SKETCH_SOLVE_COMMAND_ID).expect("sketch solve is registered");
    assert_eq!(command.name, "sketch-solve");
    assert_eq!(
        command.response_schema_version,
        SKETCH_SOLVE_RESPONSE_SCHEMA_VERSION
    );
    assert_eq!(
        command.request_schema["properties"]["entities"]["type"],
        "array"
    );
    assert_eq!(
        command.response_schema["properties"]["dof"]["type"],
        "integer"
    );
    assert!(
        command.response_schema["properties"]["status"]["enum"]
            .as_array()
            .is_some_and(|statuses| statuses.iter().any(|status| status == "solved"))
    );
}

#[test]
fn sketch_solve_response_schema_accepts_a_normalized_success() {
    let command = find(SKETCH_SOLVE_COMMAND_ID).expect("sketch solve is registered");
    validate(
        &command.response_schema,
        &json!({
            "schema_version": SKETCH_SOLVE_RESPONSE_SCHEMA_VERSION,
            "request_id": "req-1",
            "operation": "sketch_solve",
            "feature_id": "rectangle",
            "status": "solved",
            "dof": 0,
            "entity_ids": ["p0", "p1"],
            "related_constraint_ids": [],
            "diagnostics": [],
            "solved_coordinates": [
                {"entity_id": "p0", "x": 0.0, "y": 0.0},
                {"entity_id": "p1", "x": 10.0, "y": 0.0}
            ]
        }),
    )
    .expect("normalized success matches the response schema");
}

#[test]
fn sketch_solve_request_accepts_semantic_face_support_and_placement() {
    let command = find(SKETCH_SOLVE_COMMAND_ID).expect("sketch solve is registered");
    validate(
        &command.request_schema,
        &json!({
            "bundle_path": "/tmp/model",
            "feature_id": "sketch-1",
            "source_revision": "revision-1",
            "support": {
                "semantic_id": "bracket/vertical-face",
                "role": "sketch-support",
                "provenance": {
                    "source_feature_id": "bracket",
                    "source_revision_id": "revision-1",
                    "source_face_id": "bracket/vertical-face"
                },
                "evidence": {
                    "origin": [0.0, 0.0, 0.0],
                    "normal": [0.0, 1.0, 0.0],
                    "x_axis": [1.0, 0.0, 0.0],
                    "y_axis": [0.0, 0.0, -1.0]
                }
            },
            "placement": {
                "origin": [0.0, 0.0, 0.0],
                "normal": [0.0, 1.0, 0.0],
                "x_axis": [1.0, 0.0, 0.0],
                "y_axis": [0.0, 0.0, -1.0]
            },
            "entities": [{"kind": "point", "id": "p0", "x": 0.0, "y": 0.0}],
            "constraints": []
        }),
    )
    .expect("attached sketch request matches the schema");
}

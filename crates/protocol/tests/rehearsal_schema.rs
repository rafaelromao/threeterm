use threeterm_protocol::schema::{
    REHEARSE_COMMAND_ID, REHEARSE_FAILURE_DIAGNOSTIC_SCHEMA, REHEARSE_REQUEST_SCHEMA,
    REHEARSE_RESPONSE_SCHEMA, find,
};
use threeterm_protocol::schema_validator::validate;

#[test]
fn rehearsal_command_registers_a_versioned_request_and_response() {
    let entry = find(REHEARSE_COMMAND_ID).expect("rehearse is registered");
    assert_eq!(entry.name, "rehearse");
    assert_eq!(entry.schema_version, "threeterm.command.rehearse/1");
    assert_eq!(entry.request_schema, *REHEARSE_REQUEST_SCHEMA);
    assert_eq!(entry.response_schema, *REHEARSE_RESPONSE_SCHEMA);
}

#[test]
fn rehearsal_failure_has_a_structured_canonical_state_diagnostic() {
    let diagnostic = serde_json::json!({
        "schema_version": "threeterm.protocol/1",
        "code": "rehearsal_failure",
        "stage": "export",
        "detail": { "message": "export failed" },
        "current_revision": serde_json::Value::Null,
        "recovery": "retry with a fresh output root"
    });
    validate(&REHEARSE_FAILURE_DIAGNOSTIC_SCHEMA, &diagnostic)
        .expect("failure diagnostic validates");
}

#[test]
fn rehearsal_response_describes_one_release_candidate_and_artifact_hashes() {
    let request = serde_json::json!({
        "output_dir": "/tmp/rehearsal",
        "release_candidate": "rc-1"
    });
    validate(&REHEARSE_REQUEST_SCHEMA, &request).expect("request validates");

    let response = serde_json::json!({
        "schema_version": "threeterm.command.rehearse.response/1",
        "release_candidate": "rc-1",
        "fixture": "l-bracket",
        "run_count": 1,
        "sample_policy": "nearest-rank",
        "promoted": false,
        "project_path": "project",
        "export_path": "export",
        "catalog_path": "sha256-manifest.json",
        "timings": [{
            "class": "project_create",
            "unit": "ms",
            "sample_count": 1,
            "samples_ms": [1.0],
            "p50_ms": 1.0,
            "p95_ms": 1.0,
            "p99_ms": 1.0
        }],
        "artifacts": [{
            "relative_path": "project/manifest.json",
            "bytes": 1,
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }]
    });
    validate(&REHEARSE_RESPONSE_SCHEMA, &response).expect("response validates");
}

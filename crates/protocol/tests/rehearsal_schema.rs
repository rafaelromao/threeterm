use threeterm_protocol::schema::{
    REHEARSE_COMMAND_ID, REHEARSE_FAILURE_DIAGNOSTIC_SCHEMA, REHEARSE_REQUEST_SCHEMA,
    REHEARSE_RESPONSE_SCHEMA, REHEARSE_RUN_RESPONSE_SCHEMA, find,
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
fn rehearsal_response_describes_two_release_candidates_and_artifact_hashes() {
    let request = serde_json::json!({
        "output_dir": "/tmp/rehearsal",
        "release_candidate": "rc-1"
    });
    validate(&REHEARSE_REQUEST_SCHEMA, &request).expect("request validates");

    let mut response = serde_json::json!({
        "schema_version": "threeterm.command.rehearse.response/2",
        "release_candidates": ["rc-1", "rc-2"],
        "fixture": "l-bracket",
        "run_count": 2,
        "sample_policy": "nearest-rank",
        "promoted": false,
        "runs": [{
            "release_candidate": "rc-1",
            "project_path": "run-1/project",
            "export_path": "run-1/export",
            "catalog_path": "run-1/sha256-manifest.json",
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
                "relative_path": "run-1/project/manifest.json",
                "bytes": 1,
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }]
        }, {
            "release_candidate": "rc-2",
            "project_path": "run-2/project",
            "export_path": "run-2/export",
            "catalog_path": "run-2/sha256-manifest.json",
            "timings": [{
                "class": "project_create",
                "unit": "ms",
                "sample_count": 1,
                "samples_ms": [2.0],
                "p50_ms": 2.0,
                "p95_ms": 2.0,
                "p99_ms": 2.0
            }],
            "artifacts": [{
                "relative_path": "run-2/project/manifest.json",
                "bytes": 1,
                "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }]
        }],
        "comparisons": [{
            "class": "project_create",
            "run_1": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0},
            "run_2": {"p50_ms": 2.0, "p95_ms": 2.0, "p99_ms": 2.0},
            "same_order_of_magnitude": true
        }]
    });
    for index in 1..9 {
        response["comparisons"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "class": format!("class-{index}"),
                "run_1": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0},
                "run_2": {"p50_ms": 2.0, "p95_ms": 2.0, "p99_ms": 2.0},
                "same_order_of_magnitude": true
            }));
    }
    validate(&REHEARSE_RESPONSE_SCHEMA, &response).expect("response validates");
    validate(&REHEARSE_RUN_RESPONSE_SCHEMA, &response["runs"][0])
        .expect("per-run response validates");
}

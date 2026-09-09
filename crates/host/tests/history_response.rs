use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_domain::history::HistoryState;
use threeterm_host::{HistoryCommitView, Host, history_commit_value};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-history-response-{label}-{suffix}"))
}

#[test]
fn history_response_normalizes_legacy_diagnostics_to_the_current_schema() {
    let root = temp_root("legacy-diagnostic");
    let host = Host::new();
    host.save_bracket(&root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
        .expect("history initializes");
    let view = host
        .historical_edit(&root, "l-bracket-base", "length", 0.0)
        .expect("failing historical edit is committed");

    let mut legacy = serde_json::to_value(&view.history).expect("history serializes");
    for feature in legacy["active"]["features"]
        .as_object_mut()
        .expect("history features serialize")
        .values_mut()
    {
        if let Some(diagnostic) = feature["diagnostic"].as_object_mut() {
            diagnostic.remove("affected_ids");
            diagnostic.remove("recovery");
        }
    }
    let legacy_history: HistoryState =
        serde_json::from_value(legacy).expect("legacy history deserializes");
    let response = history_commit_value(
        "historical-edit",
        threeterm_protocol::schema::HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION,
        &HistoryCommitView {
            snapshot: view.snapshot,
            history: legacy_history,
            evaluation: view.evaluation,
        },
    );

    threeterm_protocol::schema_validator::validate(
        &threeterm_protocol::schema::HISTORY_COMMIT_RESPONSE_SCHEMA,
        &response,
    )
    .expect("legacy history response matches the current schema");
    assert_eq!(
        response["diagnostics"][0]["recovery"],
        "correct_geometry_or_restore_revision"
    );
    assert_eq!(
        response["diagnostics"][0]["affected_ids"],
        serde_json::json!([])
    );

    let _ = std::fs::remove_dir_all(root);
}

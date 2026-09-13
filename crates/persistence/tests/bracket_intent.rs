use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_persistence::{
    BRACKET_INTENT_SCHEMA_VERSION, BracketDeterministicInputs, Bundle, CanonicalBracketIntent,
    CanonicalIntent, canonical_bracket_request_id, occt_worker_identity, replay_canonical_state,
};
use threeterm_protocol::artifact::sha256_hex;

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-persistence-bracket-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

#[test]
fn bracket_intent_persists_with_atomic_family_entries_and_replays() {
    let root = temp_root("intent");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let intent = CanonicalBracketIntent {
        schema_version: BRACKET_INTENT_SCHEMA_VERSION.to_string(),
        command: "bracket".to_string(),
        operation: "bracket".to_string(),
        request_id: canonical_bracket_request_id("l-bracket", 60.0, 30.0, 40.0, 3.0),
        deterministic_inputs: BracketDeterministicInputs {
            length: 60.0,
            width: 30.0,
            height: 40.0,
            thickness: 3.0,
        },
        affected_semantic_ids: vec![
            "l-bracket".to_string(),
            "l-bracket-base".to_string(),
            "l-bracket-bend".to_string(),
            "l-bracket-finish".to_string(),
            "l-bracket-independent-base".to_string(),
            "l-bracket-independent-finish".to_string(),
        ],
        source_revision: source_revision.clone(),
        worker_requirements: occt_worker_identity(),
    };
    let entries = [
        (
            "l-bracket",
            "bracket:length=60.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
        ),
        ("l-bracket-plate-vertical", "plate-vertical"),
        ("l-bracket-plate-horizontal", "plate-horizontal"),
    ];
    let committed = bundle
        .append_features_with_brep_if_revision_and_history_and_provenance_and_intent(
            &entries,
            "l-bracket",
            &source_revision,
            b"bracket-brep",
            &threeterm_domain::history::HistoryState::default()
                .initialize_l_bracket("l-bracket", 60.0, 30.0, 40.0, 3.0)
                .expect("history event creates"),
            &canonical_bracket_request_id("l-bracket", 60.0, 30.0, 40.0, 3.0),
            "{}",
            &CanonicalIntent::Bracket(intent.clone()),
        )
        .expect("bracket family publishes atomically");

    let stored = committed
        .log
        .entries()
        .iter()
        .find(|entry| entry.feature_id == "l-bracket")
        .expect("bracket entry exists");
    assert_eq!(stored.intent, Some(CanonicalIntent::Bracket(intent)));
    assert!(replay_canonical_state(&committed.log).is_ok());
    assert!(committed.graph.contains_feature("l-bracket-plate-vertical"));
    assert!(
        committed
            .graph
            .contains_feature("l-bracket-plate-horizontal")
    );

    let source_sha256 = sha256_hex(b"bracket-brep");
    let source_revision = committed.revision_hash_hex().to_string();
    let (history_event, _) = committed
        .history
        .edit_l_bracket("l-bracket", 65.0, 30.0, 40.0, 3.0)
        .expect("bracket edit history event creates");
    let edited_intent = CanonicalIntent::Bracket(CanonicalBracketIntent {
        schema_version: BRACKET_INTENT_SCHEMA_VERSION.to_string(),
        command: "bracket".to_string(),
        operation: "bracket".to_string(),
        request_id: canonical_bracket_request_id("l-bracket", 65.0, 30.0, 40.0, 3.0),
        deterministic_inputs: BracketDeterministicInputs {
            length: 65.0,
            width: 30.0,
            height: 40.0,
            thickness: 3.0,
        },
        affected_semantic_ids: vec![
            "l-bracket".to_string(),
            "l-bracket-base".to_string(),
            "l-bracket-bend".to_string(),
            "l-bracket-finish".to_string(),
            "l-bracket-independent-base".to_string(),
            "l-bracket-independent-finish".to_string(),
        ],
        source_revision: source_revision.clone(),
        worker_requirements: occt_worker_identity(),
    });
    let edited = bundle
        .replace_bracket_with_brep_if_revision_and_source_and_idempotency_payload_and_intent(
            "l-bracket",
            "bracket:length=65.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
            &source_revision,
            &source_sha256,
            Some(&canonical_bracket_request_id("l-bracket", 65.0, 30.0, 40.0, 3.0)),
            Some("{}"),
            b"edited-bracket-brep",
            Some(&edited_intent),
            Some(&history_event),
        )
        .expect("parameterized bracket replacement publishes");
    assert!(replay_canonical_state(&edited.log).is_ok());
    assert_eq!(
        edited.history.active_snapshot().features["l-bracket-base"].input_value,
        65.0
    );
    assert!(
        edited
            .log
            .entries()
            .iter()
            .any(|entry| entry.intent.as_ref() == Some(&edited_intent))
    );

    let _ = fs::remove_dir_all(root);
}

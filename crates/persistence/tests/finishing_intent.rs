use threeterm_persistence::{
    FILLET_INTENT_SCHEMA_VERSION, Bundle, CanonicalEdgeReference, CanonicalFilletIntent,
    CanonicalIntent, EdgeEvidence, EdgeProvenance, occt_worker_identity, replay_canonical_state,
};
use std::time::{SystemTime, UNIX_EPOCH};

fn selected_edge() -> CanonicalEdgeReference {
    CanonicalEdgeReference {
        semantic_id: "edge-1".to_string(),
        provenance: EdgeProvenance {
            source_feature_id: "base".to_string(),
            source_revision_id: "a".repeat(64),
            source_edge_id: "edge-source-1".to_string(),
        },
        role: "outer-perimeter".to_string(),
        evidence: EdgeEvidence {
            midpoint: [1.0, 2.0, 3.0],
            tangent: [1.0, 0.0, 0.0],
            length: 4.0,
        },
    }
}

fn temp_root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-persistence-fillet-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

#[test]
fn fillet_intent_round_trips_selected_semantic_edge() {
    let intent = CanonicalFilletIntent {
        schema_version: FILLET_INTENT_SCHEMA_VERSION.to_string(),
        command: "fillet".to_string(),
        operation: "fillet".to_string(),
        base_feature_id: "base".to_string(),
        selected_edge: selected_edge(),
        radius: 0.5,
        request_id: "request-fillet-1".to_string(),
        affected_semantic_ids: vec!["fillet-1".to_string()],
        source_revision: "a".repeat(64),
        worker_requirements: occt_worker_identity(),
    };

    intent
        .validate("fillet-1")
        .expect("selected fillet intent validates");
    let canonical = CanonicalIntent::Fillet(intent.clone());
    let encoded = serde_json::to_string(&canonical).expect("intent serializes");
    let decoded: CanonicalIntent = serde_json::from_str(&encoded).expect("intent deserializes");
    assert_eq!(decoded, canonical);
}

#[test]
fn fillet_intent_is_sealed_in_the_transaction_log_and_replays() {
    let root = temp_root();
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = bundle
        .append_feature_with_brep_if_revision(
            "base",
            "brep:base",
            &bundle.open().expect("bundle opens").revision_hash_hex(),
            b"base-brep",
        )
        .expect("base publishes")
        .revision_hash_hex()
        .to_string();
    let mut edge = selected_edge();
    edge.provenance.source_revision_id = source_revision.clone();
    let intent = CanonicalFilletIntent {
        schema_version: FILLET_INTENT_SCHEMA_VERSION.to_string(),
        command: "fillet".to_string(),
        operation: "fillet".to_string(),
        base_feature_id: "base".to_string(),
        selected_edge: edge,
        radius: 0.5,
        request_id: "request-fillet-1".to_string(),
        affected_semantic_ids: vec!["fillet-1".to_string()],
        source_revision: source_revision.clone(),
        worker_requirements: occt_worker_identity(),
    };
    let committed = bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_intent(
            "fillet-1",
            "brep:fillet-1",
            &source_revision,
            "request-fillet-1",
            "{}",
            &CanonicalIntent::Fillet(intent),
            b"fillet-brep",
        )
        .expect("fillet intent publishes");

    assert!(matches!(
        committed.log.entries().last().and_then(|entry| entry.intent.as_ref()),
        Some(CanonicalIntent::Fillet(_))
    ));
    let state = replay_canonical_state(&committed.log).expect("fillet log replays");
    assert!(state.graph.contains_feature("fillet-1"));
    let _ = std::fs::remove_dir_all(root);
}

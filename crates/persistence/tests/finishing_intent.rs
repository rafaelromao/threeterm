use std::time::{SystemTime, UNIX_EPOCH};
use threeterm_persistence::{
    Bundle, CHAMFER_INTENT_SCHEMA_VERSION, CanonicalChamferIntent, CanonicalDraftIntent,
    CanonicalEdgeReference, CanonicalFilletIntent, CanonicalIntent, CanonicalLoftIntent,
    CanonicalShellIntent, DRAFT_INTENT_SCHEMA_VERSION, EdgeEvidence, EdgeProvenance,
    FILLET_INTENT_SCHEMA_VERSION, LOFT_INTENT_SCHEMA_VERSION, SHELL_INTENT_SCHEMA_VERSION,
    occt_worker_identity, replay_canonical_state,
};

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
    let base_revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let source_revision = bundle
        .append_feature_with_brep_if_revision("base", "brep:base", &base_revision, b"base-brep")
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
        committed
            .log
            .entries()
            .last()
            .and_then(|entry| entry.intent.as_ref()),
        Some(CanonicalIntent::Fillet(_))
    ));
    let state = replay_canonical_state(&committed.log).expect("fillet log replays");
    assert!(state.graph.contains_feature("fillet-1"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn finishing_intents_capture_replayable_inputs_for_each_operation() {
    let revision = "a".repeat(64);
    let edge = selected_edge();
    let intents = [
        CanonicalIntent::Chamfer(CanonicalChamferIntent {
            schema_version: CHAMFER_INTENT_SCHEMA_VERSION.to_string(),
            command: "chamfer".to_string(),
            operation: "chamfer".to_string(),
            base_feature_id: "base".to_string(),
            selected_edge: edge,
            distance: 0.25,
            request_id: "request-chamfer".to_string(),
            affected_semantic_ids: vec!["chamfer-1".to_string()],
            source_revision: revision.clone(),
            worker_requirements: occt_worker_identity(),
        }),
        CanonicalIntent::Shell(CanonicalShellIntent {
            schema_version: SHELL_INTENT_SCHEMA_VERSION.to_string(),
            command: "shell".to_string(),
            operation: "shell".to_string(),
            base_feature_id: "base".to_string(),
            thickness: 0.3,
            request_id: "request-shell".to_string(),
            affected_semantic_ids: vec!["shell-1".to_string()],
            source_revision: revision.clone(),
            worker_requirements: occt_worker_identity(),
        }),
        CanonicalIntent::Draft(CanonicalDraftIntent {
            schema_version: DRAFT_INTENT_SCHEMA_VERSION.to_string(),
            command: "draft".to_string(),
            operation: "draft".to_string(),
            base_feature_id: "base".to_string(),
            angle: 0.2,
            pull_direction: [0.0, 0.0, 1.0],
            request_id: "request-draft".to_string(),
            affected_semantic_ids: vec!["draft-1".to_string()],
            source_revision: revision.clone(),
            worker_requirements: occt_worker_identity(),
        }),
        CanonicalIntent::Loft(CanonicalLoftIntent {
            schema_version: LOFT_INTENT_SCHEMA_VERSION.to_string(),
            command: "loft".to_string(),
            operation: "loft".to_string(),
            profiles: vec![
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
                vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0]],
            ],
            is_solid: true,
            ruled: false,
            request_id: "request-loft".to_string(),
            affected_semantic_ids: vec!["loft-1".to_string()],
            source_revision: revision,
            worker_requirements: occt_worker_identity(),
        }),
    ];

    for (intent, feature_id) in intents
        .iter()
        .zip(["chamfer-1", "shell-1", "draft-1", "loft-1"])
    {
        intent
            .validate(feature_id)
            .expect("finishing intent validates");
        let encoded = serde_json::to_string(intent).expect("intent serializes");
        assert_eq!(
            serde_json::from_str::<CanonicalIntent>(&encoded).expect("intent deserializes"),
            *intent
        );
    }
}

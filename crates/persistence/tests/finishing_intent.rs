use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};
use threeterm_persistence::{
    Bundle, CHAMFER_INTENT_SCHEMA_VERSION, CanonicalChamferIntent, CanonicalDraftIntent,
    CanonicalEdgeReference, CanonicalExtrudeIntent, CanonicalFilletIntent, CanonicalIntent,
    CanonicalLoftIntent, CanonicalShellIntent, DRAFT_INTENT_SCHEMA_VERSION,
    EXTRUDE_INTENT_SCHEMA_VERSION, EdgeEvidence, EdgeProvenance, ExtrudeDeterministicInputs,
    FILLET_INTENT_SCHEMA_VERSION, LOFT_INTENT_SCHEMA_VERSION, SHELL_INTENT_SCHEMA_VERSION,
    occt_worker_identity, replay_canonical_state,
};
use threeterm_protocol::artifact::sha256_hex;

fn selected_edge() -> CanonicalEdgeReference {
    let semantic_id = format!(
        "edge-{}",
        sha256_hex(
            &serde_json::to_vec(&([1.0, 2.0, 3.0], [1.0, 0.0, 0.0], 4.0))
                .expect("edge evidence serializes")
        )
    );
    CanonicalEdgeReference {
        semantic_id,
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
fn replay_rejects_feature_ids_that_escape_the_brep_directory() {
    let root = temp_root();
    let bundle = Bundle::create(&root).expect("bundle creates");
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();

    let error = bundle
        .restore_derived_breps_if_revision(
            &revision,
            &[("../outside".to_string(), b"not-a-brep".to_vec())],
        )
        .expect_err("path traversal feature ID is rejected");
    assert!(error.to_string().contains("plain path component"));
    assert!(
        !root
            .parent()
            .expect("root has parent")
            .join("outside.brep")
            .exists()
    );

    let _ = fs::remove_dir_all(root);
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
    let base_intent = CanonicalExtrudeIntent {
        schema_version: EXTRUDE_INTENT_SCHEMA_VERSION.to_string(),
        command: "extrude".to_string(),
        operation: "additive".to_string(),
        mode: "additive".to_string(),
        target_feature_id: None,
        request_id: "request-base".to_string(),
        deterministic_inputs: ExtrudeDeterministicInputs {
            profile: vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            height: 2.0,
        },
        affected_semantic_ids: vec!["base".to_string()],
        source_revision: base_revision.clone(),
        worker_requirements: occt_worker_identity(),
    };
    let source_revision = bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_intent(
            "base",
            "brep:base",
            &base_revision,
            "request-base",
            "{}",
            &CanonicalIntent::Extrude(base_intent),
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
fn replay_batch_authenticates_every_result_before_promotion() {
    let root = temp_root();
    let bundle = Bundle::create(&root).expect("bundle creates");
    let base_revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let base_intent = CanonicalExtrudeIntent {
        schema_version: EXTRUDE_INTENT_SCHEMA_VERSION.to_string(),
        command: "extrude".to_string(),
        operation: "additive".to_string(),
        mode: "additive".to_string(),
        target_feature_id: None,
        request_id: "request-batch-base".to_string(),
        deterministic_inputs: ExtrudeDeterministicInputs {
            profile: vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            height: 2.0,
        },
        affected_semantic_ids: vec!["base".to_string()],
        source_revision: base_revision.clone(),
        worker_requirements: occt_worker_identity(),
    };
    let source_revision = bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_intent(
            "base",
            "brep:base",
            &base_revision,
            "request-batch-base",
            "{}",
            &CanonicalIntent::Extrude(base_intent),
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
        request_id: "request-fillet-batch".to_string(),
        affected_semantic_ids: vec!["fillet-1".to_string()],
        source_revision: source_revision.clone(),
        worker_requirements: occt_worker_identity(),
    };
    bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_intent(
            "fillet-1",
            "brep:fillet-1",
            &source_revision,
            "request-fillet-batch",
            "{}",
            &CanonicalIntent::Fillet(intent),
            b"fillet-brep",
        )
        .expect("fillet publishes");
    let final_revision = Bundle::at(&root)
        .open()
        .expect("final bundle opens")
        .revision_hash_hex()
        .to_string();
    fs::remove_file(root.join("brep/base.brep")).expect("base removes");
    fs::remove_file(root.join("brep/fillet-1.brep")).expect("fillet removes");

    let error = Bundle::at(&root)
        .restore_derived_breps_if_revision(
            &final_revision,
            &[
                ("base".to_string(), b"base-brep".to_vec()),
                ("fillet-1".to_string(), b"wrong".to_vec()),
            ],
        )
        .expect_err("invalid later result must reject the generation");
    assert!(error.to_string().contains("authenticated geometry"));
    assert!(!root.join("brep/base.brep").exists());
    assert!(!root.join("brep/fillet-1.brep").exists());

    Bundle::at(&root)
        .restore_derived_breps_if_revision(
            &final_revision,
            &[
                ("base".to_string(), b"base-brep".to_vec()),
                ("fillet-1".to_string(), b"fillet-brep".to_vec()),
            ],
        )
        .expect("authenticated generation restores together");
    assert_eq!(fs::read(root.join("brep/base.brep")).unwrap(), b"base-brep");
    assert_eq!(
        fs::read(root.join("brep/fillet-1.brep")).unwrap(),
        b"fillet-brep"
    );
    let _ = fs::remove_dir_all(root);
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

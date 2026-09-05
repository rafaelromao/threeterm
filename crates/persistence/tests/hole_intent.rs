use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_persistence::{
    Bundle, CanonicalHoleIntent, CanonicalIntent, HOLE_INTENT_SCHEMA_VERSION,
    HoleDeterministicInputs, occt_worker_identity, replay_canonical_state,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-persistence-hole-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

fn drilled_intent(
    base: &str,
    feature: &str,
    request_id: &str,
    source_revision: &str,
) -> CanonicalHoleIntent {
    CanonicalHoleIntent {
        schema_version: HOLE_INTENT_SCHEMA_VERSION.to_string(),
        command: "hole".to_string(),
        hole_kind: "drilled".to_string(),
        base_feature_id: base.to_string(),
        request_id: request_id.to_string(),
        deterministic_inputs: HoleDeterministicInputs {
            position: [1.5, 1.5, 0.0],
            direction: [0.0, 0.0, 1.0],
            diameter: 1.0,
            thread_designation: None,
            thread_pitch: None,
            thread_depth: None,
        },
        affected_semantic_ids: vec![feature.to_string()],
        source_revision: source_revision.to_string(),
        worker_requirements: occt_worker_identity(),
    }
}

fn tapped_intent(
    base: &str,
    feature: &str,
    request_id: &str,
    source_revision: &str,
) -> CanonicalHoleIntent {
    CanonicalHoleIntent {
        schema_version: HOLE_INTENT_SCHEMA_VERSION.to_string(),
        command: "hole".to_string(),
        hole_kind: "tapped".to_string(),
        base_feature_id: base.to_string(),
        request_id: request_id.to_string(),
        deterministic_inputs: HoleDeterministicInputs {
            position: [1.5, 1.5, 0.0],
            direction: [0.0, 0.0, 1.0],
            diameter: 1.0,
            thread_designation: Some("M6x1".to_string()),
            thread_pitch: Some(1.0),
            thread_depth: Some(2.0),
        },
        affected_semantic_ids: vec![feature.to_string()],
        source_revision: source_revision.to_string(),
        worker_requirements: occt_worker_identity(),
    }
}

fn seed_solid(bundle: &Bundle, feature_id: &str) -> String {
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    bundle
        .append_feature_with_brep_if_revision(
            feature_id,
            &format!("brep:{feature_id}"),
            &revision,
            b"solid-brep",
        )
        .expect("solid seed publishes")
        .revision_hash_hex()
        .to_string()
}

#[test]
fn drilled_hole_intent_persists_and_replays() {
    let root = temp_root("drilled-replay");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = seed_solid(&bundle, "base-1");
    let intent = drilled_intent("base-1", "hole-1", "request-hole-1", &source_revision);
    intent.validate("hole-1").expect("drilled intent validates");

    let committed = bundle
        .append_new_feature_with_brep_if_revision_and_hole_intent(
            "hole-1",
            "hole:drilled",
            &source_revision,
            "request-hole-1",
            "{}",
            &intent,
            b"holed-brep",
        )
        .expect("drilled hole intent publishes");

    let entry = committed
        .log
        .entries()
        .last()
        .expect("hole transaction exists");
    assert!(
        matches!(entry.intent.as_ref(), Some(CanonicalIntent::Hole(_))),
        "hole entry carries an explicit hole intent, not an opaque cut"
    );
    // Discriminator is explicit and versioned.
    if let Some(CanonicalIntent::Hole(stored)) = entry.intent.as_ref() {
        assert_eq!(stored.hole_kind, "drilled");
        assert_eq!(stored.schema_version, HOLE_INTENT_SCHEMA_VERSION);
        assert_eq!(stored.base_feature_id, "base-1");
    } else {
        panic!("expected hole intent");
    }
    let state = replay_canonical_state(&committed.log).expect("hole log replays");
    assert!(state.graph.contains_feature("hole-1"));
    assert!(state.graph.contains_feature("base-1"));

    let reopened = Bundle::at(&root).open().expect("generation reopens");
    let state = replay_canonical_state(&reopened.log).expect("reopened hole log replays");
    assert!(state.graph.contains_feature("hole-1"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn tapped_hole_intent_persists_with_thread_metadata_and_replays() {
    let root = temp_root("tapped-replay");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = seed_solid(&bundle, "base-1");
    let intent = tapped_intent("base-1", "hole-1", "request-tapped-1", &source_revision);
    intent.validate("hole-1").expect("tapped intent validates");

    let committed = bundle
        .append_new_feature_with_brep_if_revision_and_hole_intent(
            "hole-1",
            "hole:tapped",
            &source_revision,
            "request-tapped-1",
            "{}",
            &intent,
            b"holed-brep",
        )
        .expect("tapped hole intent publishes");

    let entry = committed
        .log
        .entries()
        .last()
        .expect("hole transaction exists");
    if let Some(CanonicalIntent::Hole(stored)) = entry.intent.as_ref() {
        assert_eq!(stored.hole_kind, "tapped");
        assert_eq!(
            stored.deterministic_inputs.thread_designation.as_deref(),
            Some("M6x1")
        );
        assert_eq!(stored.deterministic_inputs.thread_pitch, Some(1.0));
        assert_eq!(stored.deterministic_inputs.thread_depth, Some(2.0));
    } else {
        panic!("expected tapped hole intent");
    }
    let state = replay_canonical_state(&committed.log).expect("tapped log replays");
    assert!(state.graph.contains_feature("hole-1"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn hole_intent_rejects_invalid_geometry_and_unknown_support() {
    let root = temp_root("invalid");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = seed_solid(&bundle, "base-1");

    // Zero diameter.
    let mut bad = drilled_intent("base-1", "hole-1", "req-1", &source_revision);
    bad.deterministic_inputs.diameter = 0.0;
    assert!(bad.validate("hole-1").is_err());

    // Zero direction.
    let mut bad = drilled_intent("base-1", "hole-1", "req-1", &source_revision);
    bad.deterministic_inputs.direction = [0.0, 0.0, 0.0];
    assert!(bad.validate("hole-1").is_err());

    // Non-finite position.
    let mut bad = drilled_intent("base-1", "hole-1", "req-1", &source_revision);
    bad.deterministic_inputs.position = [f64::NAN, 0.0, 0.0];
    assert!(bad.validate("hole-1").is_err());

    // Unknown base support fails at publish, preserving the prior snapshot.
    let before = bundle.open().expect("snapshot loads");
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let intent = drilled_intent("missing-base", "hole-1", "req-1", &source_revision);
    let result = bundle.append_new_feature_with_brep_if_revision_and_hole_intent(
        "hole-1",
        "hole:drilled",
        &source_revision,
        "req-1",
        "{}",
        &intent,
        b"holed-brep",
    );
    assert!(result.is_err(), "unknown support must fail closed");
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads"),
        before_manifest
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads"),
        before_log
    );
    assert_eq!(
        bundle.open().expect("reloads").revision_hash_hex(),
        before.revision_hash_hex()
    );

    // Drilled must not carry thread metadata; tapped must carry all three.
    let mut bad = drilled_intent("base-1", "hole-1", "req-1", &source_revision);
    bad.deterministic_inputs.thread_designation = Some("M6x1".to_string());
    assert!(bad.validate("hole-1").is_err());
    let mut bad = tapped_intent("base-1", "hole-1", "req-1", &source_revision);
    bad.deterministic_inputs.thread_pitch = Some(0.0);
    assert!(bad.validate("hole-1").is_err());

    // Unknown fields fail closed via deny_unknown_fields.
    let raw = serde_json::json!({
        "schema_version": HOLE_INTENT_SCHEMA_VERSION,
        "command": "hole",
        "hole_kind": "drilled",
        "base_feature_id": "base-1",
        "request_id": "req-1",
        "deterministic_inputs": {
            "position": [0.0, 0.0, 0.0],
            "direction": [0.0, 0.0, 1.0],
            "diameter": 1.0,
            "unexpected": 1.0
        },
        "affected_semantic_ids": ["hole-1"],
        "source_revision": source_revision,
        "worker_requirements": threeterm_persistence::occt_worker_identity(),
    });
    assert!(serde_json::from_value::<CanonicalHoleIntent>(raw).is_err());

    let _ = fs::remove_dir_all(root);
}

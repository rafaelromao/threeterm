//! Canonical intent round-trip and replay tests for revolve, mirror,
//! linear pattern, and circular pattern.
//!
//! These tests are worker-free: they cover intent validation, log
//! persistence, fail-closed decoding, and `replay_canonical_state`
//! through the public persistence API.

use threeterm_persistence::{
    Bundle, CIRCULAR_PATTERN_INTENT_SCHEMA_VERSION, CanonicalCircularPatternIntent,
    CanonicalIntent, CanonicalLinearPatternIntent, CanonicalMirrorIntent, CanonicalRevolveIntent,
    CircularPatternDeterministicInputs, LINEAR_PATTERN_INTENT_SCHEMA_VERSION,
    LinearPatternDeterministicInputs, MIRROR_INTENT_SCHEMA_VERSION, MirrorDeterministicInputs,
    REVOLVE_INTENT_SCHEMA_VERSION, RevolveDeterministicInputs, occt_worker_identity,
    replay_canonical_state,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-canonical-intent-{label}-{}-{nanos}",
        std::process::id(),
    ))
}

fn triangle_profile() -> Vec<[f64; 2]> {
    vec![[0.0, 0.0], [4.0, 0.0], [2.0, 4.0]]
}

fn revolve_intent(request_id: &str, source_revision: &str, feature_id: &str) -> CanonicalIntent {
    CanonicalIntent::Revolve(CanonicalRevolveIntent {
        schema_version: REVOLVE_INTENT_SCHEMA_VERSION.to_string(),
        command: "revolve".to_string(),
        operation: "revolve".to_string(),
        request_id: request_id.to_string(),
        deterministic_inputs: RevolveDeterministicInputs {
            profile: triangle_profile(),
            axis_point: [0.0, 0.0, 0.0],
            axis_direction: [0.0, 1.0, 0.0],
            angle: std::f64::consts::PI,
        },
        affected_semantic_ids: vec![feature_id.to_string()],
        source_revision: source_revision.to_string(),
        worker_requirements: occt_worker_identity(),
    })
}

fn mirror_intent(request_id: &str, source_revision: &str, feature_id: &str) -> CanonicalIntent {
    CanonicalIntent::Mirror(CanonicalMirrorIntent {
        schema_version: MIRROR_INTENT_SCHEMA_VERSION.to_string(),
        command: "mirror".to_string(),
        operation: "mirror".to_string(),
        request_id: request_id.to_string(),
        deterministic_inputs: MirrorDeterministicInputs {
            base_feature_id: "base-1".to_string(),
            plane_point: [0.0, 0.0, 0.0],
            plane_normal: [1.0, 0.0, 0.0],
        },
        affected_semantic_ids: vec![feature_id.to_string(), "base-1".to_string()],
        source_revision: source_revision.to_string(),
        worker_requirements: occt_worker_identity(),
    })
}

fn linear_pattern_intent(
    request_id: &str,
    source_revision: &str,
    feature_id: &str,
) -> CanonicalIntent {
    CanonicalIntent::LinearPattern(CanonicalLinearPatternIntent {
        schema_version: LINEAR_PATTERN_INTENT_SCHEMA_VERSION.to_string(),
        command: "linear-pattern".to_string(),
        operation: "linear-pattern".to_string(),
        request_id: request_id.to_string(),
        deterministic_inputs: LinearPatternDeterministicInputs {
            base_feature_id: "base-1".to_string(),
            direction: [1.0, 0.0, 0.0],
            count: 3,
            spacing: 5.0,
        },
        affected_semantic_ids: vec![feature_id.to_string(), "base-1".to_string()],
        source_revision: source_revision.to_string(),
        worker_requirements: occt_worker_identity(),
    })
}

fn circular_pattern_intent(
    request_id: &str,
    source_revision: &str,
    feature_id: &str,
) -> CanonicalIntent {
    CanonicalIntent::CircularPattern(CanonicalCircularPatternIntent {
        schema_version: CIRCULAR_PATTERN_INTENT_SCHEMA_VERSION.to_string(),
        command: "circular-pattern".to_string(),
        operation: "circular-pattern".to_string(),
        request_id: request_id.to_string(),
        deterministic_inputs: CircularPatternDeterministicInputs {
            base_feature_id: "base-1".to_string(),
            axis_point: [0.0, 0.0, 0.0],
            axis_normal: [0.0, 0.0, 1.0],
            angle_step: std::f64::consts::FRAC_PI_2,
            count: 4,
        },
        affected_semantic_ids: vec![feature_id.to_string(), "base-1".to_string()],
        source_revision: source_revision.to_string(),
        worker_requirements: occt_worker_identity(),
    })
}

#[test]
fn revolve_intent_validates_deterministic_inputs() {
    let revision = "a".repeat(64);
    let intent = revolve_intent("req-1", &revision, "rev-1");
    assert_eq!(intent.command(), "revolve");
    assert_eq!(intent.operation(), "revolve");
    assert_eq!(intent.request_id(), "req-1");
    assert_eq!(intent.affected_semantic_ids(), &["rev-1".to_string()]);
    assert_eq!(intent.source_revision(), revision);
    assert!(intent.base_reference().is_none());
    intent.validate("rev-1").expect("valid revolve intent");

    let mut bad = revolve_intent("req-1", &revision, "rev-1");
    let CanonicalIntent::Revolve(inner) = &mut bad else {
        unreachable!("revolve intent");
    };
    inner.deterministic_inputs.profile.pop();
    inner.deterministic_inputs.profile.pop();
    assert!(bad.validate("rev-1").is_err());

    let mut bad = revolve_intent("req-1", &revision, "rev-1");
    let CanonicalIntent::Revolve(inner) = &mut bad else {
        unreachable!("revolve intent");
    };
    inner.deterministic_inputs.axis_direction = [0.0, 0.0, 0.0];
    assert!(bad.validate("rev-1").is_err());

    let mut bad = revolve_intent("req-1", &revision, "rev-1");
    let CanonicalIntent::Revolve(inner) = &mut bad else {
        unreachable!("revolve intent");
    };
    inner.deterministic_inputs.angle = 0.0;
    assert!(bad.validate("rev-1").is_err());

    let bad = revolve_intent("req-1", "not-hex", "rev-1");
    assert!(bad.validate("rev-1").is_err());

    let intent = revolve_intent("req-1", &revision, "rev-1");
    assert!(intent.validate("other-feature").is_err());
}

#[test]
fn mirror_and_pattern_intents_validate_base_references() {
    let revision = "b".repeat(64);
    for intent in [
        mirror_intent("req-m", &revision, "mir-1"),
        linear_pattern_intent("req-l", &revision, "lin-1"),
        circular_pattern_intent("req-c", &revision, "cir-1"),
    ] {
        intent
            .validate(intent.affected_semantic_ids()[0].as_str())
            .expect("valid intent");
        assert_eq!(intent.base_reference(), Some("base-1"));
        assert_eq!(
            &intent.affected_semantic_ids()[1..],
            &["base-1".to_string()]
        );
    }

    let mut bad = mirror_intent("req-m", &revision, "mir-1");
    let CanonicalIntent::Mirror(inner) = &mut bad else {
        unreachable!("mirror intent");
    };
    inner.deterministic_inputs.plane_normal = [0.0, 0.0, 0.0];
    assert!(bad.validate("mir-1").is_err());

    let mut bad = linear_pattern_intent("req-l", &revision, "lin-1");
    let CanonicalIntent::LinearPattern(inner) = &mut bad else {
        unreachable!("linear pattern intent");
    };
    inner.deterministic_inputs.count = 0;
    assert!(bad.validate("lin-1").is_err());

    let mut bad = circular_pattern_intent("req-c", &revision, "cir-1");
    let CanonicalIntent::CircularPattern(inner) = &mut bad else {
        unreachable!("circular pattern intent");
    };
    inner.deterministic_inputs.angle_step = 7.0;
    assert!(bad.validate("cir-1").is_err());
}

#[test]
fn revolve_intent_persists_and_replays_artifact_free() {
    let root = temp_root("revolve-roundtrip");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let intent = revolve_intent("req-revolve-1", &revision, "rev-1");
    bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_canonical_intent(
            "rev-1",
            "brep:rev-1",
            &revision,
            "req-revolve-1",
            "{}",
            &intent,
            b"fake-revolve-brep",
        )
        .expect("revolve transaction appends");

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    let entry = loaded.log.entries().last().expect("entry exists");
    assert_eq!(entry.intent.as_ref(), Some(&intent));
    assert_eq!(entry.idempotency_key.as_deref(), Some("req-revolve-1"));

    let state = replay_canonical_state(&loaded.log).expect("replay succeeds");
    assert!(state.graph.contains_feature("rev-1"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn mirror_replay_requires_its_base_feature() {
    let root = temp_root("mirror-base");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let intent = mirror_intent("req-mirror-1", &revision, "mir-1");
    let result = bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_canonical_intent(
            "mir-1",
            "brep:mir-1",
            &revision,
            "req-mirror-1",
            "{}",
            &intent,
            b"fake-mirror-brep",
        );
    assert!(
        result.is_err(),
        "mirror intent with a missing base feature must fail before mutation"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unknown_intent_command_fails_closed_on_decode() {
    let root = temp_root("unknown-intent");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let intent = revolve_intent("req-revolve-1", &revision, "rev-1");
    bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_canonical_intent(
            "rev-1",
            "brep:rev-1",
            &revision,
            "req-revolve-1",
            "{}",
            &intent,
            b"fake-revolve-brep",
        )
        .expect("revolve transaction appends");

    let mut unknown = serde_json::to_value(&intent).expect("intent serializes");
    unknown["command"] = serde_json::Value::String("revolve-evil".to_string());
    assert!(serde_json::from_value::<CanonicalIntent>(unknown).is_err());

    let log_path = root.join("transactions.log");
    let log_bytes = std::fs::read(&log_path).expect("log reads");
    let tampered = String::from_utf8(log_bytes)
        .expect("log is utf-8")
        .replace("\"command\":\"revolve\"", "\"command\":\"revolve-evil\"");
    assert_ne!(
        tampered.matches("\"command\":\"revolve-evil\"").count(),
        0,
        "tampered log carries the unknown command"
    );
    std::fs::write(&log_path, tampered).expect("tampered log writes");
    let reopened = Bundle::at(&root).open();
    assert!(
        reopened.is_err(),
        "unknown canonical intent command must fail closed"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unsupported_intent_version_and_worker_fail_before_mutation() {
    let root = temp_root("unsupported-intent");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(root.join("transactions.log")).expect("log reads");

    for mutation in ["version", "worker"] {
        let mut intent = revolve_intent("req-unsupported", &revision, "unsupported");
        let CanonicalIntent::Revolve(inner) = &mut intent else {
            unreachable!("revolve intent");
        };
        if mutation == "version" {
            inner.schema_version = "threeterm.intent.revolve/99".to_string();
        } else {
            inner.worker_requirements.worker_schema_version =
                "threeterm.workers.occt/99".to_string();
        }
        assert!(
            bundle
                .append_new_feature_with_brep_if_revision_and_provenance_and_canonical_intent(
                    "unsupported",
                    "brep:unsupported",
                    &revision,
                    "req-unsupported",
                    "{}",
                    &intent,
                    b"unsupported",
                )
                .is_err()
        );
        assert_eq!(
            std::fs::read(root.join("manifest.json")).unwrap(),
            manifest_before
        );
        assert_eq!(
            std::fs::read(root.join("transactions.log")).unwrap(),
            log_before
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

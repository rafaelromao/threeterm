use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_persistence::{
    BOOLEAN_INTENT_SCHEMA_VERSION, Bundle, CanonicalBooleanIntent, CanonicalIntent,
    occt_worker_identity, replay_canonical_state,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-persistence-boolean-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

fn boolean_intent(
    operation: &str,
    base: &str,
    tool: &str,
    feature: &str,
    request_id: &str,
    source_revision: &str,
) -> CanonicalBooleanIntent {
    CanonicalBooleanIntent {
        schema_version: BOOLEAN_INTENT_SCHEMA_VERSION.to_string(),
        command: "boolean".to_string(),
        operation: operation.to_string(),
        base_feature_id: base.to_string(),
        tool_feature_id: tool.to_string(),
        request_id: request_id.to_string(),
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
fn boolean_cut_intent_persists_and_replays() {
    let root = temp_root("cut-replay");
    let bundle = Bundle::create(&root).expect("bundle creates");
    seed_solid(&bundle, "base-1");
    let source_revision = seed_solid(&bundle, "tool-1");
    let intent = boolean_intent(
        "cut",
        "base-1",
        "tool-1",
        "cut-1",
        "request-cut-1",
        &source_revision,
    );
    intent.validate("cut-1").expect("boolean intent validates");

    let committed = bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_intent(
            "cut-1",
            "brep:cut-1",
            &source_revision,
            "request-cut-1",
            "{}",
            &CanonicalIntent::Boolean(intent),
            b"cut-brep",
        )
        .expect("boolean cut intent publishes");

    let entry = committed
        .log
        .entries()
        .last()
        .expect("boolean transaction exists");
    assert!(
        matches!(entry.intent.as_ref(), Some(CanonicalIntent::Boolean(_))),
        "boolean entry carries a boolean intent"
    );
    let state = replay_canonical_state(&committed.log).expect("boolean log replays");
    assert!(state.graph.contains_feature("cut-1"));
    assert!(state.graph.contains_feature("base-1"));
    assert!(state.graph.contains_feature("tool-1"));

    // Two-epoch load: the sealed generation reopens with its intent intact.
    let reopened = Bundle::at(&root).open().expect("generation reopens");
    let state = replay_canonical_state(&reopened.log).expect("reopened boolean log replays");
    assert!(state.graph.contains_feature("cut-1"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn boolean_common_intent_persists_and_replays() {
    let root = temp_root("common-replay");
    let bundle = Bundle::create(&root).expect("bundle creates");
    seed_solid(&bundle, "base-1");
    let source_revision = seed_solid(&bundle, "tool-1");
    let intent = boolean_intent(
        "common",
        "base-1",
        "tool-1",
        "common-1",
        "request-common-1",
        &source_revision,
    );

    let committed = bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_intent(
            "common-1",
            "brep:common-1",
            &source_revision,
            "request-common-1",
            "{}",
            &CanonicalIntent::Boolean(intent),
            b"common-brep",
        )
        .expect("boolean common intent publishes");
    replay_canonical_state(&committed.log).expect("boolean log replays");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn boolean_append_rejects_a_missing_base_feature() {
    let root = temp_root("missing-base");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = seed_solid(&bundle, "tool-1");
    let intent = boolean_intent(
        "cut",
        "does-not-exist",
        "tool-1",
        "cut-1",
        "request-cut-1",
        &source_revision,
    );

    let result = bundle.append_new_feature_with_brep_if_revision_and_provenance_and_intent(
        "cut-1",
        "brep:cut-1",
        &source_revision,
        "request-cut-1",
        "{}",
        &CanonicalIntent::Boolean(intent),
        b"cut-brep",
    );
    assert!(
        result.is_err(),
        "boolean with a missing base must fail closed"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn boolean_append_rejects_a_missing_tool_feature() {
    let root = temp_root("missing-tool");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = seed_solid(&bundle, "base-1");
    let intent = boolean_intent(
        "common",
        "base-1",
        "does-not-exist",
        "common-1",
        "request-common-1",
        &source_revision,
    );

    let result = bundle.append_new_feature_with_brep_if_revision_and_provenance_and_intent(
        "common-1",
        "brep:common-1",
        &source_revision,
        "request-common-1",
        "{}",
        &CanonicalIntent::Boolean(intent),
        b"common-brep",
    );
    assert!(
        result.is_err(),
        "boolean with a missing tool must fail closed"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_fuse_entry_without_intent_remains_loadable_but_not_recomputable() {
    let root = temp_root("legacy-fuse");
    let bundle = Bundle::create(&root).expect("bundle creates");
    seed_solid(&bundle, "base-1");
    let source_revision = seed_solid(&bundle, "tool-1");

    // Old fuse entries carry no intent: they load and replay as plain
    // features but offer nothing to the geometry recompute path.
    let committed = bundle
        .append_new_feature_with_brep_if_revision_and_provenance(
            "fuse-1",
            "brep:fuse-1",
            &source_revision,
            "request-fuse-1",
            "{}",
            b"fuse-brep",
        )
        .expect("legacy fuse publishes");
    let entry = committed
        .log
        .entries()
        .last()
        .expect("fuse transaction exists");
    assert!(entry.intent.is_none());
    replay_canonical_state(&committed.log).expect("legacy fuse log replays");

    let reopened = Bundle::at(&root).open().expect("generation reopens");
    assert!(
        reopened
            .log
            .entries()
            .last()
            .expect("entry")
            .intent
            .is_none(),
        "legacy fuse stays non-recomputable across epochs"
    );

    let _ = fs::remove_dir_all(root);
}

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use threeterm_domain::history::HistoryState;
use threeterm_persistence::{
    Bundle, HistoryBrepReplacement, PublicationFailurePoint, fail_next_publication_at,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-history-atomicity-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn seed_history_bundle(root: &std::path::Path) -> Bundle {
    let bundle = Bundle::create(root).expect("bundle creates");
    let state = HistoryState::default();
    let init = state
        .initialize_l_bracket("family-a", 60.0, 30.0, 40.0, 3.0)
        .expect("history initializes");
    bundle
        .append_features_with_history(
            &[
                ("family-a-plate-vertical", "plate-vertical"),
                ("family-a-plate-horizontal", "plate-horizontal"),
            ],
            &init,
        )
        .expect("history init publishes");
    let loaded = bundle.open().expect("bundle opens");
    bundle
        .append_feature_with_brep_if_revision(
            "family-a",
            "bracket:length=60.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
            loaded.revision_hash_hex(),
            b"seed-brep",
        )
        .expect("seed BREP publishes");
    bundle
}

#[test]
fn history_event_and_brep_replacement_publish_as_one_conditional_transaction() {
    let root = temp_root("commit");
    let bundle = seed_history_bundle(&root);
    let loaded = bundle.open().expect("bundle opens");
    let (event, evaluation) = loaded
        .history
        .historical_edit("family-a-base", "length", 61.0)
        .expect("edit builds");
    assert!(evaluation.diagnostics.is_empty());
    let expected_revision = loaded.revision_hash_hex().to_string();
    let source_sha = sha_hex(b"seed-brep");

    let updated = bundle
        .replace_bracket_families_with_history_if_revision(
            &[HistoryBrepReplacement {
                feature_id: "family-a",
                kind: "bracket:length=61.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
                brep_bytes: b"recomputed-brep",
                expected_source_sha256: &source_sha,
                idempotency_key: "history-historical-edit-2",
                idempotency_payload: "length=61",
            }],
            &expected_revision,
            &event,
        )
        .expect("combined publication succeeds");

    assert_eq!(
        updated.history.event_ordinal(),
        loaded.history.event_ordinal() + 1
    );
    assert_eq!(
        updated.history.active_snapshot().revision_id,
        "history-revision-2"
    );
    assert_eq!(
        fs::read(root.join("brep/family-a.brep")).expect("BREP reads"),
        b"recomputed-brep"
    );
    let log = fs::read_to_string(root.join("transactions.log")).expect("log reads");
    assert!(log.contains("history-event"));
    assert!(log.contains("recomputed-brep") || log.contains("family-a"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_revision_guard_commits_neither_history_nor_geometry() {
    let root = temp_root("stale-revision");
    let bundle = seed_history_bundle(&root);
    let loaded = bundle.open().expect("bundle opens");
    let (event, _) = loaded
        .history
        .historical_edit("family-a-base", "length", 61.0)
        .expect("edit builds");
    let before_ordinal = loaded.history.event_ordinal();
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let source_sha = sha_hex(b"seed-brep");

    let error = bundle
        .replace_bracket_families_with_history_if_revision(
            &[HistoryBrepReplacement {
                feature_id: "family-a",
                kind: "bracket:length=61.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
                brep_bytes: b"must-not-publish",
                expected_source_sha256: &source_sha,
                idempotency_key: "history-historical-edit-2",
                idempotency_payload: "length=61",
            }],
            &"0".repeat(64),
            &event,
        )
        .expect_err("an intervening writer must reject the whole publication");
    let _ = format!("{error:?}");

    let reloaded = bundle.open().expect("bundle reopens");
    assert_eq!(reloaded.history.event_ordinal(), before_ordinal);
    assert_eq!(
        fs::read(root.join("brep/family-a.brep")).expect("BREP reads"),
        b"seed-brep"
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log re-reads"),
        before_log
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn source_digest_mismatch_commits_neither_history_nor_geometry() {
    let root = temp_root("source-mismatch");
    let bundle = seed_history_bundle(&root);
    let loaded = bundle.open().expect("bundle opens");
    let (event, _) = loaded
        .history
        .historical_edit("family-a-base", "length", 61.0)
        .expect("edit builds");
    let expected_revision = loaded.revision_hash_hex().to_string();
    let before_ordinal = loaded.history.event_ordinal();
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");

    let error = bundle
        .replace_bracket_families_with_history_if_revision(
            &[HistoryBrepReplacement {
                feature_id: "family-a",
                kind: "bracket:length=61.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
                brep_bytes: b"must-not-publish",
                expected_source_sha256: &sha_hex(b"someone-else-wrote"),
                idempotency_key: "history-historical-edit-2",
                idempotency_payload: "length=61",
            }],
            &expected_revision,
            &event,
        )
        .expect_err("a changed source BREP must reject the whole publication");
    let _ = format!("{error:?}");

    let reloaded = bundle.open().expect("bundle reopens");
    assert_eq!(reloaded.history.event_ordinal(), before_ordinal);
    assert_eq!(
        fs::read(root.join("brep/family-a.brep")).expect("BREP reads"),
        b"seed-brep"
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log re-reads"),
        before_log
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_brep_promotion_commits_neither_history_nor_geometry() {
    let root = temp_root("rollback");
    let bundle = seed_history_bundle(&root);
    let loaded = bundle.open().expect("bundle opens");
    let (event, _) = loaded
        .history
        .historical_edit("family-a-base", "length", 61.0)
        .expect("edit builds");
    let expected_revision = loaded.revision_hash_hex().to_string();
    let before_ordinal = loaded.history.event_ordinal();
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let source_sha = sha_hex(b"seed-brep");

    fail_next_publication_at(PublicationFailurePoint::BrepCopy);
    let error = bundle
        .replace_bracket_families_with_history_if_revision(
            &[HistoryBrepReplacement {
                feature_id: "family-a",
                kind: "bracket:length=61.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
                brep_bytes: b"must-not-publish",
                expected_source_sha256: &source_sha,
                idempotency_key: "history-historical-edit-2",
                idempotency_payload: "length=61",
            }],
            &expected_revision,
            &event,
        )
        .expect_err("injected BREP failure rejects the whole publication");
    let _ = format!("{error:?}");

    let reloaded = bundle.open().expect("bundle reopens");
    assert_eq!(
        reloaded.history.event_ordinal(),
        before_ordinal,
        "no history transaction survives a failed BREP promotion"
    );
    assert_eq!(
        fs::read(root.join("brep/family-a.brep")).expect("BREP reads"),
        b"seed-brep",
        "no geometry survives a failed BREP promotion"
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log re-reads"),
        before_log,
        "no log entries survive a failed BREP promotion"
    );

    let _ = fs::remove_dir_all(root);
}

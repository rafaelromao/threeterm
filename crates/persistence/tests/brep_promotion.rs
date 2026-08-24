use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_persistence::{Bundle, BundleError};

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-persistence-brep-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

#[test]
fn revision_guarded_brep_promotion_publishes_log_and_bytes_together() {
    let root = temp_root("atomic");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let initial = bundle
        .append_feature_with_brep_if_revision(
            "l-bracket",
            "bracket:length=100.00000000000000000;thickness=5.00000000000000000",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"old-brep",
        )
        .expect("initial promotion succeeds");
    let before = fs::read(root.join("brep/l-bracket.brep")).expect("BREP reads");
    let committed = bundle
        .append_feature_with_brep_if_revision(
            "l-bracket",
            "bracket:length=110.00000000000000000;thickness=5.00000000000000000",
            initial.revision_hash_hex(),
            b"new-brep",
        )
        .expect("parameter replacement succeeds");
    assert_ne!(committed.revision_hash_hex(), initial.revision_hash_hex());
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        b"new-brep"
    );
    let log = fs::read_to_string(root.join("transactions.log")).expect("log reads");
    assert!(log.contains("110.00000000000000000"));

    let stale = bundle.append_feature_with_brep_if_revision(
        "l-bracket",
        "bracket:length=120.00000000000000000;thickness=5.00000000000000000",
        initial.revision_hash_hex(),
        b"must-not-publish",
    );
    assert!(matches!(stale, Err(BundleError::Invalid(_))));
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        b"new-brep"
    );
    assert_ne!(before, b"new-brep");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn parameterized_idempotency_retries_only_the_same_payload() {
    let root = temp_root("idempotency");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let initial = bundle
        .append_feature_with_brep_if_revision(
            "l-bracket",
            "bracket:length=100.00000000000000000;thickness=5.00000000000000000",
            &bundle.open().expect("bundle opens").revision_hash_hex(),
            b"old-brep",
        )
        .expect("initial promotion succeeds");
    let committed = bundle
        .append_feature_with_brep_if_revision_and_source_and_idempotency_payload(
            "l-bracket",
            "bracket:length=110.00000000000000000;thickness=5.00000000000000000",
            initial.revision_hash_hex(),
            "664ced6fc3297e324b3998958492b5338635f4b887100a012eaae3c4d9733889",
            Some("draft-1"),
            Some("semantic-a"),
            b"new-brep",
        )
        .expect("parameterized promotion succeeds");
    let retry = bundle
        .append_feature_with_brep_if_revision_and_source_and_idempotency_payload(
            "l-bracket",
            "bracket:length=110.00000000000000000;thickness=5.00000000000000000",
            initial.revision_hash_hex(),
            "664ced6fc3297e324b3998958492b5338635f4b887100a012eaae3c4d9733889",
            Some("draft-1"),
            Some("semantic-a"),
            b"new-brep",
        )
        .expect("same payload retries idempotently");
    assert_eq!(retry.revision_hash_hex(), committed.revision_hash_hex());

    let conflict = bundle
        .append_feature_with_brep_if_revision_and_source_and_idempotency_payload(
            "l-bracket",
            "bracket:length=120.00000000000000000;thickness=5.00000000000000000",
            initial.revision_hash_hex(),
            "664ced6fc3297e324b3998958492b5338635f4b887100a012eaae3c4d9733889",
            Some("draft-1"),
            Some("semantic-b"),
            b"other-brep",
        )
        .expect_err("same key with a different payload is rejected");
    assert!(matches!(conflict, BundleError::Invalid(_)));
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        b"new-brep"
    );
    let _ = fs::remove_dir_all(root);
}

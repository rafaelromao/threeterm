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

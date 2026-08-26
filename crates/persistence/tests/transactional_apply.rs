use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_persistence::{Bundle, BundleError, MANIFEST_FILENAME, TRANSACTIONS_LOG_FILENAME};

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-apply-{label}-{suffix}"))
}

#[test]
fn add_set_and_remove_are_one_revision_bound_transaction_each() {
    let root = root("accepted");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let initial = bundle.open().expect("empty bundle opens");

    let added = bundle
        .apply_feature_if_revision("add", "box", Some("cube"), initial.revision_hash_hex())
        .expect("add commits");
    assert_eq!(added.log.len(), 1);
    assert_eq!(added.log.entries()[0].operation.as_deref(), Some("add"));
    let added_revision = added.revision_hash_hex().to_string();

    let same_kind = bundle
        .apply_feature_if_revision("set", "box", Some("cube"), &added_revision)
        .expect("same-kind set commits");
    assert_eq!(same_kind.log.len(), 2);

    let set = bundle
        .apply_feature_if_revision("set", "box", Some("sphere"), same_kind.revision_hash_hex())
        .expect("set commits");
    assert_eq!(set.log.entries()[2].operation.as_deref(), Some("set"));
    assert_eq!(
        set.graph.features().next().expect("feature exists").kind,
        "sphere"
    );
    let set_revision = set.revision_hash_hex().to_string();

    let removed = bundle
        .apply_feature_if_revision("remove", "box", None, &set_revision)
        .expect("remove commits");
    assert_eq!(
        removed.log.entries()[3].operation.as_deref(),
        Some("remove")
    );
    assert!(removed.graph.features().next().is_none());
    let readded = bundle
        .apply_feature_if_revision("add", "box", Some("cube"), removed.revision_hash_hex())
        .expect("removed feature can be added again");
    assert_eq!(readded.log.len(), 5);
    assert_eq!(bundle.open().expect("reload succeeds").graph, readded.graph);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

#[test]
fn invalid_and_stale_apply_leave_canonical_bytes_unchanged() {
    let root = root("rejected");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let initial = bundle.open().expect("empty bundle opens");
    let committed = bundle
        .apply_feature_if_revision("add", "box", Some("cube"), initial.revision_hash_hex())
        .expect("add commits");
    let before_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
    let before_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");

    let duplicate = bundle.apply_feature_if_revision(
        "add",
        "box",
        Some("other"),
        committed.revision_hash_hex(),
    );
    assert!(matches!(duplicate, Err(BundleError::Invalid(_))));
    let stale =
        bundle.apply_feature_if_revision("remove", "box", None, initial.revision_hash_hex());
    assert!(matches!(stale, Err(BundleError::Invalid(_))));
    assert_eq!(
        fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
        before_manifest
    );
    assert_eq!(
        fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
        before_log
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_domain::history::HistoryState;
use threeterm_persistence::{
    Bundle, BundleError, PublicationFailurePoint, fail_next_publication_at,
};

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
        .append_feature_with_brep_if_revision_and_source_and_idempotency_payload(
            "l-bracket",
            "bracket:length=110.00000000000000000;thickness=5.00000000000000000",
            initial.revision_hash_hex(),
            "664ced6fc3297e324b3998958492b5338635f4b887100a012eaae3c4d9733889",
            Some("parameter-edit-1"),
            Some("length=110"),
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
    let source_revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let initial = bundle
        .append_feature_with_brep_if_revision(
            "l-bracket",
            "bracket:length=100.00000000000000000;thickness=5.00000000000000000",
            &source_revision,
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

#[test]
fn brep_target_failures_leave_the_prior_generation_byte_identical_and_retryable() {
    for (label, point) in [
        ("copy", PublicationFailurePoint::BrepCopy),
        ("write", PublicationFailurePoint::BrepWrite),
        ("sync", PublicationFailurePoint::BrepSync),
        ("rename", PublicationFailurePoint::BrepRename),
        ("directory-sync", PublicationFailurePoint::BrepDirectorySync),
        ("manifest", PublicationFailurePoint::ManifestSync),
    ] {
        let root = temp_root(label);
        let bundle = Bundle::create(&root).expect("bundle creates");
        let empty_revision = bundle
            .open()
            .expect("bundle opens")
            .revision_hash_hex()
            .to_string();
        bundle
            .append_feature_with_brep_if_revision("seed", "box", &empty_revision, b"prior-brep")
            .expect("initial geometry publishes");
        let source_revision = bundle
            .open()
            .expect("bundle opens")
            .revision_hash_hex()
            .to_string();
        let prior_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
        let prior_log = fs::read(root.join("transactions.log")).expect("log reads");

        fail_next_publication_at(point);
        assert!(
            bundle
                .append_feature_with_brep_if_revision(
                    "extrude-1",
                    "brep:extrude-1",
                    &source_revision,
                    b"new-brep",
                )
                .is_err()
        );
        assert_eq!(
            fs::read(root.join("manifest.json")).unwrap(),
            prior_manifest
        );
        assert_eq!(fs::read(root.join("transactions.log")).unwrap(), prior_log);
        assert!(!root.join("brep/extrude-1.brep").exists());
        assert_eq!(
            fs::read(root.join("brep/seed.brep")).unwrap(),
            b"prior-brep"
        );
        assert!(!root.join("brep/.extrude-1.brep.tmp").exists());

        bundle
            .append_feature_with_brep_if_revision(
                "extrude-1",
                "brep:extrude-1",
                &source_revision,
                b"new-brep",
            )
            .expect("retry succeeds after target failure");
        assert_eq!(
            fs::read(root.join("brep/extrude-1.brep")).unwrap(),
            b"new-brep"
        );
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn brep_parent_sync_failure_reconciles_to_the_selected_generation() {
    let root = temp_root("parent-sync");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let prior = bundle
        .append_feature_with_brep_if_revision("seed", "box", &source_revision, b"prior-brep")
        .expect("prior geometry publishes");
    let prior_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let prior_log = fs::read(root.join("transactions.log")).expect("log reads");

    fail_next_publication_at(PublicationFailurePoint::ParentSync);
    assert!(
        bundle
            .append_feature_with_brep_if_revision(
                "extrude-1",
                "brep:extrude-1",
                prior.revision_hash_hex(),
                b"new-brep",
            )
            .is_err()
    );

    let selected = bundle.open().expect("selected generation opens");
    assert_eq!(
        fs::read(root.join("brep/extrude-1.brep")).unwrap(),
        b"new-brep"
    );
    assert_ne!(
        fs::read(root.join("manifest.json")).unwrap(),
        prior_manifest
    );
    assert_ne!(fs::read(root.join("transactions.log")).unwrap(), prior_log);
    let previous = threeterm_persistence::previous_generation_path(&root);
    assert_eq!(
        fs::read(previous.join("brep/seed.brep")).unwrap(),
        b"prior-brep"
    );
    assert_eq!(
        selected.revision_hash_hex(),
        Bundle::at(&root).open().unwrap().revision_hash_hex()
    );
    let _ = fs::remove_dir_all(root);
}
#[test]
fn duplicate_brep_feature_id_is_rejected_before_replacing_the_generation() {
    let root = temp_root("duplicate-feature");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let initial = bundle
        .append_feature_with_brep_if_revision(
            "solid-1",
            "brep:solid-1",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"prior-brep",
        )
        .expect("initial promotion succeeds");
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let before_brep = fs::read(root.join("brep/solid-1.brep")).expect("BREP reads");

    let duplicate = bundle.append_feature_with_brep_if_revision(
        "solid-1",
        "brep:solid-1",
        initial.revision_hash_hex(),
        b"replacement-brep",
    );
    assert!(matches!(duplicate, Err(BundleError::Invalid(_))));
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(
        fs::read(root.join("brep/solid-1.brep")).unwrap(),
        before_brep
    );
    assert_eq!(
        bundle.open().unwrap().revision_hash_hex(),
        initial.revision_hash_hex()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loading_rejects_tampered_promoted_brep_bytes() {
    let root = temp_root("tampered-brep");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let initial = bundle
        .append_feature_with_brep_if_revision(
            "solid-1",
            "brep:solid-1",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"prior-brep",
        )
        .expect("promotion succeeds");
    assert_eq!(
        initial.revision_hash_hex(),
        bundle.open().unwrap().revision_hash_hex()
    );

    fs::write(root.join("brep/solid-1.brep"), b"tampered-brep").expect("BREP tampers");
    assert!(
        bundle.open().is_err(),
        "tampered BREP must fail closed on load"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loading_rejects_symlinked_promoted_brep() {
    use std::os::unix::fs::symlink;

    let root = temp_root("symlinked-brep");
    let bundle = Bundle::create(&root).expect("bundle creates");
    bundle
        .append_feature_with_brep_if_revision(
            "solid-1",
            "brep:solid-1",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"prior-brep",
        )
        .expect("promotion succeeds");

    let target = root.join("brep/solid-1.brep");
    let replacement = root.join("replacement.brep");
    fs::write(&replacement, b"prior-brep").expect("replacement writes");
    fs::remove_file(&target).expect("committed BREP removes");
    symlink(&replacement, &target).expect("BREP symlink creates");

    assert!(
        bundle.open().is_err(),
        "symlinked BREP must fail closed on load"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loading_rejects_a_symlinked_brep_directory() {
    use std::os::unix::fs::symlink;

    let root = temp_root("symlinked-brep-directory");
    let bundle = Bundle::create(&root).expect("bundle creates");
    bundle
        .append_feature_with_brep_if_revision(
            "solid-1",
            "brep:solid-1",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"prior-brep",
        )
        .expect("promotion succeeds");

    let external = temp_root("external-brep-directory");
    fs::create_dir_all(&external).expect("external directory creates");
    fs::write(external.join("solid-1.brep"), b"prior-brep").expect("external BREP writes");
    let committed_dir = root.join("brep");
    let retained_dir = root.join("brep-retained");
    fs::rename(&committed_dir, &retained_dir).expect("BREP directory moves");
    symlink(&external, &committed_dir).expect("BREP directory symlink creates");

    assert!(
        bundle.open().is_err(),
        "a symlinked BREP directory must fail closed on load"
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(external);
}

#[test]
fn loading_rejects_a_fifo_substituted_for_a_promoted_brep() {
    let root = temp_root("fifo-brep");
    let bundle = Bundle::create(&root).expect("bundle creates");
    bundle
        .append_feature_with_brep_if_revision(
            "solid-1",
            "brep:solid-1",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"prior-brep",
        )
        .expect("promotion succeeds");

    let target = root.join("brep/solid-1.brep");
    fs::remove_file(&target).expect("committed BREP removes");
    let status = std::process::Command::new("mkfifo")
        .arg("-m")
        .arg("600")
        .arg(&target)
        .status()
        .expect("mkfifo runs");
    assert!(status.success(), "FIFO creates");

    let (sender, receiver) = std::sync::mpsc::channel();
    let load_root = root.clone();
    std::thread::spawn(move || {
        sender.send(Bundle::at(load_root).open().is_err()).unwrap();
    });
    assert!(
        receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("FIFO load does not block")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn historically_used_feature_ids_are_rejected_without_replacement_authorization() {
    let root = temp_root("historical-feature-id");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let event = HistoryState::default()
        .initialize_l_bracket("bracket", 10.0, 5.0, 3.0, 1.0)
        .expect("history event creates");
    let history = bundle
        .append_features_with_history(&[], &event)
        .expect("history event publishes");

    let duplicate = bundle.append_feature_with_brep_if_revision(
        "history-event-0",
        "brep:history-event-0",
        history.revision_hash_hex(),
        b"replacement-brep",
    );
    assert!(matches!(duplicate, Err(BundleError::Invalid(_))));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loading_rejects_tampered_brep_provenance_metadata() {
    let root = temp_root("tampered-brep-metadata");
    let bundle = Bundle::create(&root).expect("bundle creates");
    bundle
        .append_feature_with_brep_if_revision(
            "solid-1",
            "brep:solid-1",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"prior-brep",
        )
        .expect("promotion succeeds");

    let log_path = root.join("transactions.log");
    let log = fs::read_to_string(&log_path).expect("log reads");
    let tampered = log.replace("\"brep_byte_count\":10", "\"brep_byte_count\":11");
    assert_ne!(tampered, log, "test must change authenticated metadata");
    fs::write(log_path, tampered).expect("tampered log writes");

    assert!(
        bundle.open().is_err(),
        "tampered BREP provenance must fail closed on load"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_brep_kind_without_provenance_remains_loadable() {
    let root = temp_root("legacy-brep-kind");
    let bundle = Bundle::create(&root).expect("bundle creates");
    bundle
        .append_feature("legacy", "brep:legacy")
        .expect("legacy feature publishes");
    fs::create_dir_all(root.join("brep")).expect("BREP directory creates");
    fs::write(root.join("brep/legacy.brep"), b"legacy-brep").expect("legacy BREP writes");

    assert!(bundle.open().is_ok(), "legacy BREP entries remain loadable");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn new_brep_feature_rejects_duplicate_ids_without_replacing_state() {
    let root = temp_root("duplicate-new-feature");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let empty_revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let initial = bundle
        .append_feature_with_brep_if_revision(
            "feature-1",
            "brep:feature-1",
            &empty_revision,
            b"prior-brep",
        )
        .expect("initial BREP publishes");
    let prior_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let prior_log = fs::read(root.join("transactions.log")).expect("log reads");
    let prior_brep = fs::read(root.join("brep/feature-1.brep")).expect("BREP reads");

    let error = bundle
        .append_new_feature_with_brep_if_revision(
            "feature-1",
            "brep:replacement",
            initial.revision_hash_hex(),
            b"replacement-brep",
        )
        .expect_err("a new feature commit must reject a duplicate ID");
    assert!(error.to_string().contains("already exists"));
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        prior_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), prior_log);
    assert_eq!(
        fs::read(root.join("brep/feature-1.brep")).unwrap(),
        prior_brep
    );
    assert_eq!(
        bundle.open().unwrap().revision_hash_hex(),
        initial.revision_hash_hex()
    );
    let _ = fs::remove_dir_all(root);
}

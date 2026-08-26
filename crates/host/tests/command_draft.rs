use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{
    BracketRequest, DraftRequest, ExtrudeRequest, OcctWorker, new_request_id,
};
use threeterm_persistence::Bundle;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-command-draft-{label}-{nanos}"))
}

fn skip_without_worker() -> Option<OcctWorker> {
    match OcctWorker::locate() {
        Ok(worker) => Some(worker),
        Err(error) => {
            eprintln!("command_draft: OCCT worker unavailable: {error}");
            None
        }
    }
}

fn seed_solid(host: &Host, root: &Path, worker: &OcctWorker) -> String {
    Bundle::create(root).expect("bundle creates");
    let request = ExtrudeRequest::new(
        new_request_id(),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "seed.brep")
    .with_feature_id("l-bracket");
    host.extrude(root, request, worker)
        .expect("seed solid commits");
    "l-bracket".to_string()
}

#[test]
fn command_draft_preview_is_revision_bound_and_non_mutating() {
    let Some(worker) = skip_without_worker() else {
        return;
    };
    let root = temp_root("preview");
    let host = Host::new();
    let feature_id = seed_solid(&host, &root, &worker);
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let before_brep = fs::read(root.join("brep/l-bracket.brep")).expect("brep reads");
    let source_revision = host.current().expect("current snapshot").revision_hash;
    let request = DraftRequest::new(
        new_request_id(),
        root.join("brep/l-bracket.brep"),
        std::f64::consts::FRAC_PI_2 / 12.0,
        [0.0, 0.0, 1.0],
    )
    .with_output_path(root.join("caller-controlled-stage"), "preview.brep")
    .with_feature_id("l-bracket-preview");

    let draft = host
        .open_draft(&root, "draft-preview", feature_id, request)
        .expect("draft opens");
    assert_eq!(draft.source_revision, source_revision);
    let preview = host
        .preview_draft(&root, &draft.draft_id, &worker)
        .expect("preview succeeds");

    assert_eq!(preview.source_revision, source_revision);
    assert!(!preview.preview_revision.is_empty());
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        before_brep
    );
    assert!(!preview.brep_path.starts_with(&root));

    host.discard_draft(&root, &draft.draft_id)
        .expect("discard removes draft");
    assert!(!host.has_draft(&root, &draft.draft_id));
    assert!(!preview.brep_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn generic_draft_ids_are_scoped_to_their_canonical_bundle_root() {
    let Some(worker) = skip_without_worker() else {
        return;
    };
    let first_root = temp_root("generic-scope-first");
    let second_root = temp_root("generic-scope-second");
    let host = Host::new();
    let first_feature = seed_solid(&host, &first_root, &worker);
    let second_feature = seed_solid(&host, &second_root, &worker);
    let request = |root: &Path, feature_id: &str| {
        DraftRequest::new(
            new_request_id(),
            root.join(format!("brep/{feature_id}.brep")),
            std::f64::consts::FRAC_PI_2 / 12.0,
            [0.0, 0.0, 1.0],
        )
        .with_feature_id(feature_id)
    };

    host.open_draft(
        &first_root,
        "shared-draft",
        &first_feature,
        request(&first_root, &first_feature),
    )
    .expect("first bundle draft opens");
    host.open_draft(
        &second_root,
        "shared-draft",
        &second_feature,
        request(&second_root, &second_feature),
    )
    .expect("second bundle draft opens independently");

    assert!(host.has_draft(&first_root, "shared-draft"));
    assert!(host.has_draft(&second_root, "shared-draft"));
    host.discard_draft(&first_root, "shared-draft")
        .expect("first draft discards");
    host.discard_draft(&second_root, "shared-draft")
        .expect("second draft discards");
    let _ = fs::remove_dir_all(first_root);
    let _ = fs::remove_dir_all(second_root);
}

#[test]
fn l_bracket_parameter_preview_commit_and_refuse_use_one_canonical_path() {
    let Some(worker) = skip_without_worker() else {
        return;
    };
    let root = temp_root("bracket-lifecycle");
    let host = Host::new();
    Bundle::create(&root).expect("bundle creates");
    let initial =
        BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 5.0).with_feature_id("l-bracket");
    let before = host
        .create_bracket(&root, initial, &worker)
        .expect("initial L-bracket commits");
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let before_brep = fs::read(root.join("brep/l-bracket.brep")).expect("brep reads");

    let edit =
        BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 4.0).with_feature_id("l-bracket");
    let draft = host
        .open_bracket_parameter_draft(&root, "bracket-edit-1", "l-bracket", edit)
        .expect("parameter draft opens");
    let preview = host
        .preview_bracket_parameter_draft(&root, &draft.draft_id, &worker)
        .expect("parameter preview succeeds");
    assert_eq!(preview.source_revision, before.snapshot.revision_hash);
    assert_ne!(preview.preview_revision, before.snapshot.revision_hash);
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        before_brep
    );
    assert!(!preview.brep_path.starts_with(&root));

    let committed = host
        .commit_bracket_parameter_draft(&root, &draft.draft_id, &worker)
        .expect("parameter commit succeeds");
    assert_ne!(
        committed.snapshot.revision_hash,
        before.snapshot.revision_hash
    );
    assert_eq!(committed.input_fingerprint, preview.input_fingerprint);
    assert!(!host.has_bracket_parameter_draft(&root, &draft.draft_id));
    assert_ne!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        before_brep
    );
    let committed_log = fs::read_to_string(root.join("transactions.log")).expect("log reads");
    assert!(committed_log.contains("bracket:length=100.00000000000000000"));

    let retry = host
        .open_bracket_parameter_draft(
            &root,
            "bracket-edit-1",
            "l-bracket",
            BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 4.0),
        )
        .expect("same idempotency key can be retried");
    let retried = host
        .commit_bracket_parameter_draft(&root, &retry.draft_id, &worker)
        .expect("idempotent retry returns the sealed commit");
    assert_eq!(
        retried.snapshot.revision_hash,
        committed.snapshot.revision_hash
    );
    assert_eq!(retried.input_fingerprint, committed.input_fingerprint);

    let next = host
        .open_bracket_parameter_draft(
            &root,
            "bracket-edit-next",
            "l-bracket",
            BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 3.0),
        )
        .expect("next draft opens");
    host.commit_bracket_parameter_draft(&root, &next.draft_id, &worker)
        .expect("next commit succeeds");
    let reused = host
        .open_bracket_parameter_draft(
            &root,
            "bracket-edit-1",
            "l-bracket",
            BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 4.0),
        )
        .expect("old key can be inspected after a later revision");
    assert!(matches!(
        host.commit_bracket_parameter_draft(&root, &reused.draft_id, &worker),
        Err(HostError::DraftIdempotencyConflict { .. })
    ));

    let refused_before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let refused_before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let refused_before_brep = fs::read(root.join("brep/l-bracket.brep")).expect("brep reads");
    let refused = host
        .open_bracket_parameter_draft(
            &root,
            "bracket-edit-refused",
            "l-bracket",
            BracketRequest::new(new_request_id(), 110.0, 60.0, 40.0, 4.0),
        )
        .expect("refused draft opens");
    let refused_preview = host
        .preview_bracket_parameter_draft(&root, &refused.draft_id, &worker)
        .expect("refused preview succeeds");
    host.discard_bracket_parameter_draft(&root, &refused.draft_id)
        .expect("refusal discards the draft");
    assert!(!refused_preview.brep_path.exists());
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        refused_before_manifest
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).unwrap(),
        refused_before_log
    );
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        refused_before_brep
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn duplicate_initial_bracket_is_rejected_without_replacing_committed_state() {
    let Some(worker) = skip_without_worker() else {
        return;
    };
    let root = temp_root("duplicate-initial-bracket");
    let host = Host::new();
    Bundle::create(&root).expect("bundle creates");
    let initial =
        BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 5.0).with_feature_id("l-bracket");
    host.create_bracket(&root, initial, &worker)
        .expect("initial L-bracket commits");
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let before_brep = fs::read(root.join("brep/l-bracket.brep")).expect("BREP reads");

    let duplicate =
        BracketRequest::new(new_request_id(), 110.0, 60.0, 40.0, 4.0).with_feature_id("l-bracket");
    assert!(host.create_bracket(&root, duplicate, &worker).is_err());

    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        before_brep
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bracket_parameter_edit_rejects_a_non_bracket_brep_without_replacing_state() {
    let Some(worker) = skip_without_worker() else {
        return;
    };
    let root = temp_root("non-bracket-edit");
    let host = Host::new();
    let feature_id = seed_solid(&host, &root, &worker);
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    let before_brep = fs::read(root.join("brep/l-bracket.brep")).expect("BREP reads");
    let request = BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 4.0);
    let draft = host
        .open_bracket_parameter_draft(&root, "non-bracket-edit", feature_id, request)
        .expect("draft opens for the existing BREP");

    assert!(
        host.commit_bracket_parameter_draft(&root, &draft.draft_id, &worker)
            .is_err()
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        before_brep
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bracket_draft_ids_are_scoped_to_their_bundle_root() {
    let Some(worker) = skip_without_worker() else {
        return;
    };
    let root_a = temp_root("bracket-scope-a");
    let root_b = temp_root("bracket-scope-b");
    let host = Host::new();
    seed_solid(&host, &root_a, &worker);
    seed_solid(&host, &root_b, &worker);

    let draft_id = "same-draft-id";
    let request_a = BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 4.0);
    let request_b = BracketRequest::new(new_request_id(), 110.0, 60.0, 40.0, 4.0);
    host.open_bracket_parameter_draft(&root_a, draft_id, "l-bracket", request_a)
        .expect("first scoped draft opens");
    let draft_b = host
        .open_bracket_parameter_draft(&root_b, draft_id, "l-bracket", request_b)
        .expect("second scoped draft opens");

    assert!(host.has_bracket_parameter_draft(&root_a, draft_id));
    assert!(host.has_bracket_parameter_draft(&root_b, draft_id));
    let preview_a = host
        .preview_bracket_parameter_draft(&root_a, draft_id, &worker)
        .expect("first scoped preview succeeds");
    let preview_b = host
        .preview_bracket_parameter_draft(&root_b, &draft_b.draft_id, &worker)
        .expect("second scoped preview succeeds");
    assert_ne!(preview_a.brep_path, preview_b.brep_path);
    host.discard_bracket_parameter_draft(&root_a, draft_id)
        .expect("first scoped draft discards");
    assert!(!host.has_bracket_parameter_draft(&root_a, draft_id));
    assert!(host.has_bracket_parameter_draft(&root_b, draft_id));
    host.discard_bracket_parameter_draft(&root_b, draft_id)
        .expect("second scoped draft discards");

    let _ = fs::remove_dir_all(root_a);
    let _ = fs::remove_dir_all(root_b);
}

#[test]
fn stale_bracket_draft_returns_structured_revision_diagnostic_without_worker_use() {
    let root = temp_root("stale");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let initial = bundle
        .append_feature_with_brep_if_revision(
            "l-bracket",
            "bracket:length=100.00000000000000000;thickness=5.00000000000000000",
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
            b"canonical-brep",
        )
        .expect("initial bracket persists");
    let host = Host::new();
    let draft = host
        .open_bracket_parameter_draft(
            &root,
            "stale-draft",
            "l-bracket",
            BracketRequest::new(new_request_id(), 110.0, 60.0, 40.0, 5.0),
        )
        .expect("draft opens");
    host.save(&root, "unrelated", "marker")
        .expect("canonical revision advances");
    let error = host
        .preview_bracket_parameter_draft(
            &root,
            &draft.draft_id,
            &OcctWorker::with_binary_path(PathBuf::from("missing-worker")),
        )
        .expect_err("stale draft is rejected before worker invocation");
    assert!(matches!(
        error,
        HostError::DraftStale {
            draft_id,
            source_revision,
            current_revision,
            recovery: "discard_and_reopen",
        } if draft_id == "stale-draft"
            && source_revision == initial.revision_hash_hex()
            && current_revision != source_revision
    ));
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).unwrap(),
        b"canonical-brep"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bracket_draft_update_rejects_stale_sequence_without_mutating_newer_values() {
    let root = temp_root("sequence");
    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    bundle
        .append_feature_with_brep_if_revision(
            "l-bracket",
            "bracket:length=100.00000000000000000;thickness=5.00000000000000000",
            &source_revision,
            b"canonical-brep",
        )
        .expect("initial bracket persists");
    let host = Host::new();
    let draft = host
        .open_bracket_parameter_draft(
            &root,
            "sequence-draft",
            "l-bracket",
            BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 5.0),
        )
        .expect("draft opens");
    let draft_fingerprint = host
        .bracket_draft_fingerprint(&root, &draft.draft_id)
        .expect("draft fingerprint exists");

    let updated = host
        .update_bracket_parameter_draft(
            &root,
            &draft.draft_id,
            0,
            &draft_fingerprint,
            BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 4.0),
        )
        .expect("first update succeeds");
    assert_eq!(updated.sequence, 1);

    let error = host
        .update_bracket_parameter_draft(
            &root,
            &draft.draft_id,
            0,
            &draft_fingerprint,
            BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 3.0),
        )
        .expect_err("stale update is rejected");
    assert!(matches!(
        error,
        HostError::DraftSequenceConflict {
            draft_id,
            expected: 0,
            current: 1,
        } if draft_id == "sequence-draft"
    ));

    let current = host
        .bracket_draft_sequence(&root, &draft.draft_id)
        .expect("draft remains after conflict");
    assert_eq!(current, 1);
    host.discard_bracket_parameter_draft(&root, &draft.draft_id)
        .expect("draft discards");
    let _ = fs::remove_dir_all(root);
}

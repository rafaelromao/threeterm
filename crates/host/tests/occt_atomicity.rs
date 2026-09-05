//! Atomicity and end-to-end tests for the host's `extrude` and
//! `boolean_fuse` methods.
//!
//! These tests exercise the real worker binary through the
//! `OcctWorker` boundary so the production code path is the system
//! under test. They assert that every failure mode leaves the
//! canonical host state unchanged: the bundle's `manifest.json` and
//! `transactions.log` are byte-identical to a pre-call snapshot, and
//! `Host::current()` is preserved.
//!
//! When the worker binary is unavailable the tests soft-skip via
//! `OcctWorker::locate` returning `Err`; native E2E sets
//! `THREETERM_REQUIRE_OCCT=1` and requires the production worker.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{
    BooleanFuseRequest, BooleanPatternRequest, ChamferRequest, CircularPatternRequest,
    DraftRequest, ExtrudeMode, ExtrudeRequest, FilletRequest, HoleRequest, LinearPatternRequest,
    LoftRequest, MirrorRequest, Operation, ShellRequest,
};
use threeterm_persistence::{
    Bundle, CanonicalExtrudeIntent, CanonicalIntent, EXTRUDE_INTENT_SCHEMA_VERSION,
    ExtrudeDeterministicInputs, MANIFEST_FILENAME, PublicationFailurePoint,
    TRANSACTIONS_LOG_FILENAME, fail_next_publication_at,
};
use threeterm_protocol::artifact::sha256_hex;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-host-occt-{label}-{}-{nanos}",
        std::process::id(),
    ))
}

fn unique_request_id(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{label}-{nanos}-{}", std::process::id())
}

#[test]
fn subtractive_extrude_rejects_a_missing_semantic_target_without_mutation() {
    let root = temp_root("subtractive-missing-target");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let before_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
    let before_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");
    let before = host.load(&root).expect("snapshot loads");
    let worker = threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from(
        "/missing/threeterm-occt-worker",
    ));

    let error = host
        .extrude(
            &root,
            ExtrudeRequest::new(
                unique_request_id("missing-target"),
                vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
                1.0,
            )
            .with_mode(ExtrudeMode::Subtractive)
            .with_target_feature_id("does-not-exist")
            .with_feature_id("cut"),
            &worker,
        )
        .expect_err("missing semantic target must fail closed");
    assert!(matches!(error, HostError::Validation { .. }));
    assert_eq!(
        fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
        before_manifest
    );
    assert_eq!(
        fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
        before_log
    );
    assert_eq!(host.load(&root).unwrap(), before);
    let _ = fs::remove_dir_all(root);
}

fn locate_worker() -> Option<threeterm_occt_worker::OcctWorker> {
    threeterm_occt_worker::OcctWorker::locate().ok()
}

fn required_fixture_worker(test_name: &str) -> Option<threeterm_occt_worker::OcctWorker> {
    let worker = locate_worker();
    if worker.is_none() && std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() {
        panic!("{test_name}: OCCT worker is required in this environment");
    }
    worker
}

fn is_brep_invalid<T>(result: &Result<T, HostError>) -> bool {
    match result {
        Err(HostError::BrepInvalid { .. }) => true,
        Err(HostError::WorkerTerminated { record }) => {
            record.failed_code.as_deref() == Some("brep_invalid")
        }
        _ => false,
    }
}

/// Shell-script fake OCCT worker speaking the versioned envelope
/// protocol. Every fake emits the `worker_ready` handshake, consumes the
/// host's request envelope, and runs `reply` lines that may interpolate
/// `$request_id` (extracted from the request envelope). The fakes model
/// production failure modes without an OCCT install.
fn fake_worker_script(reply: &str) -> String {
    format!(
        "#!/bin/sh\n\
         printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fake\"}}'\n\
         read request_line\n\
         request_id=$(printf '%s' \"$request_line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
         {reply}\n"
    )
}

/// Failed-envelope reply for a fake worker. `code`/`detail` mirror the
/// structured `failed` envelope the real worker emits; the envelope is
/// bound to the request the supervisor actually sent.
fn fake_failed_reply(code: &str, detail: &str) -> String {
    format!(
        "printf '%s\\n' '{{\"kind\":\"failed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"'\"$request_id\"'\",\"code\":\"{code}\",\"detail\":\"{detail}\"}}'"
    )
}

fn fresh_bundle_with_feature(label: &str, feature_id: &str, kind: &str) -> PathBuf {
    let root = temp_root(label);
    let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
    bundle
        .append_feature(feature_id, kind)
        .expect("seed feature appends");
    root
}

fn fresh_bundle_with_brep(label: &str, feature_id: &str, kind: &str, bytes: &[u8]) -> PathBuf {
    let root = temp_root(label);
    let bundle = Bundle::create(&root).expect("bundle creates");
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    bundle
        .append_feature_with_brep_if_revision(feature_id, kind, &revision, bytes)
        .expect("seed BREP publishes");
    root
}

fn rectangle_extrude_request(label: &str) -> ExtrudeRequest {
    ExtrudeRequest::new(
        unique_request_id(label),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_feature_id(format!("{label}-box-1"))
}

fn triangle_extrude_request(label: &str, feature_id: &str) -> ExtrudeRequest {
    ExtrudeRequest::new(
        unique_request_id(label),
        vec![(0.0, 0.0), (4.0, 0.0), (2.0, 4.0)],
        2.0,
    )
    .with_feature_id(feature_id.to_string())
}

fn snapshot_files(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
    let log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");
    (manifest, log)
}

fn brep_inventory(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = fs::read_dir(root.join("brep"))
        .expect("brep directory reads")
        .map(|entry| {
            let path = entry.expect("brep entry reads").path();
            (
                path.file_name()
                    .expect("brep entry has a name")
                    .to_string_lossy()
                    .into_owned(),
                fs::read(path).expect("brep bytes read"),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

#[test]
fn extrude_commits_brep_into_a_new_revision() {
    let Some(worker) = required_fixture_worker(
        "subtractive_extrude_replays_and_exports_the_same_cut_after_derived_deletion",
    ) else {
        return;
    };
    let root = fresh_bundle_with_feature("commit", "box-seed", "box");
    let prior = Host::new().load(&root).expect("host loads prior");

    let request =
        rectangle_extrude_request("commit").with_output_path(root.join("stage"), "extrude.brep");
    let view = Host::new()
        .extrude(&root, request, &worker)
        .expect("extrude commits");

    assert!(view.snapshot.revision_hash != prior.revision_hash);
    assert!(view.snapshot.feature_graph_hash != prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Extrude);
    let brep_path = root.join("brep/commit-box-1.brep");
    assert!(brep_path.is_file(), "BREP is on disk at {brep_path:?}");
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn canonical_extrude_reloads_and_recomputes_after_derived_results_are_removed() {
    let Some(worker) =
        required_fixture_worker("invalid_subtractive_cut_preserves_the_prior_revision_snapshot")
    else {
        return;
    };
    let root = fresh_bundle_with_feature("replay", "box-seed", "box");
    let request =
        rectangle_extrude_request("replay").with_output_path(root.join("stage"), "unused.brep");
    let committed = Host::new()
        .extrude(&root, request, &worker)
        .expect("extrude commits");
    let original_brep = fs::read(&committed.result.brep_path).expect("committed BREP reads");
    Host::new()
        .load_with_extrude_replay(&root)
        .expect("intact extrude loads without replay");
    let manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
    let log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");
    let loaded = Bundle::at(&root)
        .open()
        .expect("canonical generation opens");
    let entry = loaded
        .log
        .entries()
        .last()
        .expect("extrude transaction exists");
    let intent = entry.intent.as_ref().expect("extrude intent persists");
    let CanonicalIntent::Extrude(intent) = intent else {
        panic!("expected extrude intent");
    };
    assert_eq!(intent.command, "extrude");
    assert_eq!(intent.operation, "additive");
    assert_eq!(intent.affected_semantic_ids, ["replay-box-1"]);
    assert_eq!(
        intent.source_revision,
        committed.source_snapshot.revision_hash
    );
    assert_eq!(intent.deterministic_inputs.height, 3.0);
    assert_eq!(intent.worker_requirements.worker_kind, "occt");

    fs::remove_file(&committed.result.brep_path).expect("promoted BREP removes");
    let _ = fs::remove_dir_all(root.join(".derived"));
    let _ = fs::remove_dir_all(root.join("cache"));
    Host::new()
        .load(&root)
        .expect("canonical project loads without BREP");

    let verification = Host::new()
        .verify_history_replay(&root)
        .expect("replay verification recomputes missing extrude geometry");
    assert!(verification.deterministic);
    assert_eq!(
        verification.geometry_fingerprints,
        [sha256_hex(&original_brep)]
    );

    let replayed = Host::new()
        .reload_and_recompute_extrudes(&root, &worker)
        .expect("extrude recomputes");
    assert_eq!(
        replayed.snapshot.revision_hash,
        committed.snapshot.revision_hash
    );
    assert_eq!(replayed.recomputed, 1);
    assert_eq!(replayed.geometry_fingerprints, [sha256_hex(&original_brep)]);
    assert_eq!(
        fs::read(&committed.result.brep_path).expect("replayed BREP reads"),
        original_brep
    );
    assert_eq!(
        fs::read(root.join(MANIFEST_FILENAME)).expect("manifest rereads"),
        manifest
    );
    assert_eq!(
        fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log rereads"),
        log
    );
    assert!(
        !Host::new()
            .load(&root)
            .expect("replayed project reloads")
            .recovered_from_previous
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_load_path_replays_missing_extrude_results() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("production-replay", "box-seed", "box");
    let committed = Host::new()
        .extrude(
            &root,
            rectangle_extrude_request("production-replay"),
            &worker,
        )
        .expect("extrude commits");
    let expected = fs::read(&committed.result.brep_path).expect("committed BREP reads");
    fs::remove_file(&committed.result.brep_path).expect("promoted BREP removes");
    let _ = fs::remove_dir_all(root.join(".derived"));
    let _ = fs::remove_dir_all(root.join("cache"));

    let loaded = Host::new()
        .load_with_extrude_replay(&root)
        .expect("production reload replays extrude");

    assert_eq!(loaded.revision_hash, committed.snapshot.revision_hash);
    assert_eq!(fs::read(&committed.result.brep_path).unwrap(), expected);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn subtractive_extrude_replays_and_exports_the_same_cut_after_derived_deletion() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("subtractive-replay", "seed", "box");
    let host = Host::new();
    host.extrude(
        &root,
        rectangle_extrude_request("subtractive-base")
            .with_feature_id("base")
            .with_output_path(root.join("stage"), "base.brep"),
        &worker,
    )
    .expect("base solid commits");
    let committed = host
        .extrude(
            &root,
            ExtrudeRequest::new(
                "subtractive-cut",
                vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)],
                2.0,
            )
            .with_feature_id("cut")
            .with_mode(ExtrudeMode::Subtractive)
            .with_target_feature_id("base"),
            &worker,
        )
        .expect("subtractive solid commits");
    let original_cut = fs::read(&committed.result.brep_path).expect("cut BREP reads");
    let before_export = host
        .export(
            &root,
            "cut",
            &["step".to_string()],
            &root.join("export-before"),
            0.1,
            false,
            false,
            &[],
        )
        .expect("cut exports");
    let before_step =
        fs::read(before_export.artifacts.first().expect("STEP artifact")).expect("STEP reads");
    fs::remove_file(&committed.result.brep_path).expect("cut BREP removes");
    fs::remove_file(root.join("brep/base.brep")).expect("base BREP removes");
    let after_export = Host::new()
        .export(
            &root,
            "cut",
            &["step".to_string()],
            &root.join("export-after"),
            0.1,
            false,
            false,
            &[],
        )
        .expect("deleted cut reloads and exports");
    assert_eq!(
        fs::read(root.join("brep/cut.brep")).expect("replayed cut reads"),
        original_cut
    );
    assert_eq!(
        fs::read(
            after_export
                .artifacts
                .first()
                .expect("replayed STEP artifact")
        )
        .expect("replayed STEP reads"),
        before_step
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_subtractive_cut_preserves_the_prior_revision_snapshot() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("subtractive-invalid-cut", "seed", "box");
    let host = Host::new();
    let base = host
        .extrude(
            &root,
            rectangle_extrude_request("subtractive-invalid-base")
                .with_feature_id("base")
                .with_output_path(root.join("stage"), "base.brep"),
            &worker,
        )
        .expect("base solid commits");
    let before_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
    let before_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");
    let before = host.load(&root).expect("prior snapshot loads");
    let before_brep = fs::read(&base.result.brep_path).expect("base BREP reads");

    let error = host
        .extrude(
            &root,
            ExtrudeRequest::new(
                "subtractive-invalid-cut",
                vec![(20.0, 20.0), (22.0, 20.0), (22.0, 22.0), (20.0, 22.0)],
                3.0,
            )
            .with_feature_id("cut")
            .with_mode(ExtrudeMode::Subtractive)
            .with_target_feature_id("base"),
            &worker,
        )
        .expect_err("disjoint subtractive cut must fail");
    assert!(matches!(
        error,
        HostError::BrepInvalid { .. } | HostError::WorkerFailure { .. }
    ));
    assert_eq!(host.load(&root).expect("snapshot reloads"), before);
    assert_eq!(
        fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
        before_manifest
    );
    assert_eq!(
        fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
        before_log
    );
    assert_eq!(fs::read(&base.result.brep_path).unwrap(), before_brep);
    assert!(!root.join("brep/cut.brep").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn replay_reconstructs_persisted_extrude_inputs_through_a_worker_boundary() {
    let root = temp_root("replay-fake-worker");
    let worker_root = temp_root("replay-fake-worker-bin");
    fs::create_dir_all(&worker_root).expect("worker directory creates");
    let script = worker_root.join("occt-worker.sh");
    let bytes = b"replayable-brep";
    let digest = sha256_hex(bytes);
    fs::write(
        &script,
        format!(
            r##"#!/bin/sh
printf '%s\n' '{{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"occt"}}'
IFS= read -r request
request_id=$(printf '%s\n' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
source_revision_id=$(printf '%s\n' "$request" | sed -n 's/.*"source_revision_id":"\([^"]*\)".*/\1/p')
output_dir=$(printf '%s\n' "$request" | sed -n 's/.*"output_dir":"\([^"]*\)".*/\1/p')
output_filename=$(printf '%s\n' "$request" | sed -n 's/.*"output_filename":"\([^"]*\)".*/\1/p')
feature_id=$(printf '%s\n' "$request" | sed -n 's/.*"feature_id":"\([^"]*\)".*/\1/p')
printf '%s' 'replayable-brep' > "$output_dir/$output_filename"
printf '{{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"%s","result":{{"schema_version":"threeterm.workers.occt/1","request_id":"%s","source_revision_id":"%s","operation":"extrude","status":"ok","brep_path":"%s/%s","brep_sha256":"{digest}","brep_bytes":{bytes_len},"feature_id":"%s"}}}}\n' "$request_id" "$request_id" "$source_revision_id" "$output_dir" "$output_filename" "$feature_id"
"##,
            digest = digest,
            bytes_len = bytes.len(),
        ),
    )
    .expect("worker script writes");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
        .expect("worker script becomes executable");

    let bundle = Bundle::create(&root).expect("bundle creates");
    let source_revision = bundle
        .append_feature("box-seed", "box")
        .expect("seed appends")
        .revision_hash_hex()
        .to_string();
    let intent = CanonicalExtrudeIntent {
        schema_version: EXTRUDE_INTENT_SCHEMA_VERSION.to_string(),
        command: "extrude".to_string(),
        operation: "additive".to_string(),
        mode: "additive".to_string(),
        target_feature_id: None,
        request_id: "replay-request".to_string(),
        deterministic_inputs: ExtrudeDeterministicInputs {
            profile: vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            height: 2.0,
        },
        affected_semantic_ids: vec!["replay-solid".to_string()],
        source_revision,
        worker_requirements: threeterm_protocol::artifact::WorkerFingerprint {
            worker_kind: "occt".to_string(),
            worker_schema_version: "threeterm.workers.occt/1".to_string(),
            protocol_schema_version: "threeterm.protocol/1".to_string(),
        },
    };
    let committed = bundle
        .append_new_feature_with_brep_if_revision_and_provenance_and_intent(
            "replay-solid",
            "brep:replay-solid",
            intent.source_revision.as_str(),
            &intent.request_id,
            "{}",
            &intent,
            bytes,
        )
        .expect("canonical extrude appends");
    let manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
    let log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");
    fs::remove_file(root.join("brep/replay-solid.brep")).expect("derived BREP removes");

    let replayed = Host::new()
        .reload_and_recompute_extrudes(
            &root,
            &threeterm_occt_worker::OcctWorker::with_binary_path(script),
        )
        .expect("canonical extrude replays");
    assert_eq!(replayed.recomputed, 1);
    assert_eq!(
        replayed.snapshot.revision_hash,
        committed.revision_hash_hex()
    );
    assert_eq!(replayed.geometry_fingerprints, [digest]);
    assert_eq!(
        fs::read(root.join("brep/replay-solid.brep")).unwrap(),
        bytes
    );
    assert_eq!(fs::read(root.join(MANIFEST_FILENAME)).unwrap(), manifest);
    assert_eq!(fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).unwrap(), log);

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(worker_root);
}

#[test]
fn verified_brep_manifest_failure_preserves_the_prior_generation_and_can_retry() {
    let root = fresh_bundle_with_brep(
        "verified-manifest-failure",
        "box-seed",
        "box",
        b"prior-brep",
    );
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");
    let prior_files = snapshot_files(&root);
    let prior_breps = brep_inventory(&root);
    let stage = root.join("stage/verified-manifest.brep");
    fs::create_dir_all(stage.parent().expect("stage parent")).expect("stage creates");
    fs::write(&stage, b"new-brep").expect("staged BREP writes");
    let digest = threeterm_occt_worker::sha256_file(&stage).expect("staged BREP hashes");

    fail_next_publication_at(PublicationFailurePoint::ManifestSync);
    let result = host.commit_brep_feature_verified(
        &root,
        "manifest-failure-1",
        &stage,
        b"new-brep".len(),
        &digest,
    );
    assert!(
        matches!(result, Err(HostError::Persistence(_))),
        "got {result:?}"
    );
    assert_eq!(snapshot_files(&root), prior_files);
    assert_eq!(brep_inventory(&root), prior_breps);
    assert_eq!(host.current(), Some(prior.clone()));
    assert_eq!(Host::new().load(&root).expect("prior reloads"), prior);
    assert!(!stage.exists(), "failed worker stage is cleaned up");

    fs::create_dir_all(stage.parent().expect("stage parent")).expect("stage recreates");
    fs::write(&stage, b"new-brep").expect("retry BREP writes");
    let digest = threeterm_occt_worker::sha256_file(&stage).expect("retry BREP hashes");
    host.commit_brep_feature_verified(
        &root,
        "manifest-failure-1",
        &stage,
        b"new-brep".len(),
        &digest,
    )
    .expect("retry succeeds");
    assert_eq!(
        fs::read(root.join("brep/manifest-failure-1.brep")).unwrap(),
        b"new-brep"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn verified_brep_duplicate_feature_id_preserves_artifact_and_host_snapshot() {
    let root = fresh_bundle_with_brep("verified-duplicate", "box-seed", "box", b"prior-brep");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");
    let prior_files = snapshot_files(&root);
    let prior_breps = brep_inventory(&root);
    let stage = root.join("stage/duplicate.brep");
    fs::create_dir_all(stage.parent().expect("stage parent")).expect("stage creates");
    fs::write(&stage, b"replacement-brep").expect("staged BREP writes");
    let digest = threeterm_occt_worker::sha256_file(&stage).expect("staged BREP hashes");

    let result = host.commit_brep_feature_verified(
        &root,
        "box-seed",
        &stage,
        b"replacement-brep".len(),
        &digest,
    );
    assert!(
        matches!(result, Err(HostError::Validation { .. })),
        "got {result:?}"
    );
    assert_eq!(snapshot_files(&root), prior_files);
    assert_eq!(brep_inventory(&root), prior_breps);
    assert_eq!(host.current(), Some(prior.clone()));
    assert_eq!(Host::new().load(&root).expect("prior reloads"), prior);
    assert!(!stage.exists(), "rejected worker stage is cleaned up");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn verified_brep_parent_sync_failure_retains_the_promoted_generation() {
    let root = fresh_bundle_with_brep("verified-parent-sync", "box-seed", "box", b"prior-brep");
    let host = Host::new();
    host.load(&root).expect("host loads prior");
    let stage = root.join("stage/parent-sync.brep");
    fs::create_dir_all(stage.parent().expect("stage parent")).expect("stage creates");
    fs::write(&stage, b"new-brep").expect("staged BREP writes");
    let digest = threeterm_occt_worker::sha256_file(&stage).expect("staged BREP hashes");

    fail_next_publication_at(PublicationFailurePoint::ParentSync);
    let result = host.commit_brep_feature_verified(
        &root,
        "parent-sync-1",
        &stage,
        b"new-brep".len(),
        &digest,
    );
    assert!(
        matches!(result, Err(HostError::Persistence(_))),
        "got {result:?}"
    );
    assert_eq!(
        fs::read(root.join("brep/parent-sync-1.brep")).unwrap(),
        b"new-brep"
    );
    let current = Host::new()
        .load(&root)
        .expect("promoted generation reloads");
    assert_eq!(host.current(), Some(current.clone()));
    assert!(!stage.exists(), "worker stage is cleaned up");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cancelled_324_hole_boolean_pattern_preserves_the_revision_snapshot() {
    let Some(worker) = required_fixture_worker(
        "cancelled_324_hole_boolean_pattern_preserves_the_revision_snapshot",
    ) else {
        return;
    };
    let root = fresh_bundle_with_feature("boolean-pattern-cancel", "box-seed", "box");
    let host = Host::new();
    let base_request = ExtrudeRequest::new(
        unique_request_id("boolean-pattern-base"),
        vec![(0.0, 0.0), (120.0, 0.0), (120.0, 120.0), (0.0, 120.0)],
        20.0,
    )
    .with_output_path(root.join("stage"), "boolean-pattern-base.brep")
    .with_feature_id("boolean-pattern-base-1");
    host.extrude(&root, base_request, &worker)
        .expect("real OCCT base extrude");
    let before = host.load(&root).expect("load canonical snapshot");
    let before_files = snapshot_files(&root);
    let before_breps = brep_inventory(&root);
    let base_path = committed_brep_path(&root, "boolean-pattern-base-1");
    let request = BooleanPatternRequest::new(
        unique_request_id("boolean-pattern-cancel"),
        &base_path,
        [6.0, 6.0, -1.0],
        [6.0, 6.0],
        18,
        18,
        2.0,
    )
    .with_output_path(root.join("stage"), "boolean-pattern-cancelled.brep")
    .with_feature_id("boolean-pattern-cancelled-1");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut observed = false;
    let mut on_progress = |progress: &threeterm_protocol::supervisor::Progress| {
        if progress.stage.starts_with("boolean_pattern:") {
            observed = true;
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    };

    let result = host.boolean_pattern_with_cancel_and_progress(
        &root,
        request.clone(),
        &worker,
        &cancel,
        &mut on_progress,
    );
    let Err(HostError::WorkerTerminated { record }) = result else {
        panic!("expected structured cooperative cancellation, got {result:?}");
    };
    assert!(observed, "cancellation must follow real worker progress");
    assert_eq!(record.request_id, request.request_id);
    assert_eq!(record.cancel_reason.as_deref(), Some("cancelled by host"));
    assert_eq!(
        record.exit_kind,
        threeterm_protocol::supervisor::ExitKind::Cooperative
    );
    assert!(record.exit_signal.is_none());
    assert!(record.exit_code.is_none() || record.exit_code == Some(0));
    assert_eq!(snapshot_files(&root), before_files);
    assert_eq!(brep_inventory(&root), before_breps);
    assert_eq!(host.current(), Some(before.clone()));
    assert_eq!(
        Host::new().load(&root).expect("reload unchanged snapshot"),
        before
    );
    assert!(!root.join("stage/boolean-pattern-cancelled.brep").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let caller_output_dir = root.join("caller-output");
    fs::create_dir_all(&caller_output_dir).expect("caller output directory creates");
    let caller_output = caller_output_dir.join("existing.brep");
    let caller_bytes = b"caller-owned output";
    fs::write(&caller_output, caller_bytes).expect("caller output writes");
    let request = rectangle_extrude_request("spawn-fail")
        .with_output_path(&caller_output_dir, "existing.brep");
    let request_id = request.request_id.clone();
    let result = host.extrude(&root, request, &bad_worker);
    match &result {
        Err(HostError::WorkerFailure {
            request_id: Some(actual),
            ..
        }) => assert_eq!(actual, &request_id),
        Err(HostError::WorkerTerminated { .. }) => {}
        _ => panic!("expected ID-bearing worker failure; got {result:?}"),
    }

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest, "manifest must be unchanged");
    assert_eq!(prior_log, post_log, "log must be unchanged");
    assert_eq!(host.current(), Some(prior_view));
    assert_eq!(
        fs::read(&caller_output).expect("caller output remains"),
        caller_bytes
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn extrude_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // 2-vertex profile is rejected by the worker as request_malformed.
    let mut request = triangle_extrude_request("bad-req", "bad-req-box")
        .with_output_path(root.join("stage"), "extrude.brep");
    request.profile = vec![[0.0, 0.0], [1.0, 0.0]];
    let result = host.extrude(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn extrude_malformed_response_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("malformed", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // Build a tiny shell script that exits 0 with empty stdout — the
    // worker dies before completing the handshake. The supervisor fails
    // closed and the host classifies the result as a worker failure,
    // preserving canonical state.
    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-{}.sh",
        std::process::id()
    ));
    fs::write(&script, "#!/bin/sh\nexit 0\n").expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request =
        rectangle_extrude_request("malformed").with_output_path(root.join("stage"), "extrude.brep");
    let result = host.extrude(&root, request, &fake_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn extrude_non_zero_exit_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("non-zero", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-nz-{}.sh",
        std::process::id()
    ));
    fs::write(&script, "#!/bin/sh\nexit 7\n").expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request =
        rectangle_extrude_request("non-zero").with_output_path(root.join("stage"), "extrude.brep");
    let result = host.extrude(&root, request, &fake_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn extrude_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-brep-{}.sh",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = rectangle_extrude_request("brep-invalid")
        .with_output_path(root.join("stage"), "extrude.brep");
    let result = host.extrude(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn extrude_non_ok_typed_result_preserves_request_id() {
    let root = fresh_bundle_with_feature("non-ok-result", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let staged = root.join("stage/extrude.brep");
    fs::create_dir_all(staged.parent().expect("stage parent")).expect("stage creates");
    fs::write(&staged, []).expect("empty staged output writes");
    let digest = threeterm_occt_worker::sha256_file(&staged).expect("staged output hashes");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-non-ok-{}.sh",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\n",
            worker = fake_worker_script(&format!(
                "printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"'\"$request_id\"'\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"'\"$request_id\"'\",\"operation\":\"extrude\",\"status\":\"internal_error\",\"brep_path\":\"{}\",\"brep_sha256\":\"{}\",\"brep_bytes\":0,\"feature_id\":\"non-ok-result-box\"}}}}'",
                staged.display(), digest
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = rectangle_extrude_request("non-ok-result").with_feature_id("non-ok-result-box");
    let request_id = request.request_id.clone();
    let result = host.extrude(&root, request, &fake_worker);
    match &result {
        Err(HostError::BrepInvalid {
            request_id: Some(actual),
            ..
        }) => assert_eq!(actual, &request_id),
        _ => panic!("expected ID-bearing non-ok result; got {result:?}"),
    }

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn extrude_persistence_append_failure_preserves_canonical_state() {
    use std::os::unix::fs::PermissionsExt;
    // The chmod 0o500 trick cannot deny writes when running as root.
    // Probe by creating a temp dir, chmod'ing it 0o500, attempting a
    // write, and skipping the test if the write succeeds (i.e. we are
    // root and the chmod cannot deny the write).
    {
        let probe_parent = std::env::temp_dir().join(format!(
            "threeterm-host-occt-persist-probe-{}",
            std::process::id()
        ));
        fs::create_dir(&probe_parent).expect("probe parent creates");
        let mut perms = fs::metadata(&probe_parent).expect("stat").permissions();
        perms.set_mode(0o500);
        fs::set_permissions(&probe_parent, perms).expect("chmod");
        let probe = probe_parent.join("attempt");
        let write = fs::write(&probe, b"x");
        let mut restore = fs::metadata(&probe_parent).expect("stat").permissions();
        restore.set_mode(0o700);
        fs::set_permissions(&probe_parent, restore).expect("restore perms");
        let _ = fs::remove_dir_all(&probe_parent);
        if write.is_ok() {
            eprintln!("persistence_append_failure_preserves_canonical_state: skipping under root");
            return;
        }
    }

    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("persist-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut perms = fs::metadata(&root).expect("stat").permissions();
    perms.set_mode(0o500);
    fs::set_permissions(&root, perms).expect("chmod");

    let request = rectangle_extrude_request("persist-fail")
        .with_output_path(root.join("stage"), "extrude.brep");
    let result = host.extrude(&root, request, &worker);
    // The read-only permission kills the BREP-directory creation step
    // before the append_feature call, so the error is `HostError::BrepIo`.
    // Either is a valid persistence-failure signal for this scenario;
    // the atomicity invariant (manifest/log byte-equal, current
    // restored) holds in both branches.
    assert!(
        matches!(
            result,
            Err(HostError::Persistence(_)) | Err(HostError::BrepIo { .. })
        ),
        "got {result:?}"
    );

    let mut perms = fs::metadata(&root).expect("stat").permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&root, perms).expect("restore perms");
    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn boolean_fuse_of_two_extrudes_commits_a_fused_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("fuse", "box-seed", "box");
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let base_request = rectangle_extrude_request("fuse-base")
        .with_output_path(root.join("stage"), "base.brep")
        .with_feature_id("fuse-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let tool_request = ExtrudeRequest::new(
        unique_request_id("fuse-tool"),
        vec![(5.0, 0.0), (15.0, 0.0), (15.0, 5.0), (5.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "tool.brep")
    .with_feature_id("fuse-tool-1");
    let tool_view = host
        .extrude(&root, tool_request, &worker)
        .expect("tool extrude");
    assert_eq!(tool_view.result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("fuse"),
        root.join("brep/fuse-base-1.brep"),
        root.join("brep/fuse-tool-1.brep"),
    )
    .with_output_path(root.join("stage"), "fused.brep")
    .with_feature_id("fused-1");
    let fuse_request_id = fuse_request.request_id.clone();
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("boolean fuse commits");

    assert_eq!(fuse_view.result.status, "ok");
    assert_eq!(fuse_view.result.operation, Operation::BooleanFuse);
    assert_eq!(
        fuse_view.source_snapshot.revision_hash,
        tool_view.snapshot.revision_hash
    );
    assert_eq!(fuse_view.artifact.request_id, fuse_request_id);
    assert_eq!(fuse_view.artifact.operation, "boolean_fuse");
    assert_eq!(fuse_view.artifact.feature_id, "fused-1");
    assert_eq!(fuse_view.artifact.path, root.join("brep/fused-1.brep"));
    assert_eq!(
        fuse_view.artifact.byte_count,
        fuse_view.result.brep_bytes as u64
    );
    assert_eq!(fuse_view.artifact.sha256, fuse_view.result.brep_sha256);
    assert!(fuse_view.snapshot.revision_hash != base_view.snapshot.revision_hash);
    let fused_brep = root.join("brep/fused-1.brep");
    assert!(
        fused_brep.is_file(),
        "fused BREP is on disk at {fused_brep:?}"
    );
    assert_ne!(fuse_view.snapshot.revision_hash, prior_view.revision_hash);
    let transactions = fs::read_to_string(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");
    assert!(transactions.contains(&fuse_request_id));
    assert!(transactions.contains("boolean_fuse"));
    assert!(!transactions.contains("/stage/"));
    assert_eq!(
        Host::new().load(&root).expect("reloads"),
        fuse_view.snapshot
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn boolean_fuse_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("fuse-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("fuse-fail"),
        "/no/such/base.brep",
        "/no/such/tool.brep",
    )
    .with_output_path(root.join("stage"), "fused.brep")
    .with_feature_id("fused-fail-1");
    let result = host.boolean_fuse(&root, fuse_request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn boolean_fuse_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("fuse-malformed", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("fuse-malformed"),
        "/no/such/base.brep",
        "/no/such/tool.brep",
    )
    .with_output_path(root.join("stage"), "fused.brep")
    .with_feature_id("fused-malformed-1");
    let result = host.boolean_fuse(&root, fuse_request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn boolean_fuse_malformed_response_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("fuse-malformed-resp", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-fuse-{}.sh",
        std::process::id()
    ));
    fs::write(&script, "#!/bin/sh\nexit 0\n").expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("fuse-malformed-resp"),
        "/no/such/base.brep",
        "/no/such/tool.brep",
    )
    .with_output_path(root.join("stage"), "fused.brep")
    .with_feature_id("fused-malformed-resp-1");
    let result = host.boolean_fuse(&root, fuse_request, &fake_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn boolean_fuse_non_zero_exit_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("fuse-non-zero", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-fuse-nz-{}.sh",
        std::process::id()
    ));
    fs::write(&script, "#!/bin/sh\nexit 7\n").expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("fuse-non-zero"),
        "/no/such/base.brep",
        "/no/such/tool.brep",
    )
    .with_output_path(root.join("stage"), "fused.brep")
    .with_feature_id("fused-non-zero-1");
    let result = host.boolean_fuse(&root, fuse_request, &fake_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn boolean_fuse_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("fuse-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-fuse-brep-{}.sh",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("fuse-brep-invalid"),
        "/no/such/base.brep",
        "/no/such/tool.brep",
    )
    .with_output_path(root.join("stage"), "fused.brep")
    .with_feature_id("fused-brep-invalid-1");
    let result = host.boolean_fuse(&root, fuse_request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

fn fillet_request(label: &str, feature_id: &str, base_path: &Path) -> FilletRequest {
    FilletRequest::new(unique_request_id(label), base_path, 0.5)
        .with_output_path(PathBuf::from("/tmp"), "out.brep")
        .with_feature_id(feature_id)
}

fn chamfer_request(label: &str, feature_id: &str, base_path: &Path) -> ChamferRequest {
    ChamferRequest::new(unique_request_id(label), base_path, 0.25)
        .with_output_path(PathBuf::from("/tmp"), "out.brep")
        .with_feature_id(feature_id)
}

fn hole_request(label: &str, feature_id: &str, base_path: &Path) -> HoleRequest {
    HoleRequest::new(
        unique_request_id(label),
        base_path,
        [1.5, 1.5, 0.0],
        [0.0, 0.0, 1.0],
        1.0,
    )
    .with_output_path(PathBuf::from("/tmp"), "out.brep")
    .with_feature_id(feature_id)
}

fn mirror_request(label: &str, feature_id: &str, base_path: &Path) -> MirrorRequest {
    MirrorRequest::new(
        unique_request_id(label),
        base_path,
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
    )
    .with_output_path(PathBuf::from("/tmp"), "out.brep")
    .with_feature_id(feature_id)
}

fn linear_pattern_request(label: &str, feature_id: &str, base_path: &Path) -> LinearPatternRequest {
    LinearPatternRequest::new(unique_request_id(label), base_path, [1.0, 0.0, 0.0], 3, 3.0)
        .with_output_path(PathBuf::from("/tmp"), "out.brep")
        .with_feature_id(feature_id)
}

fn circular_pattern_request(
    label: &str,
    feature_id: &str,
    base_path: &Path,
) -> CircularPatternRequest {
    CircularPatternRequest::new(
        unique_request_id(label),
        base_path,
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        std::f64::consts::FRAC_PI_2,
        4,
    )
    .with_output_path(PathBuf::from("/tmp"), "out.brep")
    .with_feature_id(feature_id)
}

fn shell_request(label: &str, feature_id: &str, base_path: &Path) -> ShellRequest {
    ShellRequest::new(unique_request_id(label), base_path, 0.3)
        .with_output_path(PathBuf::from("/tmp"), "out.brep")
        .with_feature_id(feature_id)
}

fn draft_request(label: &str, feature_id: &str, base_path: &Path, angle: f64) -> DraftRequest {
    DraftRequest::new(unique_request_id(label), base_path, angle, [0.0, 0.0, 1.0])
        .with_output_path(PathBuf::from("/tmp"), "out.brep")
        .with_feature_id(feature_id)
}

fn committed_brep_path(root: &Path, feature_id: &str) -> PathBuf {
    root.join("brep").join(format!("{feature_id}.brep"))
}

#[test]
fn fillet_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("fillet-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("fillet-commit-base")
        .with_output_path(root.join("stage"), "fillet-base.brep")
        .with_feature_id("fillet-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "fillet-commit-base-1");
    let request = fillet_request("fillet-commit", "fillet-commit-1", &base_brep)
        .with_output_path(root.join("stage"), "fillet-commit.brep");
    let view = host
        .fillet(&root, request, &worker)
        .expect("fillet commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Fillet);
    let committed = committed_brep_path(&root, "fillet-commit-1");
    assert!(
        committed.is_file(),
        "filleted BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn chamfer_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("chamfer-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("chamfer-commit-base")
        .with_output_path(root.join("stage"), "chamfer-base.brep")
        .with_feature_id("chamfer-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "chamfer-commit-base-1");
    let request = chamfer_request("chamfer-commit", "chamfer-commit-1", &base_brep)
        .with_output_path(root.join("stage"), "chamfer-commit.brep");
    let view = host
        .chamfer(&root, request, &worker)
        .expect("chamfer commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Chamfer);
    let committed = committed_brep_path(&root, "chamfer-commit-1");
    assert!(
        committed.is_file(),
        "chamfered BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn fillet_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("fillet-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = fillet_request(
        "fillet-spawn-fail",
        "fillet-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.fillet(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn chamfer_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("chamfer-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = chamfer_request(
        "chamfer-spawn-fail",
        "chamfer-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.chamfer(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn fillet_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("fillet-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut request = FilletRequest::new(
        unique_request_id("fillet-bad-req"),
        "/no/such/base.brep",
        0.5,
    )
    .with_output_path(root.join("stage"), "fillet.brep")
    .with_feature_id("fillet-bad-req-1");
    request.radius = 0.0;
    let result = host.fillet(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn chamfer_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("chamfer-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut request = ChamferRequest::new(
        unique_request_id("chamfer-bad-req"),
        "/no/such/base.brep",
        0.25,
    )
    .with_output_path(root.join("stage"), "chamfer.brep")
    .with_feature_id("chamfer-bad-req-1");
    request.distance = 0.0;
    let result = host.chamfer(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn fillet_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("fillet-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-fillet-brep-{}.sh",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = fillet_request(
        "fillet-brep-invalid",
        "fillet-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.fillet(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn chamfer_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("chamfer-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-chamfer-brep-{}.sh",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = chamfer_request(
        "chamfer-brep-invalid",
        "chamfer-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.chamfer(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn fillet_then_chamfer_chain_reports_an_atomic_geometry_limitation() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("chain", "box-seed", "box");
    let host = Host::new();

    let base_request = rectangle_extrude_request("chain-base")
        .with_output_path(root.join("stage"), "chain-base.brep")
        .with_feature_id("chain-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "chain-base-1");
    let fillet_request = fillet_request("chain-fillet", "chain-fillet-1", &base_brep)
        .with_output_path(root.join("stage"), "chain-fillet.brep");
    let fillet_view = host.fillet(&root, fillet_request, &worker).expect("fillet");
    assert_eq!(fillet_view.result.status, "ok");

    let fillet_brep = committed_brep_path(&root, "chain-fillet-1");
    let chamfer_request = chamfer_request("chain-chamfer", "chain-chamfer-1", &fillet_brep)
        .with_output_path(root.join("stage"), "chain-chamfer.brep");
    let (manifest_before_chamfer, log_before_chamfer) = snapshot_files(&root);
    let result = host.chamfer(&root, chamfer_request, &worker);
    assert!(
        matches!(result, Err(HostError::UnsupportedGeometry { .. })),
        "got {result:?}"
    );

    assert_ne!(
        fillet_view.snapshot.revision_hash,
        base_view.snapshot.revision_hash
    );

    let reloaded = Host::new().load(&root).expect("reloads");
    assert_eq!(fillet_view.snapshot, reloaded);
    assert_eq!(
        snapshot_files(&root),
        (manifest_before_chamfer, log_before_chamfer)
    );
    assert!(
        !root.join("brep/chain-chamfer-1.brep").exists(),
        "rejected chamfer must not write a BREP"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn unsupported_chamfer_preserves_the_preceding_fillet_revision() {
    let root = fresh_bundle_with_feature("unsupported-chamfer", "l-bracket-fillet-1", "brep");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads preceding fillet revision");

    let script = std::env::temp_dir().join(format!(
        "threeterm-host-unsupported-chamfer-{}.sh",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 4\n",
            worker = fake_worker_script(&fake_failed_reply(
                "unsupported_geometry",
                "selected edges include fillet curves",
            ))
        ),
    )
    .expect("script writes");
    let mut permissions = fs::metadata(&script).expect("stat").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).expect("chmod");

    let worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = chamfer_request(
        "unsupported-chamfer",
        "l-bracket-chamfer-1",
        &root.join("brep/l-bracket-fillet-1.brep"),
    )
    .with_output_path(root.join("stage"), "l-bracket-chamfer.brep");
    let result = host.chamfer(&root, request, &worker);
    assert!(
        matches!(result, Err(HostError::UnsupportedGeometry { .. })),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view.clone()));
    assert_eq!(Host::new().load(&root).expect("reloads"), prior_view);
    assert!(!root.join("brep/l-bracket-chamfer-1.brep").exists());

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[allow(dead_code)]
fn _unused_command_marker() {
    let _ = Command::new("true").stdin(Stdio::null()).status();
}

#[test]
fn hole_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("hole-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("hole-commit-base")
        .with_output_path(root.join("stage"), "hole-base.brep")
        .with_feature_id("hole-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "hole-commit-base-1");
    let request = hole_request("hole-commit", "hole-commit-1", &base_brep)
        .with_output_path(root.join("stage"), "hole-commit.brep");
    let view = host.hole(&root, request, &worker).expect("hole commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Hole);
    let committed = committed_brep_path(&root, "hole-commit-1");
    assert!(
        committed.is_file(),
        "holed BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn hole_on_l_bracket_shows_hole_in_viewport() {
    // Demoable L-bracket end-to-end:
    //   1. Extrude a 10x5x3 base slab and a 3x10x3 vertical leg.
    //   2. Fuse them into an L-bracket.
    //   3. Drill a through-hole (diameter 1.0) along +Z at (1.5, 1.5, 0).
    //   4. Commit; the resulting BREP shows the hole.
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("l-bracket", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let slab_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-slab"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-slab.brep")
    .with_feature_id("l-bracket-slab-1");
    let slab_view = host
        .extrude(&root, slab_request, &worker)
        .expect("slab extrude");
    assert_eq!(slab_view.result.status, "ok");

    let leg_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-leg"),
        vec![(0.0, 0.0), (3.0, 0.0), (3.0, 10.0), (0.0, 10.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-leg.brep")
    .with_feature_id("l-bracket-leg-1");
    let leg_view = host
        .extrude(&root, leg_request, &worker)
        .expect("leg extrude");
    assert_eq!(leg_view.result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("l-bracket-fuse"),
        committed_brep_path(&root, "l-bracket-slab-1"),
        committed_brep_path(&root, "l-bracket-leg-1"),
    )
    .with_output_path(root.join("stage"), "l-bracket.brep")
    .with_feature_id("l-bracket-1");
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("l-bracket fuse");
    assert_eq!(fuse_view.result.status, "ok");

    let fused_brep = committed_brep_path(&root, "l-bracket-1");
    let fused_bytes = fs::read(&fused_brep).expect("fused BREP reads");
    let hole_request = HoleRequest::new(
        unique_request_id("l-bracket-hole"),
        &fused_brep,
        [1.5, 1.5, 0.0],
        [0.0, 0.0, 1.0],
        1.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-hole.brep")
    .with_feature_id("l-bracket-hole-1");
    let hole_view = host
        .hole(&root, hole_request, &worker)
        .expect("l-bracket hole");

    assert_eq!(hole_view.result.status, "ok");
    assert_eq!(hole_view.result.operation, Operation::Hole);
    assert_ne!(hole_view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(
        hole_view.snapshot.revision_hash,
        fuse_view.snapshot.revision_hash
    );
    let committed = committed_brep_path(&root, "l-bracket-hole-1");
    assert!(
        committed.is_file(),
        "holed L-bracket BREP is on disk at {committed:?}"
    );
    let bytes = fs::read(&committed).expect("holed BREP reads");
    assert!(!bytes.is_empty());
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "holed L-bracket BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
    assert_ne!(
        bytes, fused_bytes,
        "holed L-bracket BREP must differ byte-for-byte from the fused BREP; \
         an unchanged payload would mean the cut did not run"
    );
    assert_ne!(
        hole_view.result.brep_sha256, fuse_view.result.brep_sha256,
        "holed BREP sha256 must differ from the fused BREP sha256; \
         identical hashes would mean the cut did not run"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn hole_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("hole-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = hole_request(
        "hole-spawn-fail",
        "hole-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.hole(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn hole_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("hole-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let request = hole_request(
        "hole-bad-req",
        "hole-bad-req-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.hole(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn hole_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("hole-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-hole-brep-{}",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = hole_request(
        "hole-brep-invalid",
        "hole-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.hole(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn mirror_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("mirror-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("mirror-commit-base")
        .with_output_path(root.join("stage"), "mirror-base.brep")
        .with_feature_id("mirror-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "mirror-commit-base-1");
    let request = mirror_request("mirror-commit", "mirror-commit-1", &base_brep)
        .with_output_path(root.join("stage"), "mirror-commit.brep");
    let view = host
        .mirror(&root, request, &worker)
        .expect("mirror commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Mirror);
    let committed = committed_brep_path(&root, "mirror-commit-1");
    assert!(
        committed.is_file(),
        "mirrored BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn mirror_on_l_bracket_shows_mirrored_solid_in_viewport() {
    // Demoable L-bracket end-to-end:
    //   1. Extrude a 10x5x3 base slab and a 3x10x3 vertical leg.
    //   2. Fuse them into an L-bracket.
    //   3. Mirror the L-bracket across the YZ plane (x=0,
    //      normal=[1,0,0]); the result lands at x∈[-10, 0] next to
    //      the source at x∈[0, 10].
    //   4. Commit; the mirrored BREP shows the reflected L-bracket.
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("l-bracket-mirror", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let slab_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-mirror-slab"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-mirror-slab.brep")
    .with_feature_id("l-bracket-mirror-slab-1");
    let slab_view = host
        .extrude(&root, slab_request, &worker)
        .expect("slab extrude");
    assert_eq!(slab_view.result.status, "ok");

    let leg_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-mirror-leg"),
        vec![(0.0, 0.0), (3.0, 0.0), (3.0, 10.0), (0.0, 10.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-mirror-leg.brep")
    .with_feature_id("l-bracket-mirror-leg-1");
    let leg_view = host
        .extrude(&root, leg_request, &worker)
        .expect("leg extrude");
    assert_eq!(leg_view.result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("l-bracket-mirror-fuse"),
        committed_brep_path(&root, "l-bracket-mirror-slab-1"),
        committed_brep_path(&root, "l-bracket-mirror-leg-1"),
    )
    .with_output_path(root.join("stage"), "l-bracket-mirror.brep")
    .with_feature_id("l-bracket-mirror-1");
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("l-bracket fuse");
    assert_eq!(fuse_view.result.status, "ok");

    let fused_brep = committed_brep_path(&root, "l-bracket-mirror-1");
    let fused_bytes = fs::read(&fused_brep).expect("fused BREP reads");
    let mirror_request = MirrorRequest::new(
        unique_request_id("l-bracket-mirror"),
        &fused_brep,
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
    )
    .with_output_path(root.join("stage"), "l-bracket-mirror-mirror.brep")
    .with_feature_id("l-bracket-mirror-mirror-1");
    let mirror_view = host
        .mirror(&root, mirror_request, &worker)
        .expect("l-bracket mirror");

    assert_eq!(mirror_view.result.status, "ok");
    assert_eq!(mirror_view.result.operation, Operation::Mirror);
    assert_ne!(mirror_view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(
        mirror_view.snapshot.revision_hash,
        fuse_view.snapshot.revision_hash
    );
    let committed = committed_brep_path(&root, "l-bracket-mirror-mirror-1");
    assert!(
        committed.is_file(),
        "mirrored L-bracket BREP is on disk at {committed:?}"
    );
    let bytes = fs::read(&committed).expect("mirrored BREP reads");
    assert!(!bytes.is_empty());
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "mirrored L-bracket BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
    assert_ne!(
        bytes, fused_bytes,
        "mirrored L-bracket BREP must differ byte-for-byte from the fused BREP; \
         an unchanged payload would mean the mirror did not run"
    );
    assert_ne!(
        mirror_view.result.brep_sha256, fuse_view.result.brep_sha256,
        "mirrored BREP sha256 must differ from the fused BREP sha256; \
         identical hashes would mean the mirror did not run"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn mirror_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("mirror-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = mirror_request(
        "mirror-spawn-fail",
        "mirror-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.mirror(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn mirror_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("mirror-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut request = mirror_request(
        "mirror-bad-req",
        "mirror-bad-req-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    request.plane_normal = [0.0, 0.0, 0.0];
    let result = host.mirror(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn mirror_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("mirror-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-mirror-brep-{}",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = mirror_request(
        "mirror-brep-invalid",
        "mirror-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.mirror(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn linear_pattern_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("linear-pattern-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("linear-pattern-commit-base")
        .with_output_path(root.join("stage"), "linear-pattern-base.brep")
        .with_feature_id("linear-pattern-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "linear-pattern-commit-base-1");
    let request = linear_pattern_request(
        "linear-pattern-commit",
        "linear-pattern-commit-1",
        &base_brep,
    )
    .with_output_path(root.join("stage"), "linear-pattern-commit.brep");
    let view = host
        .linear_pattern(&root, request, &worker)
        .expect("linear_pattern commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::LinearPattern);
    let committed = committed_brep_path(&root, "linear-pattern-commit-1");
    assert!(
        committed.is_file(),
        "linear-pattern BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn linear_pattern_on_l_bracket_shows_patterned_solid_in_viewport() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("l-bracket-linear-pattern", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let slab_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-linear-pattern-slab"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-linear-pattern-slab.brep")
    .with_feature_id("l-bracket-linear-pattern-slab-1");
    let slab_view = host
        .extrude(&root, slab_request, &worker)
        .expect("slab extrude");
    assert_eq!(slab_view.result.status, "ok");

    let leg_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-linear-pattern-leg"),
        vec![(0.0, 0.0), (3.0, 0.0), (3.0, 10.0), (0.0, 10.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-linear-pattern-leg.brep")
    .with_feature_id("l-bracket-linear-pattern-leg-1");
    let leg_view = host
        .extrude(&root, leg_request, &worker)
        .expect("leg extrude");
    assert_eq!(leg_view.result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("l-bracket-linear-pattern-fuse"),
        committed_brep_path(&root, "l-bracket-linear-pattern-slab-1"),
        committed_brep_path(&root, "l-bracket-linear-pattern-leg-1"),
    )
    .with_output_path(root.join("stage"), "l-bracket-linear-pattern.brep")
    .with_feature_id("l-bracket-linear-pattern-1");
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("l-bracket fuse");
    assert_eq!(fuse_view.result.status, "ok");

    let fused_brep = committed_brep_path(&root, "l-bracket-linear-pattern-1");
    let fused_bytes = fs::read(&fused_brep).expect("fused BREP reads");
    let pattern_request = LinearPatternRequest::new(
        unique_request_id("l-bracket-linear-pattern"),
        &fused_brep,
        [1.0, 0.0, 0.0],
        3,
        12.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-linear-pattern-pattern.brep")
    .with_feature_id("l-bracket-linear-pattern-pattern-1");
    let pattern_view = host
        .linear_pattern(&root, pattern_request, &worker)
        .expect("l-bracket linear pattern");

    assert_eq!(pattern_view.result.status, "ok");
    assert_eq!(pattern_view.result.operation, Operation::LinearPattern);
    assert_ne!(pattern_view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(
        pattern_view.snapshot.revision_hash,
        fuse_view.snapshot.revision_hash
    );
    let committed = committed_brep_path(&root, "l-bracket-linear-pattern-pattern-1");
    assert!(
        committed.is_file(),
        "patterned L-bracket BREP is on disk at {committed:?}"
    );
    let bytes = fs::read(&committed).expect("patterned BREP reads");
    assert!(!bytes.is_empty());
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "patterned L-bracket BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
    assert_ne!(
        bytes, fused_bytes,
        "patterned L-bracket BREP must differ byte-for-byte from the fused BREP; \
         an unchanged payload would mean the pattern did not run"
    );
    assert_ne!(
        pattern_view.result.brep_sha256, fuse_view.result.brep_sha256,
        "patterned BREP sha256 must differ from the fused BREP sha256; \
         identical hashes would mean the pattern did not run"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn linear_pattern_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("linear-pattern-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = linear_pattern_request(
        "linear-pattern-spawn-fail",
        "linear-pattern-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.linear_pattern(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn linear_pattern_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("linear-pattern-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut request = linear_pattern_request(
        "linear-pattern-bad-req",
        "linear-pattern-bad-req-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    request.direction = [0.0, 0.0, 0.0];
    let result = host.linear_pattern(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn linear_pattern_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("linear-pattern-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-linear-pattern-brep-{}",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = linear_pattern_request(
        "linear-pattern-brep-invalid",
        "linear-pattern-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.linear_pattern(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn circular_pattern_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("circular-pattern-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("circular-pattern-commit-base")
        .with_output_path(root.join("stage"), "circular-pattern-base.brep")
        .with_feature_id("circular-pattern-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "circular-pattern-commit-base-1");
    let request = circular_pattern_request(
        "circular-pattern-commit",
        "circular-pattern-commit-1",
        &base_brep,
    )
    .with_output_path(root.join("stage"), "circular-pattern-commit.brep");
    let view = host
        .circular_pattern(&root, request, &worker)
        .expect("circular_pattern commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::CircularPattern);
    let committed = committed_brep_path(&root, "circular-pattern-commit-1");
    assert!(
        committed.is_file(),
        "circular-pattern BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn circular_pattern_on_l_bracket_shows_patterned_solid_in_viewport() {
    // Demoable L-bracket end-to-end:
    //   1. Extrude a 10x5x3 base slab and a 3x10x3 vertical leg.
    //   2. Fuse them into an L-bracket.
    //   3. Pattern the L-bracket four times around the +Z axis at
    //      (0, 0, 0) with a 90° step (the resulting 4-tuple lands the
    //      copies at 0°, 90°, 180°, 270° around the source).
    //   4. Commit; the patterned BREP shows the rotated L-bracket.
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("l-bracket-circular-pattern", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let slab_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-circular-pattern-slab"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-circular-pattern-slab.brep")
    .with_feature_id("l-bracket-circular-pattern-slab-1");
    let slab_view = host
        .extrude(&root, slab_request, &worker)
        .expect("slab extrude");
    assert_eq!(slab_view.result.status, "ok");

    let leg_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-circular-pattern-leg"),
        vec![(0.0, 0.0), (3.0, 0.0), (3.0, 10.0), (0.0, 10.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-circular-pattern-leg.brep")
    .with_feature_id("l-bracket-circular-pattern-leg-1");
    let leg_view = host
        .extrude(&root, leg_request, &worker)
        .expect("leg extrude");
    assert_eq!(leg_view.result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("l-bracket-circular-pattern-fuse"),
        committed_brep_path(&root, "l-bracket-circular-pattern-slab-1"),
        committed_brep_path(&root, "l-bracket-circular-pattern-leg-1"),
    )
    .with_output_path(root.join("stage"), "l-bracket-circular-pattern.brep")
    .with_feature_id("l-bracket-circular-pattern-1");
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("l-bracket fuse");
    assert_eq!(fuse_view.result.status, "ok");

    let fused_brep = committed_brep_path(&root, "l-bracket-circular-pattern-1");
    let fused_bytes = fs::read(&fused_brep).expect("fused BREP reads");
    let pattern_request = CircularPatternRequest::new(
        unique_request_id("l-bracket-circular-pattern"),
        &fused_brep,
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        std::f64::consts::FRAC_PI_2,
        4,
    )
    .with_output_path(
        root.join("stage"),
        "l-bracket-circular-pattern-pattern.brep",
    )
    .with_feature_id("l-bracket-circular-pattern-pattern-1");
    let pattern_view = host
        .circular_pattern(&root, pattern_request, &worker)
        .expect("l-bracket circular pattern");

    assert_eq!(pattern_view.result.status, "ok");
    assert_eq!(pattern_view.result.operation, Operation::CircularPattern);
    assert_ne!(pattern_view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(
        pattern_view.snapshot.revision_hash,
        fuse_view.snapshot.revision_hash
    );
    let committed = committed_brep_path(&root, "l-bracket-circular-pattern-pattern-1");
    assert!(
        committed.is_file(),
        "patterned L-bracket BREP is on disk at {committed:?}"
    );
    let bytes = fs::read(&committed).expect("patterned BREP reads");
    assert!(!bytes.is_empty());
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "patterned L-bracket BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
    assert_ne!(
        bytes, fused_bytes,
        "patterned L-bracket BREP must differ byte-for-byte from the fused BREP; \
         an unchanged payload would mean the pattern did not run"
    );
    assert_ne!(
        pattern_view.result.brep_sha256, fuse_view.result.brep_sha256,
        "patterned BREP sha256 must differ from the fused BREP sha256; \
         identical hashes would mean the pattern did not run"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn circular_pattern_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("circular-pattern-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = circular_pattern_request(
        "circular-pattern-spawn-fail",
        "circular-pattern-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.circular_pattern(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn circular_pattern_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("circular-pattern-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut request = circular_pattern_request(
        "circular-pattern-bad-req",
        "circular-pattern-bad-req-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    request.axis_normal = [0.0, 0.0, 0.0];
    let result = host.circular_pattern(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn circular_pattern_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("circular-pattern-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-circular-pattern-brep-{}",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = circular_pattern_request(
        "circular-pattern-brep-invalid",
        "circular-pattern-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.circular_pattern(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn shell_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("shell-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("shell-commit-base")
        .with_output_path(root.join("stage"), "shell-base.brep")
        .with_feature_id("shell-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "shell-commit-base-1");
    let request = shell_request("shell-commit", "shell-commit-1", &base_brep)
        .with_output_path(root.join("stage"), "shell-commit.brep");
    let view = host.shell(&root, request, &worker).expect("shell commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Shell);
    let committed = committed_brep_path(&root, "shell-commit-1");
    assert!(
        committed.is_file(),
        "shelled BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn shell_on_l_bracket_shows_shelled_solid_in_viewport() {
    // Demoable L-bracket end-to-end:
    //   1. Extrude a 10x5x3 base slab and a 3x10x3 vertical leg.
    //   2. Fuse them into an L-bracket.
    //   3. Shell the L-bracket with a positive wall thickness,
    //      producing a hollow BREP.
    //   4. Commit; the resulting BREP shows the shelled L-bracket.
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("l-bracket-shell", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let slab_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-shell-slab"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-shell-slab.brep")
    .with_feature_id("l-bracket-shell-slab-1");
    let slab_view = host
        .extrude(&root, slab_request, &worker)
        .expect("slab extrude");
    assert_eq!(slab_view.result.status, "ok");

    let leg_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-shell-leg"),
        vec![(0.0, 0.0), (3.0, 0.0), (3.0, 10.0), (0.0, 10.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-shell-leg.brep")
    .with_feature_id("l-bracket-shell-leg-1");
    let leg_view = host
        .extrude(&root, leg_request, &worker)
        .expect("leg extrude");
    assert_eq!(leg_view.result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("l-bracket-shell-fuse"),
        committed_brep_path(&root, "l-bracket-shell-slab-1"),
        committed_brep_path(&root, "l-bracket-shell-leg-1"),
    )
    .with_output_path(root.join("stage"), "l-bracket-shell.brep")
    .with_feature_id("l-bracket-shell-1");
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("l-bracket fuse");
    assert_eq!(fuse_view.result.status, "ok");

    let fused_brep = committed_brep_path(&root, "l-bracket-shell-1");
    let fused_bytes = fs::read(&fused_brep).expect("fused BREP reads");
    let shell_request = ShellRequest::new(unique_request_id("l-bracket-shell"), &fused_brep, 0.3)
        .with_output_path(root.join("stage"), "l-bracket-shell-shell.brep")
        .with_feature_id("l-bracket-shell-shell-1");
    let shell_view = host
        .shell(&root, shell_request, &worker)
        .expect("l-bracket shell");

    assert_eq!(shell_view.result.status, "ok");
    assert_eq!(shell_view.result.operation, Operation::Shell);
    assert_ne!(shell_view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(
        shell_view.snapshot.revision_hash,
        fuse_view.snapshot.revision_hash
    );
    let committed = committed_brep_path(&root, "l-bracket-shell-shell-1");
    assert!(
        committed.is_file(),
        "shelled L-bracket BREP is on disk at {committed:?}"
    );
    let bytes = fs::read(&committed).expect("shelled BREP reads");
    assert!(!bytes.is_empty());
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "shelled L-bracket BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
    assert_ne!(
        bytes, fused_bytes,
        "shelled L-bracket BREP must differ byte-for-byte from the fused BREP; \
         an unchanged payload would mean the shell did not run"
    );
    assert_ne!(
        shell_view.result.brep_sha256, fuse_view.result.brep_sha256,
        "shelled BREP sha256 must differ from the fused BREP sha256; \
         identical hashes would mean the shell did not run"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn shell_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("shell-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = shell_request(
        "shell-spawn-fail",
        "shell-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.shell(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn shell_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("shell-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut request = ShellRequest::new(
        unique_request_id("shell-bad-req"),
        "/no/such/base.brep",
        0.3,
    )
    .with_output_path(root.join("stage"), "shell.brep")
    .with_feature_id("shell-bad-req-1");
    request.thickness = 0.0;
    let result = host.shell(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn shell_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("shell-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-shell-brep-{}",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = shell_request(
        "shell-brep-invalid",
        "shell-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.shell(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn shell_persistence_append_failure_preserves_canonical_state() {
    use std::os::unix::fs::PermissionsExt;
    {
        let probe_parent = std::env::temp_dir().join(format!(
            "threeterm-host-occt-shell-persist-probe-{}",
            std::process::id()
        ));
        fs::create_dir(&probe_parent).expect("probe parent creates");
        let mut perms = fs::metadata(&probe_parent).expect("stat").permissions();
        perms.set_mode(0o500);
        fs::set_permissions(&probe_parent, perms).expect("chmod");
        let probe = probe_parent.join("attempt");
        let write = fs::write(&probe, b"x");
        let mut restore = fs::metadata(&probe_parent).expect("stat").permissions();
        restore.set_mode(0o700);
        fs::set_permissions(&probe_parent, restore).expect("restore perms");
        let _ = fs::remove_dir_all(&probe_parent);
        if write.is_ok() {
            eprintln!(
                "shell_persistence_append_failure_preserves_canonical_state: skipping under root"
            );
            return;
        }
    }

    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("shell-persist-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut perms = fs::metadata(&root).expect("stat").permissions();
    perms.set_mode(0o500);
    fs::set_permissions(&root, perms).expect("chmod");

    let request = shell_request(
        "shell-persist-fail",
        "shell-persist-fail-1",
        &PathBuf::from("/no/such/base.brep"),
    );
    let result = host.shell(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::Persistence(_)) | Err(HostError::BrepIo { .. })
        ),
        "got {result:?}"
    );

    let mut perms = fs::metadata(&root).expect("stat").permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&root, perms).expect("restore perms");
    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("draft-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let base_request = rectangle_extrude_request("draft-commit-base")
        .with_output_path(root.join("stage"), "draft-base.brep")
        .with_feature_id("draft-commit-base-1");
    let base_view = host
        .extrude(&root, base_request, &worker)
        .expect("base extrude");
    assert_eq!(base_view.result.status, "ok");

    let base_brep = committed_brep_path(&root, "draft-commit-base-1");
    let request = draft_request(
        "draft-commit",
        "draft-commit-1",
        &base_brep,
        0.2617993877991494,
    )
    .with_output_path(root.join("stage"), "draft-commit.brep");
    let view = host.draft(&root, request, &worker).expect("draft commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Draft);
    let committed = committed_brep_path(&root, "draft-commit-1");
    assert!(
        committed.is_file(),
        "drafted BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_on_l_bracket_shows_drafted_solid_in_viewport() {
    // Demoable L-bracket end-to-end:
    //   1. Extrude a 10x5x3 base slab and a 3x10x3 vertical leg.
    //   2. Fuse them into an L-bracket.
    //   3. Draft the L-bracket with a positive draft angle along +Z,
    //      producing a tapered BREP.
    //   4. Commit; the resulting BREP shows the drafted L-bracket.
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("l-bracket-draft", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let slab_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-draft-slab"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-draft-slab.brep")
    .with_feature_id("l-bracket-draft-slab-1");
    let slab_view = host
        .extrude(&root, slab_request, &worker)
        .expect("slab extrude");
    assert_eq!(slab_view.result.status, "ok");

    let leg_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-draft-leg"),
        vec![(0.0, 0.0), (3.0, 0.0), (3.0, 10.0), (0.0, 10.0)],
        3.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-draft-leg.brep")
    .with_feature_id("l-bracket-draft-leg-1");
    let leg_view = host
        .extrude(&root, leg_request, &worker)
        .expect("leg extrude");
    assert_eq!(leg_view.result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("l-bracket-draft-fuse"),
        committed_brep_path(&root, "l-bracket-draft-slab-1"),
        committed_brep_path(&root, "l-bracket-draft-leg-1"),
    )
    .with_output_path(root.join("stage"), "l-bracket-draft.brep")
    .with_feature_id("l-bracket-draft-1");
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("l-bracket fuse");
    assert_eq!(fuse_view.result.status, "ok");

    let fused_brep = committed_brep_path(&root, "l-bracket-draft-1");
    let fused_bytes = fs::read(&fused_brep).expect("fused BREP reads");
    let angle = std::f64::consts::FRAC_PI_2 / 6.0; // 15°
    let draft_request = DraftRequest::new(
        unique_request_id("l-bracket-draft"),
        &fused_brep,
        angle,
        [0.0, 0.0, 1.0],
    )
    .with_output_path(root.join("stage"), "l-bracket-draft-draft.brep")
    .with_feature_id("l-bracket-draft-draft-1");
    let draft_view = host
        .draft(&root, draft_request, &worker)
        .expect("l-bracket draft");

    assert_eq!(draft_view.result.status, "ok");
    assert_eq!(draft_view.result.operation, Operation::Draft);
    assert_ne!(draft_view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(
        draft_view.snapshot.revision_hash,
        fuse_view.snapshot.revision_hash
    );
    let committed = committed_brep_path(&root, "l-bracket-draft-draft-1");
    assert!(
        committed.is_file(),
        "drafted L-bracket BREP is on disk at {committed:?}"
    );
    let bytes = fs::read(&committed).expect("drafted BREP reads");
    assert!(!bytes.is_empty());
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "drafted L-bracket BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
    assert_ne!(
        bytes, fused_bytes,
        "drafted L-bracket BREP must differ byte-for-byte from the fused BREP; \
         an unchanged payload would mean the draft did not run"
    );
    assert_ne!(
        draft_view.result.brep_sha256, fuse_view.result.brep_sha256,
        "drafted BREP sha256 must differ from the fused BREP sha256; \
         identical hashes would mean the draft did not run"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("draft-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = draft_request(
        "draft-spawn-fail",
        "draft-spawn-fail-1",
        &PathBuf::from("/no/such/base.brep"),
        std::f64::consts::FRAC_PI_2 / 6.0,
    );
    let result = host.draft(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_request_malformed_preserves_canonical_state() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("draft-bad-req", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut request = DraftRequest::new(
        unique_request_id("draft-bad-req"),
        "/no/such/base.brep",
        std::f64::consts::FRAC_PI_2 / 6.0,
        [0.0, 0.0, 1.0],
    )
    .with_output_path(root.join("stage"), "draft.brep")
    .with_feature_id("draft-bad-req-1");
    request.angle = 0.0;
    let result = host.draft(&root, request, &worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("draft-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-draft-brep-{}",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = draft_request(
        "draft-brep-invalid",
        "draft-brep-invalid-1",
        &PathBuf::from("/no/such/base.brep"),
        std::f64::consts::FRAC_PI_2 / 6.0,
    );
    let result = host.draft(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

fn loft_request(label: &str, feature_id: &str) -> LoftRequest {
    LoftRequest::new(
        unique_request_id(label),
        vec![
            vec![
                [0.0, 0.0, 0.0],
                [10.0, 0.0, 0.0],
                [10.0, 10.0, 0.0],
                [0.0, 10.0, 0.0],
            ],
            vec![
                [2.5, 2.5, 5.0],
                [7.5, 2.5, 5.0],
                [7.5, 7.5, 5.0],
                [2.5, 7.5, 5.0],
            ],
        ],
    )
    .with_output_path(PathBuf::from("/tmp"), "out.brep")
    .with_feature_id(feature_id)
}

#[test]
fn loft_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("loft-commit", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let request = loft_request("loft-commit", "loft-commit-1")
        .with_output_path(root.join("stage"), "loft-commit.brep");
    let view = host.loft(&root, request, &worker).expect("loft commits");

    assert_ne!(view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(view.snapshot.feature_graph_hash, prior.feature_graph_hash);
    assert_eq!(view.result.status, "ok");
    assert_eq!(view.result.operation, Operation::Loft);
    let committed = committed_brep_path(&root, "loft-commit-1");
    assert!(
        committed.is_file(),
        "lofted BREP is on disk at {committed:?}"
    );
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loft_on_two_rectangles_shows_lofted_solid_in_viewport() {
    // Demoable end-to-end: build a loft from two rectangles that share
    // the same edge count, commit, and verify the BREP starts with the
    // OCCT viewport marker and differs from the underlying extrude BREP.
    let Some(worker) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("l-bracket-loft", "box-seed", "box");
    let host = Host::new();
    let prior = host.load(&root).expect("host loads prior");

    let slab_request = ExtrudeRequest::new(
        unique_request_id("l-bracket-loft-slab"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
        1.0,
    )
    .with_output_path(root.join("stage"), "l-bracket-loft-slab.brep")
    .with_feature_id("l-bracket-loft-slab-1");
    let slab_view = host
        .extrude(&root, slab_request, &worker)
        .expect("slab extrude");
    assert_eq!(slab_view.result.status, "ok");

    let extrude_brep = committed_brep_path(&root, "l-bracket-loft-slab-1");
    let extrude_bytes = fs::read(&extrude_brep).expect("extrude BREP reads");

    let profiles = vec![
        vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ],
        vec![
            [2.5, 2.5, 5.0],
            [7.5, 2.5, 5.0],
            [7.5, 7.5, 5.0],
            [2.5, 7.5, 5.0],
        ],
    ];
    let loft_request = LoftRequest::new(unique_request_id("l-bracket-loft"), profiles)
        .with_output_path(root.join("stage"), "l-bracket-loft-lofted.brep")
        .with_feature_id("l-bracket-loft-lofted-1");
    let loft_view = host
        .loft(&root, loft_request, &worker)
        .expect("l-bracket loft");

    assert_eq!(loft_view.result.status, "ok");
    assert_eq!(loft_view.result.operation, Operation::Loft);
    assert_ne!(loft_view.snapshot.revision_hash, prior.revision_hash);
    assert_ne!(
        loft_view.snapshot.revision_hash,
        slab_view.snapshot.revision_hash
    );
    let committed = committed_brep_path(&root, "l-bracket-loft-lofted-1");
    assert!(
        committed.is_file(),
        "lofted BREP is on disk at {committed:?}"
    );
    let bytes = fs::read(&committed).expect("lofted BREP reads");
    assert!(!bytes.is_empty());
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "lofted BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
    assert_ne!(
        bytes, extrude_bytes,
        "lofted BREP must differ byte-for-byte from the extrude BREP; \
         an unchanged payload would mean the loft did not run"
    );
    assert_ne!(
        loft_view.result.brep_sha256, slab_view.result.brep_sha256,
        "lofted BREP sha256 must differ from the extrude BREP sha256; \
         identical hashes would mean the loft did not run"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loft_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("loft-spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = loft_request("loft-spawn-fail", "loft-spawn-fail-1")
        .with_output_path(root.join("stage"), "loft-spawn.brep");
    let result = host.loft(&root, request, &bad_worker);
    assert!(
        matches!(
            result,
            Err(HostError::WorkerFailure { .. } | HostError::WorkerTerminated { .. })
        ),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loft_brep_invalid_preserves_canonical_state() {
    let Some(_) = locate_worker() else {
        return;
    };
    let root = fresh_bundle_with_feature("loft-brep-invalid", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-fake-occt-loft-brep-{}",
        std::process::id()
    ));
    fs::write(
        &script,
        format!(
            "{worker}\nexit 3\n",
            worker = fake_worker_script(&fake_failed_reply(
                "brep_invalid",
                "BRepCheck_Analyzer failed",
            ))
        ),
    )
    .expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let request = loft_request("loft-brep-invalid", "loft-brep-invalid-1")
        .with_output_path(root.join("stage"), "loft-brep.brep");
    let result = host.loft(&root, request, &fake_worker);
    assert!(is_brep_invalid(&result), "got {result:?}");

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn adversarial_trailing_worker_data_preserves_canonical_host_state() {
    let root = fresh_bundle_with_feature("adversarial-trailing", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");
    let output_dir = root.join("stage");
    fs::create_dir_all(&output_dir).expect("stage directory creates");
    let output = output_dir.join("extrude.brep");
    let bytes = b"valid staged worker output";
    fs::write(&output, bytes).expect("staged output writes");
    let digest = threeterm_occt_worker::sha256_file(&output).expect("staged output hashes");
    let request = ExtrudeRequest::new(
        "req-adversarial",
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_feature_id("adversarial-box")
    .with_output_path(&output_dir, "extrude.brep");
    let reply = format!(
        r#"printf '%s\n%s\n' '{{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"req-adversarial","result":{{"schema_version":"threeterm.workers.occt/1","request_id":"req-adversarial","operation":"extrude","status":"ok","brep_path":"{path}","brep_sha256":"{digest}","brep_bytes":{bytes},"feature_id":"adversarial-box"}}}}' '{{"kind":"progress","schema_version":"threeterm.protocol/1","request_id":"req-adversarial","stage":"trailing","percent":100}}'"#,
        path = output.display(),
        digest = digest,
        bytes = bytes.len(),
    );
    let mut script = std::env::temp_dir();
    script.push(format!(
        "threeterm-host-adversarial-trailing-{}.sh",
        std::process::id()
    ));
    fs::write(&script, format!("{}\n", fake_worker_script(&reply))).expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = threeterm_occt_worker::OcctWorker::with_binary_path(script.clone());
    let result = host.extrude(&root, request, &fake_worker);
    let record = match result {
        Err(HostError::WorkerTerminated { record }) => record,
        other => panic!("trailing worker data must terminate the request: {other:?}"),
    };
    assert_eq!(record.request_id, "req-adversarial");
    assert!(
        record.stage.contains("trailing") || record.stage.contains("protocol"),
        "termination must preserve the framing failure: {:?}",
        record.stage
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));
    assert!(!root.join("brep/adversarial-box.brep").exists());
    assert_eq!(
        fs::read(&output).expect("caller output remains"),
        bytes,
        "rejected caller output must be preserved"
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

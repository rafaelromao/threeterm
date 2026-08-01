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
//! `OcctWorker::locate` returning `Err`; the CI archlinux container
//! installs `opencascade` so the binary is built and the tests run.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{
    BooleanFuseRequest, ChamferRequest, ExtrudeRequest, FilletRequest, Operation, schema_version,
};
use threeterm_persistence::{Bundle, MANIFEST_FILENAME, TRANSACTIONS_LOG_FILENAME};

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

fn locate_worker() -> Option<threeterm_occt_worker::OcctWorker> {
    threeterm_occt_worker::OcctWorker::locate().ok()
}

fn fresh_bundle_with_feature(label: &str, feature_id: &str, kind: &str) -> PathBuf {
    let root = temp_root(label);
    let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
    bundle
        .append_feature(feature_id, kind)
        .expect("seed feature appends");
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

#[test]
fn extrude_commits_brep_into_a_new_revision() {
    let Some(worker) = locate_worker() else {
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
fn worker_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("spawn-fail", "box-seed", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    let bad_worker =
        threeterm_occt_worker::OcctWorker::with_binary_path(PathBuf::from("/no/such/worker"));
    let request = rectangle_extrude_request("spawn-fail")
        .with_output_path(root.join("stage"), "extrude.brep");
    let result = host.extrude(&root, request, &bad_worker);
    assert!(
        matches!(result, Err(HostError::WorkerFailure { .. })),
        "got {result:?}"
    );

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest, "manifest must be unchanged");
    assert_eq!(prior_log, post_log, "log must be unchanged");
    assert_eq!(host.current(), Some(prior_view));

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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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

    // Build a tiny shell script that exits 0 with empty stdout —
    // mirrors the worker's malformed-output path. The host should
    // classify this as a worker failure and preserve canonical state.
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
    let diagnostic = serde_json::json!({
        "schema_version": schema_version(),
        "code": "brep_invalid",
        "arg": "BRepCheck_Analyzer failed"
    });
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat <<'JSON'\n{diagnostic}\nJSON\nexit 3\n",
            diagnostic = serde_json::to_string(&diagnostic).unwrap()
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
    assert!(
        matches!(result, Err(HostError::BrepInvalid { .. })),
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
    let fuse_view = host
        .boolean_fuse(&root, fuse_request, &worker)
        .expect("boolean fuse commits");

    assert_eq!(fuse_view.result.status, "ok");
    assert_eq!(fuse_view.result.operation, Operation::BooleanFuse);
    assert!(fuse_view.snapshot.revision_hash != base_view.snapshot.revision_hash);
    let fused_brep = root.join("brep/fused-1.brep");
    assert!(
        fused_brep.is_file(),
        "fused BREP is on disk at {fused_brep:?}"
    );
    assert_ne!(fuse_view.snapshot.revision_hash, prior_view.revision_hash);

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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
    let diagnostic = serde_json::json!({
        "schema_version": threeterm_occt_worker::schema_version(),
        "code": "brep_invalid",
        "arg": "BRepCheck_Analyzer failed"
    });
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat <<'JSON'\n{diagnostic}\nJSON\nexit 3\n",
            diagnostic = serde_json::to_string(&diagnostic).unwrap()
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
    assert!(
        matches!(result, Err(HostError::BrepInvalid { .. })),
        "got {result:?}"
    );

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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
        matches!(result, Err(HostError::WorkerFailure { .. })),
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
    let diagnostic = serde_json::json!({
        "schema_version": schema_version(),
        "code": "brep_invalid",
        "arg": "BRepCheck_Analyzer failed"
    });
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat <<'JSON'\n{diagnostic}\nJSON\nexit 3\n",
            diagnostic = serde_json::to_string(&diagnostic).unwrap()
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
    assert!(
        matches!(result, Err(HostError::BrepInvalid { .. })),
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
    let diagnostic = serde_json::json!({
        "schema_version": schema_version(),
        "code": "brep_invalid",
        "arg": "BRepCheck_Analyzer failed"
    });
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat <<'JSON'\n{diagnostic}\nJSON\nexit 3\n",
            diagnostic = serde_json::to_string(&diagnostic).unwrap()
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
    assert!(
        matches!(result, Err(HostError::BrepInvalid { .. })),
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
fn fillet_then_chamfer_chain_commits_two_revisions() {
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
    let chamfer_view = host
        .chamfer(&root, chamfer_request, &worker)
        .expect("chamfer");
    assert_eq!(chamfer_view.result.status, "ok");

    assert_ne!(
        fillet_view.snapshot.revision_hash,
        base_view.snapshot.revision_hash
    );
    assert_ne!(
        chamfer_view.snapshot.revision_hash,
        fillet_view.snapshot.revision_hash
    );

    let reloaded = Host::new().load(&root).expect("reloads");
    assert_eq!(chamfer_view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[allow(dead_code)]
fn _unused_command_marker() {
    let _ = Command::new("true").stdin(Stdio::null()).status();
}

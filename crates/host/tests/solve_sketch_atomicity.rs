//! Atomicity and end-to-end tests for the host's `solve_sketch` method.
//!
//! These tests exercise the real worker binary through the SlvsWorker
//! boundary so the production code path is the system under test. They
//! assert that every failure mode leaves the canonical host state
//! unchanged: the bundle's `manifest.json` and `transactions.log` are
//! byte-identical to a pre-solve snapshot, and `Host::current()` is
//! preserved.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::{Host, HostError};
use threeterm_persistence::{Bundle, MANIFEST_FILENAME, TRANSACTIONS_LOG_FILENAME};
use threeterm_slvs_worker::{SketchEntity, SketchRequest, SketchConstraint, SlvsWorker, WorkerError};

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-host-slvs-{label}-{}-{nanos}",
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

fn locate_worker() -> Option<SlvsWorker> {
    SlvsWorker::locate().ok()
}

fn fresh_bundle_with_feature(label: &str, feature_id: &str, kind: &str) -> PathBuf {
    let root = temp_root(label);
    let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str())
        .expect("bundle creates");
    bundle
        .append_feature(feature_id, kind)
        .expect("seed feature appends");
    root
}

fn fully_constrained_rectangle() -> SketchRequest {
    SketchRequest::new(unique_request_id("rect"))
        .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0))
        .with_entity(SketchEntity::point_2d("p2", 10.0, 0.0))
        .with_entity(SketchEntity::point_2d("p3", 10.0, 5.0))
        .with_entity(SketchEntity::point_2d("p4", 0.0, 5.0))
        .with_entity(SketchEntity::line_segment_2d("l1", "p1", "p2"))
        .with_entity(SketchEntity::line_segment_2d("l2", "p2", "p3"))
        .with_entity(SketchEntity::line_segment_2d("l3", "p3", "p4"))
        .with_entity(SketchEntity::line_segment_2d("l4", "p4", "p1"))
        .with_constraint(SketchConstraint::horizontal("h1", "l1"))
        .with_constraint(SketchConstraint::vertical("v2", "l2"))
        .with_constraint(SketchConstraint::horizontal("h3", "l3"))
        .with_constraint(SketchConstraint::vertical("v4", "l4"))
        .with_constraint(SketchConstraint::distance("dw", "p1", "p2", 10.0))
        .with_constraint(SketchConstraint::distance("dh", "p1", "p4", 5.0))
}

fn snapshot_files(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("manifest reads");
    let log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("log reads");
    (manifest, log)
}

#[test]
fn solve_sketch_commits_geometry_into_a_new_revision() {
    let Some(worker) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("commit", "box-1", "box");
    let prior = Host::new().load(&root).expect("host loads prior");

    let view = Host::new()
        .solve_sketch(&root, &fully_constrained_rectangle(), &worker)
        .expect("solve_sketch commits");

    assert!(view.snapshot.revision_hash != prior.revision_hash);
    assert!(view.snapshot.feature_graph_hash != prior.feature_graph_hash);
    let reloaded = Host::new().load(&root).expect("reloads after commit");
    assert_eq!(view.snapshot, reloaded);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_spawn_failure_preserves_canonical_state() {
    let Some(_) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("spawn-fail", "box-1", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // Point the worker at a non-existent binary.
    let bad_worker = SlvsWorker::with_binary_path(PathBuf::from("/no/such/worker-binary"));
    let result = host.solve_sketch(&root, &fully_constrained_rectangle(), &bad_worker);
    assert!(matches!(result, Err(HostError::WorkerFailure { .. })));

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest, "manifest must be unchanged");
    assert_eq!(prior_log, post_log, "log must be unchanged");
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_non_ok_status_preserves_canonical_state() {
    let Some(worker) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("non-ok", "box-1", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // Two coincident fixed points with a non-zero distance between them is
    // an inconsistent sketch.
    let request = SketchRequest::new(unique_request_id("inconsistent"))
        .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0))
        .with_entity(SketchEntity::fixed_point_2d("p2", 1.0, 0.0))
        .with_constraint(SketchConstraint::distance("d", "p1", "p2", 10.0))
        .with_constraint(SketchConstraint::coincident("c", "p1", "p2"));
    let result = host.solve_sketch(&root, &request, &worker);
    assert!(matches!(result, Err(HostError::WorkerFailure { .. })));

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn underconstrained_sketch_commits_with_resolved_coordinates() {
    let Some(worker) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("under", "box-1", "box");
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // Two free points with no constraints — the worker reports ok with
    // positive dof (no constraint failures). The host commits because the
    // status is ok; the caller sees the dof in the returned view.
    let request = SketchRequest::new(unique_request_id("under"))
        .with_entity(SketchEntity::point_2d("p1", 0.0, 0.0))
        .with_entity(SketchEntity::point_2d("p2", 1.0, 1.0));
    let view = host
        .solve_sketch(&root, &request, &worker)
        .expect("underconstrained solve commits");

    assert_eq!(view.solve.status, "ok");
    assert!(view.solve.dof >= 2);
    let coords = view.solve.coordinates.expect("coordinates");
    assert_eq!(coords.get("p1"), Some(&[0.0, 0.0]));
    assert_eq!(coords.get("p2"), Some(&[1.0, 1.0]));

    // The bundle must have grown.
    assert_ne!(view.snapshot.revision_hash, prior_view.revision_hash);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_malformed_response_preserves_canonical_state() {
    let Some(worker) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("malformed", "box-1", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // A request that the worker rejects as malformed triggers the
    // diagnostic path. The host converts it into a WorkerFailure and
    // preserves canonical state.
    let mut request = SketchRequest::new(unique_request_id("dup"));
    request.entities.push(SketchEntity::fixed_point_2d("p1", 0.0, 0.0));
    request.entities.push(SketchEntity::fixed_point_2d("p1", 1.0, 1.0));
    let result = host.solve_sketch(&root, &request, &worker);
    assert!(matches!(result, Err(HostError::WorkerFailure { .. })));

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn non_zero_exit_preserves_canonical_state() {
    let Some(_worker) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("non-zero", "box-1", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // Build a tiny shell script that exits 7 with empty stdout — mirrors
    // the worker's non-zero-exit path. The host should classify this as
    // a worker failure and preserve canonical state.
    let mut script = std::env::temp_dir();
    script.push(format!("threeterm-host-fake-worker-{}.sh", std::process::id()));
    fs::write(&script, "#!/bin/sh\nexit 7\n").expect("script writes");
    let mut perms = fs::metadata(&script).expect("stat").permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let fake_worker = SlvsWorker::with_binary_path(script.clone());
    let result =
        host.solve_sketch(&root, &fully_constrained_rectangle(), &fake_worker);
    assert!(matches!(result, Err(HostError::WorkerFailure { .. })));

    let (post_manifest, post_log) = snapshot_files(&root);
    assert_eq!(prior_manifest, post_manifest);
    assert_eq!(prior_log, post_log);
    assert_eq!(host.current(), Some(prior_view));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(script);
}

#[test]
fn persistence_append_failure_preserves_canonical_state() {
    // Skip when running as root because chmod 0o500 cannot deny writes
    // to root; the test relies on the filesystem enforcing permissions.
    use std::os::unix::fs::PermissionsExt;
    if fs::metadata("/proc/self/status")
        .ok()
        .and_then(|_| None::<()>)
        .is_none()
    {
        // Probe the sandbox by trying to write to a 0o500 dir as we
        // would in the test body. If the write succeeds, we're running
        // as root and the chmod cannot deny writes.
        let probe_dir = std::env::temp_dir().join(format!(
            "threeterm-host-persist-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir(&probe_dir).expect("probe dir creates");
        fs::write(probe_dir.join("read_only"), b"seed").expect("seed");
        let mut perms = fs::metadata(&probe_dir).expect("stat").permissions();
        perms.set_mode(0o500);
        fs::set_permissions(&probe_dir, perms).expect("chmod");
        let probe_write = fs::write(probe_dir.join("attempt"), b"x");
        let mut perms = fs::metadata(&probe_dir).expect("stat").permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&probe_dir, perms).expect("restore perms");
        let _ = fs::remove_dir_all(&probe_dir);
        if probe_write.is_ok() {
            eprintln!(
                "persistence_append_failure_preserves_canonical_state: skipping under root"
            );
            return;
        }
    }

    let Some(worker) = locate_worker() else { return };
    let root = fresh_bundle_with_feature("persist-fail", "box-1", "box");
    let (prior_manifest, prior_log) = snapshot_files(&root);
    let host = Host::new();
    let prior_view = host.load(&root).expect("loads");

    // Make the bundle directory read-only so the manifest write fails.
    let mut perms = fs::metadata(&root).expect("stat").permissions();
    perms.set_mode(0o500);
    fs::set_permissions(&root, perms).expect("chmod");

    let result =
        host.solve_sketch(&root, &fully_constrained_rectangle(), &worker);
    assert!(matches!(result, Err(HostError::Persistence(_))));

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
fn worker_error_from_trait_classifies_diagnostic_into_worker_failure() {
    // Manual smoke check that WorkerError variants round-trip through
    // HostError::From. We don't need the real worker binary for this.
    let diag = threeterm_slvs_worker::SolveDiagnostic::new("request_malformed", "bad input");
    let worker_error: WorkerError = WorkerError::Diagnostic(diag);
    let host_error: HostError = HostError::from(worker_error);
    match host_error {
        HostError::WorkerFailure { detail } => {
            assert!(detail.contains("request_malformed"));
            assert!(detail.contains("bad input"));
        }
        other => panic!("expected WorkerFailure, got {other:?}"),
    }
}

#[allow(dead_code)]
fn _unused_command_marker() {
    // Forces the build script to keep the worker integration test binary in
    // sync when we change the cargo dependency graph. Currently a no-op.
    let _ = Command::new("true").stdin(Stdio::null()).status();
}
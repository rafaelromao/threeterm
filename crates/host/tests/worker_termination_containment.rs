//! Host canonical-state preservation across worker termination.
//!
//! Each test drives a production `Host::extrude` request with a fixture
//! worker that terminates (force-stop, signal, failure-then-crash) and
//! asserts the canonical `Bundle` manifest/transactions are byte-identical,
//! no staged geometry was promoted, and the structured `HostError` retains
//! diagnostic context.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::{Bundle, MANIFEST_FILENAME, TRANSACTIONS_LOG_FILENAME};

const PROTOCOL_SCHEMA: &str = "threeterm.protocol/1";

struct FixtureDir {
    root: PathBuf,
}

impl FixtureDir {
    fn new(label: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "threeterm-host-termination-{label}-{}-{nanos}-{count}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("fixture dir creates");
        Self { root }
    }

    fn worker_script(&self, name: &str, body: &str) -> OcctWorker {
        let script = self.root.join(name);
        std::fs::write(&script, body).expect("fixture script writes");
        let mut perm = std::fs::metadata(&script).expect("metadata").permissions();
        use std::os::unix::fs::PermissionsExt;
        perm.set_mode(0o755);
        std::fs::set_permissions(&script, perm).expect("chmod");
        OcctWorker::with_binary_path(script)
            .with_expected_worker_id("fixture")
            .with_grace(Duration::from_millis(500))
    }
}

fn retry_host<T>(mut attempt: impl FnMut() -> Result<T, HostError>) -> Result<T, HostError> {
    let mut last = None;
    for _ in 0..3 {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(e) if e.to_string().contains("Text file busy") => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("retry"))
}

fn sample_extrude_fixed_id(request_id: &str, output_dir: &std::path::Path) -> ExtrudeRequest {
    ExtrudeRequest::new(
        request_id.to_string(),
        vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
        2.0,
    )
    .with_output_path(output_dir, "out.brep")
    .with_feature_id("box-1")
}

fn create_host_with_bundle(label: &str) -> (FixtureDir, Host, String) {
    let dir = FixtureDir::new(label);
    let bundle_root = dir.root.join("bundle");
    let bundle =
        Bundle::create_for_test(&bundle_root, "00".repeat(16).as_str()).expect("bundle creates");
    bundle
        .append_feature("seed-1", "plate")
        .expect("seed appends");
    let host = Host::new();
    host.load(&bundle_root).expect("host loads");
    let _current = host.current().expect("host has current");
    (dir, host, bundle_root.display().to_string())
}

fn snapshot_manifest_transactions(bundle_root: &str) -> (Vec<u8>, Vec<u8>) {
    let manifest =
        std::fs::read(PathBuf::from(bundle_root).join(MANIFEST_FILENAME)).expect("manifest reads");
    let log = std::fs::read(PathBuf::from(bundle_root).join(TRANSACTIONS_LOG_FILENAME))
        .expect("log reads");
    (manifest, log)
}

#[test]
fn cooperative_cancel_does_not_mutate_canonical_state() {
    // Cooperative cancel via direct worker with HostError projection:
    // the HostError must be WorkerTerminated with last_progress retained.
    let (dir, host, bundle_root) = create_host_with_bundle("coop-canonical");
    let (before_manifest, before_log) = snapshot_manifest_transactions(&bundle_root);
    let before_snapshot = host.current().expect("before snapshot");

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tracing\",\"percent\":10}}'\n\
             echo \"stderr-marker-host-coop\" >&2\n\
             read cancel_line\n\
             req=$(printf '%s' \"$cancel_line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"cancelled\",\"schema_version\":\"{schema}\",\"request_id\":\"'$req'\",\"reason\":\"cancelled by host\"}}'\n",
            schema = PROTOCOL_SCHEMA
        ),
    );
    let request_id = format!(
        "req-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let host_err =
        retry_host(|| host.extrude_with_cancel(&bundle_root, request.clone(), &worker, &cancel))
            .expect_err("must cancel");
    match &host_err {
        HostError::WorkerTerminated { record } => {
            assert_eq!(record.request_id, request_id);
            assert_eq!(record.stage, "cancelled");
            let prog = record
                .last_progress
                .as_ref()
                .expect("last_progress retained");
            assert_eq!(prog.stage, "tracing");
            assert!(
                record.stderr_tail.contains("stderr-marker-host-coop"),
                "stderr tail must contain marker, got {:?}",
                record.stderr_tail
            );
        }
        other => panic!("expected WorkerTerminated for cooperative cancel, got {other:?}"),
    }

    // Canonical must be unchanged
    let (after_manifest, after_log) = snapshot_manifest_transactions(&bundle_root);
    assert_eq!(
        before_manifest, after_manifest,
        "manifest unchanged after cancel"
    );
    assert_eq!(before_log, after_log, "log unchanged after cancel");
    assert_eq!(host.current().expect("after snapshot"), before_snapshot);
    assert_eq!(
        host.current().expect("after generation").generation_id,
        before_snapshot.generation_id
    );
    assert!(
        !dir.root.join("out.brep").exists(),
        "staged output must be discarded"
    );
}

#[test]
fn force_stop_does_not_mutate_canonical_state() {
    let (dir, host, bundle_root) = create_host_with_bundle("force-canonical");
    let (before_manifest, before_log) = snapshot_manifest_transactions(&bundle_root);
    let before_snapshot = host.current().expect("before");

    let worker = dir.worker_script(
        "worker.sh",
        "#!/bin/sh\n\
         printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'\n\
         read line\n\
         sleep 30\n",
    );
    let request_id = format!(
        "req-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let err = retry_host(|| host.extrude(&bundle_root, request.clone(), &worker))
        .expect_err("must force-terminate");
    match &err {
        HostError::WorkerTerminated { record } => {
            assert_eq!(record.request_id, request_id);
            assert_eq!(record.exit_signal, Some(9));
        }
        other => panic!("expected WorkerTerminated, got {other:?}"),
    }

    let (after_manifest, after_log) = snapshot_manifest_transactions(&bundle_root);
    assert_eq!(before_manifest, after_manifest);
    assert_eq!(before_log, after_log);
    assert_eq!(host.current().expect("after"), before_snapshot);
    assert_eq!(
        host.current().expect("after generation").generation_id,
        before_snapshot.generation_id
    );
    assert!(!dir.root.join("out.brep").exists());
}

#[test]
fn signal_crash_does_not_mutate_canonical_state() {
    let (dir, host, bundle_root) = create_host_with_bundle("signal-canonical");
    let (before_manifest, before_log) = snapshot_manifest_transactions(&bundle_root);
    let before_snapshot = host.current().expect("before");

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             kill -TERM $$\n",
            schema = PROTOCOL_SCHEMA
        ),
    );
    let request_id = format!(
        "req-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let err = retry_host(|| host.extrude(&bundle_root, request.clone(), &worker))
        .expect_err("must signal");
    assert!(
        matches!(
            &err,
            HostError::WorkerTerminated { .. } | HostError::WorkerFailure { .. }
        ),
        "got {err:?}"
    );
    if let HostError::WorkerTerminated { record } = &err {
        assert_eq!(record.exit_signal, Some(15));
    }

    let (after_manifest, after_log) = snapshot_manifest_transactions(&bundle_root);
    assert_eq!(before_manifest, after_manifest);
    assert_eq!(before_log, after_log);
    assert_eq!(host.current().expect("after"), before_snapshot);
}

#[test]
fn failure_then_crash_does_not_mutate_canonical_state() {
    let (dir, host, bundle_root) = create_host_with_bundle("fail-crash-canonical");
    let (before_manifest, before_log) = snapshot_manifest_transactions(&bundle_root);
    let before_snapshot = host.current().expect("before");

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"failed\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"code\":\"worker_failed\",\"detail\":\"boom\"}}'\n\
             kill -TERM $$\n",
            schema = PROTOCOL_SCHEMA
        ),
    );
    let request_id = format!(
        "req-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let err =
        retry_host(|| host.extrude(&bundle_root, request.clone(), &worker)).expect_err("must fail");
    match &err {
        HostError::WorkerTerminated { record } => {
            // failure-then-crash retains failed_code and signal
            assert_eq!(record.failed_code.as_deref(), Some("worker_failed"));
        }
        HostError::WorkerFailure { .. } => {
            // Also acceptable if mapped to WorkerFailure (failed without signal)
        }
        other => panic!("expected WorkerTerminated or WorkerFailure, got {other:?}"),
    }

    let (after_manifest, after_log) = snapshot_manifest_transactions(&bundle_root);
    assert_eq!(before_manifest, after_manifest);
    assert_eq!(before_log, after_log);
    assert_eq!(host.current().expect("after"), before_snapshot);
}

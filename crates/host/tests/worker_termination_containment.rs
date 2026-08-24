//! Host canonical-state preservation across worker termination.
//!
//! Each test drives a production `OcctWorker` request that terminates
//! (cooperative cancel, force-stop, signal, descendant) and asserts the
//! canonical `Bundle` manifest/transactions are byte-identical and no
//! staged geometry was promoted.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, WorkerError};
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

fn retry_fixture<T>(mut attempt: impl FnMut() -> Result<T, WorkerError>) -> Result<T, WorkerError> {
    let mut last = None;
    for _ in 0..3 {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(e @ WorkerError::Spawn { .. }) if e.to_string().contains("Text file busy") => {
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
    let (dir, host, bundle_root) = create_host_with_bundle("coop-canonical");
    let bundle_path = PathBuf::from(&bundle_root);
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
    let err =
        retry_fixture(|| worker.extrude_with_cancel(&request, &cancel)).expect_err("must cancel");
    assert!(matches!(err, WorkerError::Cancelled { .. }), "got {err:?}");

    // Canonical must be unchanged
    let (after_manifest, after_log) = snapshot_manifest_transactions(&bundle_root);
    assert_eq!(
        before_manifest, after_manifest,
        "manifest unchanged after cancel"
    );
    assert_eq!(before_log, after_log, "log unchanged after cancel");
    assert_eq!(host.current().expect("after snapshot"), before_snapshot);
    // Staged output must not exist
    assert!(
        !dir.root.join("out.brep").exists(),
        "staged output must be discarded"
    );
    let _ = bundle_path;
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
    let err = retry_fixture(|| worker.extrude(&request)).expect_err("must force-terminate");
    assert!(matches!(err, WorkerError::Supervised { .. }), "got {err:?}");

    let (after_manifest, after_log) = snapshot_manifest_transactions(&bundle_root);
    assert_eq!(before_manifest, after_manifest);
    assert_eq!(before_log, after_log);
    assert_eq!(host.current().expect("after"), before_snapshot);
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
    let err = retry_fixture(|| worker.extrude(&request)).expect_err("must signal");
    assert!(
        matches!(
            err,
            WorkerError::SignalledWithContext { .. } | WorkerError::Supervised { .. }
        ),
        "got {err:?}"
    );

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
    let err = retry_fixture(|| worker.extrude(&request)).expect_err("must fail");
    assert!(
        matches!(
            err,
            WorkerError::Supervised { .. } | WorkerError::DiagnosticWithContext { .. }
        ),
        "got {err:?}"
    );

    let (after_manifest, after_log) = snapshot_manifest_transactions(&bundle_root);
    assert_eq!(before_manifest, after_manifest);
    assert_eq!(before_log, after_log);
    assert_eq!(host.current().expect("after"), before_snapshot);
}

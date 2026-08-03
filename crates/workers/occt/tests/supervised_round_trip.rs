//! End-to-end tests driving the production typed `OcctWorker` API
//! against fixture worker processes that speak the versioned envelope
//! protocol.
//!
//! Unlike `worker_integration.rs` (which needs the OCCT-built binary),
//! these tests use plain `sh` fixture workers so the supervised
//! production path — spawn, handshake, request envelope, completed /
//! failed mapping — is exercised in any environment.

use std::time::Duration;

use threeterm_occt_worker::{ExtrudeRequest, OcctDiagnostic, OcctWorker, WorkerError};

const PROTOCOL_SCHEMA: &str = "threeterm.protocol/1";

/// Per-test fixture directories. A fresh directory per (label, test run)
/// means parallel tests never share a script path (writing one test's
/// script while another test's leftover process still executes it would
/// fail with `Text file busy`). Directories are left in the temp dir,
/// matching the repo's other `threeterm-*` test artifacts.
struct FixtureDir {
    root: std::path::PathBuf,
}

impl FixtureDir {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "threeterm-fixture-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("fixture dir creates");
        Self { root }
    }

    fn worker_script(&self, name: &str, body: &str) -> OcctWorker {
        let script = self.root.join(name);
        std::fs::write(&script, body).expect("fixture script writes");
        let mut permissions = std::fs::metadata(&script)
            .expect("fixture script metadata")
            .permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("fixture script chmod");
        OcctWorker::with_binary_path(script).with_grace(Duration::from_secs(10))
    }
}

/// A fixture worker that emits the WorkerReady handshake, reads one
/// request line, then runs `reply` (which must emit one envelope line).
fn fixture_worker_named(label: &str, reply: &str) -> OcctWorker {
    let dir = FixtureDir::new(label);
    let fixture = format!(
        "#!/bin/sh\n\
         printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
         read line\n\
         {reply}\n",
        schema = PROTOCOL_SCHEMA
    );
    dir.worker_script("worker.sh", &fixture)
}

/// Retries a fixture-driven call when the exec of a freshly written
/// script hits the overlayfs `Text file busy` race (the container's
/// `/tmp` is an overlayfs with `fsync=volatile`, where execve of a just
/// written script can transiently fail with ETXTBSY). The retry budget
/// is small and bounded; real failures still surface.
fn retry_fixture<T>(mut attempt: impl FnMut() -> Result<T, WorkerError>) -> Result<T, WorkerError> {
    let mut last = None;
    for _ in 0..3 {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error @ WorkerError::Spawn { .. }) => {
                let detail = error.to_string();
                if detail.contains("Text file busy") {
                    last = Some(error);
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("at least one attempt ran"))
}

fn sample_extrude_request() -> ExtrudeRequest {
    ExtrudeRequest::new("req-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 2.0)
        .with_output_path("/tmp", "out.brep")
        .with_feature_id("box-1")
}

/// Completed-envelope reply whose `result` is a valid `ExtrudeResult`
/// JSON object. The request_id is echoed from the request line so the
/// supervisor's request binding holds.
fn completed_reply() -> &'static str {
    r#"printf '%s\n' '{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"req-1","result":{"schema_version":"threeterm.workers.occt/1","request_id":"req-1","operation":"extrude","status":"ok","brep_path":"/tmp/out.brep","brep_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","brep_bytes":42,"feature_id":"box-1"}}'"#
}

#[test]
fn typed_extrude_routes_through_the_supervised_protocol() {
    let worker = fixture_worker_named("ok", completed_reply());
    let result =
        retry_fixture(|| worker.extrude(&sample_extrude_request())).expect("extrude succeeds");
    assert_eq!(result.status, "ok");
    assert_eq!(result.feature_id, "box-1");
    assert_eq!(result.brep_bytes, 42);
    assert_eq!(result.brep_path, std::path::PathBuf::from("/tmp/out.brep"));
}

#[test]
fn typed_extrude_surfaces_a_cooperative_failed_envelope_as_diagnostic() {
    let worker = fixture_worker_named(
        "failed",
        r#"printf '%s\n' '{"kind":"failed","schema_version":"threeterm.protocol/1","request_id":"req-1","code":"brep_invalid","detail":"BRepCheck_Analyzer failed"}'"#,
    );
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request()))
        .expect_err("failed envelope must fail the typed call");
    match error {
        WorkerError::Diagnostic(diagnostic) => {
            assert_eq!(diagnostic.code, "brep_invalid");
            assert_eq!(diagnostic.arg, "BRepCheck_Analyzer failed");
        }
        other => panic!("expected Diagnostic; got {other:?}"),
    }
}

#[test]
fn typed_extrude_rejects_a_schema_mismatched_worker() {
    let dir = FixtureDir::new("bad-schema");
    let worker = dir.worker_script(
        "worker.sh",
        "#!/bin/sh\n\
         printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/0\",\"worker_id\":\"old\"}'\n\
         sleep 5\n",
    );
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request()))
        .expect_err("schema-mismatched worker must fail closed");
    assert!(
        matches!(error, WorkerError::Malformed { .. }),
        "expected Malformed; got {error:?}"
    );
}

#[test]
fn typed_extrude_fails_closed_when_the_worker_never_completes() {
    // The fixture completes the handshake but never emits a terminal
    // envelope; the supervisor grace must force-terminate it.
    let dir = FixtureDir::new("hang");
    let worker = dir.worker_script(
        "worker.sh",
        "#!/bin/sh\n\
         printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'\n\
         read line\n\
         sleep 30\n",
    )
    .with_grace(Duration::from_millis(500));
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request()))
        .expect_err("hanging worker must fail closed");
    match error {
        WorkerError::Signalled { signal, .. } => {
            assert_eq!(signal, 9, "force-terminated worker reports SIGKILL");
        }
        other => panic!("expected Signalled; got {other:?}"),
    }
}

#[test]
fn locate_missing_binary_fails_closed() {
    let worker = OcctWorker::with_binary_path(std::path::PathBuf::from("/no/such/worker"))
        .with_grace(Duration::from_millis(100));
    let error = worker
        .extrude(&sample_extrude_request())
        .expect_err("missing binary must fail closed");
    assert!(
        matches!(error, WorkerError::Spawn { .. }),
        "expected Spawn; got {error:?}"
    );
}

#[test]
fn diagnostics_round_trip_with_schema_version() {
    let diagnostic = OcctDiagnostic::new("request_malformed", "empty profile");
    assert_eq!(diagnostic.schema_version, "threeterm.workers.occt/1");
}

#[test]
fn typed_extrude_fails_closed_on_oversized_staged_output() {
    // The worker reports a brep_bytes count above the staged artifact
    // bound; the typed boundary must fail closed before the host could
    // promote the oversized payload.
    let oversized = format!("{}", threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1);
    let reply = format!(
        r#"printf '%s\n' '{{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"req-1","result":{{"schema_version":"threeterm.workers.occt/1","request_id":"req-1","operation":"extrude","status":"ok","brep_path":"/tmp/out.brep","brep_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","brep_bytes":{oversized},"feature_id":"box-1"}}}}'"#
    );
    let worker = fixture_worker_named("oversized", &reply);
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request()))
        .expect_err("oversized staged output must fail closed");
    match error {
        WorkerError::Malformed { detail } => {
            assert!(
                detail.contains("exceeds the"),
                "detail must name the bound; got {detail:?}"
            );
        }
        other => panic!("expected Malformed; got {other:?}"),
    }
}

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
    sample_extrude_request_at("/tmp")
}

fn sample_extrude_request_at(output_dir: impl Into<std::path::PathBuf>) -> ExtrudeRequest {
    ExtrudeRequest::new("req-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 2.0)
        .with_output_path(output_dir, "out.brep")
        .with_feature_id("box-1")
}

#[test]
fn typed_extrude_routes_through_the_supervised_protocol() {
    // The staged output must exist as a regular file whose digest
    // matches the advertisement; write it before the worker runs.
    let dir = FixtureDir::new("ok-real");
    let staged = dir.root.join("out.brep");
    let bytes = vec![b'x'; 42];
    std::fs::write(&staged, &bytes).expect("staged file writes");
    let digest = threeterm_occt_worker::sha256_file(&staged).expect("staged file hashes");
    let reply = format!(
        r#"printf '%s\n' '{{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"req-1","result":{{"schema_version":"threeterm.workers.occt/1","request_id":"req-1","operation":"extrude","status":"ok","brep_path":"{path}","brep_sha256":"{digest}","brep_bytes":42,"feature_id":"box-1"}}}}'"#,
        path = staged.display()
    );
    let worker = dir.worker_script("worker.sh", &format!(
        "#!/bin/sh\n\
         printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
         read line\n\
         {reply}\n"
    ));
    let result = retry_fixture(|| worker.extrude(&sample_extrude_request_at(&dir.root)))
        .expect("extrude succeeds");
    assert_eq!(result.status, "ok");
    assert_eq!(result.feature_id, "box-1");
    assert_eq!(result.brep_bytes, 42);
    assert_eq!(result.brep_path, staged);
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
        WorkerError::Supervised { record } => {
            assert_eq!(
                record.exit_signal,
                Some(9),
                "force-terminated worker reports SIGKILL"
            );
            assert_eq!(record.stage, "grace_exceeded");
        }
        other => panic!("expected Supervised; got {other:?}"),
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
    // The worker writes an oversized staged file and reports a matching
    // brep_bytes count above the staged artifact bound; the typed
    // boundary must fail closed before the host could promote the
    // oversized payload.
    let dir = FixtureDir::new("oversized");
    let staged = dir.root.join("out.brep");
    let mut body = Vec::new();
    body.resize(threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1, b'x');
    std::fs::write(&staged, &body).expect("oversized staged file writes");
    let digest = threeterm_occt_worker::sha256_file(&staged).expect("staged file hashes");
    let oversized = format!("{}", threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1);
    let reply = format!(
        r#"printf '%s\n' '{{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"req-1","result":{{"schema_version":"threeterm.workers.occt/1","request_id":"req-1","operation":"extrude","status":"ok","brep_path":"{path}","brep_sha256":"{digest}","brep_bytes":{oversized},"feature_id":"box-1"}}}}'"#,
        path = staged.display()
    );
    let worker = dir.worker_script("worker.sh", &format!(
        "#!/bin/sh\n\
         printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
         read line\n\
         {reply}\n"
    ));
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request_at(&dir.root)))
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

#[test]
fn typed_extrude_fails_closed_when_the_actual_staged_file_is_oversized() {
    // The worker under-reports brep_bytes but writes an oversized file
    // on disk; the typed boundary must verify the ACTUAL size, not the
    // advertised metadata.
    let dir = FixtureDir::new("actual-oversized");
    let oversized_path = dir.root.join("out.brep");
    let mut body = Vec::new();
    body.resize(threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1, b'x');
    std::fs::write(&oversized_path, &body).expect("oversized staged file writes");
    let small = format!("{}", threeterm_protocol::worker::MAX_ARTIFACT_BYTES - 1);

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"req-1\",\"operation\":\"extrude\",\"status\":\"ok\",\"brep_path\":\"{path}\",\"brep_sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"brep_bytes\":{small},\"feature_id\":\"box-1\"}}}}'\n",
            path = oversized_path.display()
        ),
    );
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request_at(&dir.root)))
        .expect_err("actual oversized staged output must fail closed");
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

#[test]
fn typed_extrude_with_cancel_acknowledges_cooperative_worker() {
    // The fixture worker emits WorkerReady, reads the request line and
    // the Cancel line, and acknowledges with a Cancelled envelope. The
    // cancellable typed path must surface the cooperative cancellation.
    let dir = FixtureDir::new("cancel-ack");
    let worker = dir.worker_script(
        "worker.sh",
        "#!/bin/sh\n\
         printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'\n\
         read line\n\
         read line\n\
         printf '%s\\n' '{\"kind\":\"cancelled\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"reason\":\"cancelled by host\"}'\n",
    );
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let error = retry_fixture(|| worker.extrude_with_cancel(&sample_extrude_request(), &cancel))
        .expect_err("cancellable extrude must surface the cancellation");
    match error {
        WorkerError::Cancelled { request_id } => {
            assert_eq!(request_id, "req-1");
        }
        other => panic!("expected Cancelled; got {other:?}"),
    }
}

#[test]
fn typed_extrude_with_cancel_force_terminates_an_uncooperative_worker() {
    // The fixture worker never acks the Cancel; the cancellation
    // lifecycle must force-terminate the worker after the grace period
    // and report the kill signal in the structured record.
    let dir = FixtureDir::new("cancel-force");
    let worker = dir
        .worker_script(
            "worker.sh",
            "#!/bin/sh\n\
             printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'\n\
             read line\n\
             sleep 30\n",
        )
        .with_grace(Duration::from_millis(300));
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let error = retry_fixture(|| worker.extrude_with_cancel(&sample_extrude_request(), &cancel))
        .expect_err("uncooperative cancellable extrude must fail closed");
    match error {
        WorkerError::Supervised { record } => {
            assert_eq!(record.exit_signal, Some(9), "SIGKILL after grace");
        }
        other => panic!("expected Supervised; got {other:?}"),
    }
}

#[test]
fn typed_extrude_fails_closed_on_staged_digest_mismatch() {
    // The worker advertises one SHA-256 but the staged file contains
    // different bytes; the typed boundary must fail closed instead of
    // trusting the advertisement.
    let dir = FixtureDir::new("digest-mismatch");
    let staged = dir.root.join("out.brep");
    std::fs::write(&staged, b"actual worker bytes").expect("staged file writes");
    let actual = threeterm_occt_worker::sha256_file(&staged).expect("staged file hashes");

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"req-1\",\"operation\":\"extrude\",\"status\":\"ok\",\"brep_path\":\"{path}\",\"brep_sha256\":\"{advertised}\",\"brep_bytes\":19,\"feature_id\":\"box-1\"}}}}'\n",
            path = staged.display(),
            advertised = {
                let mut fake = actual.clone();
                if fake.ends_with('0') { fake.pop(); fake.push('1'); } else { fake.pop(); fake.push('0'); }
                fake
            },
        ),
    );
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request_at(&dir.root)))
        .expect_err("digest mismatch must fail closed");
    match error {
        WorkerError::Malformed { detail } => {
            assert!(
                detail.contains("digest mismatch"),
                "detail must name the digest mismatch; got {detail:?}"
            );
        }
        other => panic!("expected Malformed; got {other:?}"),
    }
}

#[test]
fn typed_extrude_fails_closed_on_non_regular_staged_file() {
    // A worker pointing its output at a directory (not a regular file)
    // must fail closed: only regular files can be promoted.
    let dir = FixtureDir::new("non-regular");
    let staged = dir.root.join("out.brep");
    std::fs::create_dir_all(&staged).expect("directory staged as output");

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"req-1\",\"operation\":\"extrude\",\"status\":\"ok\",\"brep_path\":\"{path}\",\"brep_sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"brep_bytes\":0,\"feature_id\":\"box-1\"}}}}'\n",
            path = staged.display()
        ),
    );
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request_at(&dir.root)))
        .expect_err("non-regular staged output must fail closed");
    match error {
        WorkerError::Malformed { detail } => {
            assert!(
                detail.contains("not a regular file"),
                "detail must name the file identity; got {detail:?}"
            );
        }
        other => panic!("expected Malformed; got {other:?}"),
    }
}

#[test]
fn typed_extrude_fails_closed_on_a_symlinked_staged_file() {
    // A worker pointing its output at a symlink must fail closed: the
    // staged artifact must be a regular file, not a redirected path.
    let dir = FixtureDir::new("symlink");
    let target = dir.root.join("real.brep");
    std::fs::write(&target, b"real bytes").expect("target file writes");
    let staged = dir.root.join("out.brep");
    std::os::unix::fs::symlink(&target, &staged).expect("symlink creates");
    let digest = threeterm_occt_worker::sha256_file(&target).expect("target hashes");

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"req-1\",\"operation\":\"extrude\",\"status\":\"ok\",\"brep_path\":\"{path}\",\"brep_sha256\":\"{digest}\",\"brep_bytes\":10,\"feature_id\":\"box-1\"}}}}'\n",
            path = staged.display()
        ),
    );
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request_at(&dir.root)))
        .expect_err("symlinked staged output must fail closed");
    match error {
        WorkerError::Malformed { detail } => {
            assert!(
                detail.contains("symlink"),
                "detail must name the symlink; got {detail:?}"
            );
        }
        other => panic!("expected Malformed; got {other:?}"),
    }
}

#[test]
fn typed_extrude_with_cancel_fails_closed_on_oversized_staged_output() {
    // The cancellable path must run the same bounded decoder: an
    // oversized staged file fails closed even when a token is present.
    let dir = FixtureDir::new("cancel-oversized");
    let staged = dir.root.join("out.brep");
    let mut body = Vec::new();
    body.resize(threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1, b'x');
    std::fs::write(&staged, &body).expect("oversized staged file writes");
    let digest = threeterm_occt_worker::sha256_file(&staged).expect("staged file hashes");
    let oversized = format!("{}", threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1);
    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"req-1\",\"operation\":\"extrude\",\"feature_id\":\"box-1\",\"status\":\"ok\",\"brep_path\":\"{path}\",\"brep_sha256\":\"{digest}\",\"brep_bytes\":{oversized},\"feature_id\":\"box-1\"}}}}'\n",
            path = staged.display()
        ),
    );
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let error = retry_fixture(|| {
        worker.extrude_with_cancel(&sample_extrude_request_at(&dir.root), &cancel)
    })
    .expect_err("cancellable oversized staged output must fail closed");
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

#[test]
fn typed_extrude_with_cancel_fails_closed_on_digest_mismatch() {
    let dir = FixtureDir::new("cancel-digest");
    let staged = dir.root.join("out.brep");
    std::fs::write(&staged, b"actual worker bytes").expect("staged file writes");
    let actual = threeterm_occt_worker::sha256_file(&staged).expect("staged file hashes");
    let mut advertised = actual.clone();
    advertised.pop();
    advertised.push('0');

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"req-1\",\"operation\":\"extrude\",\"feature_id\":\"box-1\",\"status\":\"ok\",\"brep_path\":\"{path}\",\"brep_sha256\":\"{advertised}\",\"brep_bytes\":19,\"feature_id\":\"box-1\"}}}}'\n",
            path = staged.display()
        ),
    );
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let error = retry_fixture(|| {
        worker.extrude_with_cancel(&sample_extrude_request_at(&dir.root), &cancel)
    })
    .expect_err("cancellable digest mismatch must fail closed");
    match error {
        WorkerError::Malformed { detail } => {
            assert!(
                detail.contains("digest mismatch"),
                "detail must name the digest mismatch; got {detail:?}"
            );
        }
        other => panic!("expected Malformed; got {other:?}"),
    }
}

#[test]
fn typed_extrude_fails_closed_on_foreign_feature_id_in_result() {
    // The completed result claims a different feature id than the
    // request; the typed boundary must fail closed instead of letting
    // the host commit a foreign identity.
    let dir = FixtureDir::new("foreign-feature");
    let staged = dir.root.join("out.brep");
    std::fs::write(&staged, b"xxxx").expect("staged file writes");
    let digest = threeterm_occt_worker::sha256_file(&staged).expect("staged file hashes");

    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"schema_version\":\"threeterm.workers.occt/1\",\"request_id\":\"req-1\",\"operation\":\"extrude\",\"feature_id\":\"foreign-feature\",\"status\":\"ok\",\"brep_path\":\"{path}\",\"brep_sha256\":\"{digest}\",\"brep_bytes\":4,\"feature_id\":\"foreign-feature\"}}}}'\n",
            path = staged.display()
        ),
    );
    let error = retry_fixture(|| worker.extrude(&sample_extrude_request_at(&dir.root)))
        .expect_err("foreign feature_id must fail closed");
    match error {
        WorkerError::Malformed { detail } => {
            assert!(
                detail.contains("feature_id"),
                "detail must name the feature id; got {detail:?}"
            );
        }
        other => panic!("expected Malformed; got {other:?}"),
    }
}

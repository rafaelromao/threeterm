//! End-to-end integration tests wiring the production `Supervisor` to
//! real fixture subprocesses through `SubprocessWorkerHost`.
//!
//! The fixtures are plain `sh` processes speaking the versioned envelope
//! protocol, so the tests are deterministic and run anywhere (no OCCT
//! needed). They prove the supervised lifecycle over real pipes:
//! handshake negotiation, request/completion round-trip, cancellation,
//! grace-based force termination, and request binding.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use threeterm_protocol::frame::MAX_FRAME_BUFFER;
use threeterm_protocol::supervisor::{ExitKind, Request, Supervisor, SupervisorOutcome};
use threeterm_protocol::worker::{StreamLimits, SubprocessWorkerHost};

const LIMITS: StreamLimits = StreamLimits {
    stdout_bytes: 64 * 1024,
    stderr_bytes: 4096,
};

fn worker_ready_line() -> &'static str {
    "{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}"
}

/// Path of a marker file a fixture worker fills with any bytes it reads
/// from stdin, proving whether the host sent anything on the wire.
fn stdin_marker(label: &str) -> String {
    format!(
        "{}/threeterm-stdin-{label}-{}.log",
        std::env::temp_dir().display(),
        std::process::id()
    )
}

/// True when the fixture worker actually read bytes from stdin (the
/// shell creates the marker file at redirect time, so an empty file
/// means nothing arrived on the wire).
fn stdin_sniffer_saw_bytes(marker: &str) -> bool {
    std::fs::read(marker)
        .map(|bytes| !bytes.is_empty())
        .unwrap_or(false)
}

/// Spawns a fixture that writes every stdin byte it receives into
/// `marker` (blocking until input arrives or the process is killed).
fn spawn_stdin_sniffer(script_before: &str, marker: &str) -> std::process::Child {
    let fixture = format!("{script_before}; head -c 1 > {marker}");
    spawn_shell(&fixture)
}

fn completed_line() -> &'static str {
    "{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"status\":\"ok\"}}"
}

fn cancelled_line() -> &'static str {
    "{\"kind\":\"cancelled\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"reason\":\"user pressed stop\"}"
}

/// Spawns a fixture worker that runs `script` after emitting the
/// WorkerReady handshake line. The script may read stdin lines and emit
/// envelopes.
fn spawn_ready_worker(script: &str) -> std::process::Child {
    let fixture = format!(
        "printf '%s\\n' '{ready}'; {script}",
        ready = worker_ready_line()
    );
    spawn_shell(&fixture)
}

fn spawn_shell(fixture: &str) -> std::process::Child {
    Command::new("sh")
        .arg("-c")
        .arg(fixture)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fixture worker starts in its own process group")
}

fn supervised_host(child: std::process::Child) -> Supervisor {
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    Supervisor::new(Duration::from_millis(2000), Box::new(host), None)
}

fn sample_request() -> Request {
    Request {
        request_id: "req-1".to_string(),
        command_id: "extrude".to_string(),
        args: serde_json::json!({ "height": 3.0 }),
        revision_id: "rev-0".to_string(),
    }
}

#[test]
fn well_behaved_fixture_worker_completes_a_request() {
    let child = spawn_ready_worker(&format!(
        "read line; printf '%s\\n' '{completed}'",
        completed = completed_line()
    ));
    let mut supervisor = supervised_host(child);

    let outcome = supervisor.request(sample_request());
    match outcome {
        SupervisorOutcome::Completed {
            request_id,
            result,
            artifact_headers,
        } => {
            assert_eq!(request_id, "req-1");
            assert_eq!(result["status"], "ok");
            assert!(artifact_headers.is_empty());
        }
        other => panic!("expected Completed; got {other:?}"),
    }
}

#[test]
fn worker_that_never_emits_worker_ready_is_force_terminated_before_request() {
    // The worker never speaks; the handshake must fail closed before any
    // request is sent. The stdin sniffer proves no bytes were written.
    let marker = stdin_marker("no-ready");
    let _ = std::fs::remove_file(&marker);
    let child = spawn_stdin_sniffer("", &marker);
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_millis(300), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.starts_with("handshake_"),
        "stage: {:?}",
        record.stage
    );
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert!(
        !stdin_sniffer_saw_bytes(&marker),
        "no request bytes may be written before the handshake"
    );
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn schema_mismatched_worker_ready_is_rejected_before_request() {
    let marker = stdin_marker("schema");
    let _ = std::fs::remove_file(&marker);
    let child = spawn_stdin_sniffer(
        "printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/0\",\"worker_id\":\"old\"}'",
        &marker,
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_millis(2000), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert!(
        record.stage.starts_with("handshake_schema_mismatch"),
        "stage: {:?}",
        record.stage
    );
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert!(
        !stdin_sniffer_saw_bytes(&marker),
        "an incompatible worker must be rejected before any request is sent"
    );
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn foreign_worker_identity_is_rejected_before_request_dispatch() {
    let marker = stdin_marker("worker-id");
    let _ = std::fs::remove_file(&marker);
    let child = spawn_stdin_sniffer(
        "printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"foreign-worker\"}'",
        &marker,
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None)
        .with_expected_worker_id("fixture");

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("foreign worker must not receive a request: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.starts_with("handshake_worker_id_mismatch"),
        "wrong worker identity must fail during handshake: {:?}",
        record.stage
    );
    assert!(
        !stdin_sniffer_saw_bytes(&marker),
        "wrong worker identity must be rejected before request dispatch"
    );
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn worker_that_ignores_cancel_is_force_terminated_after_grace() {
    // The worker acks nothing; the supervisor must SIGKILL the process
    // group after the cancellation grace expires and report the signal.
    let child = spawn_ready_worker("read line; sleep 100");
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_millis(300), Box::new(host), None);

    let outcome = supervisor.cancel("req-1", "user pressed stop");
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.exit_kind, ExitKind::ForceAfterGrace);
    assert_eq!(record.exit_signal, Some(9), "SIGKILL after grace");
}

#[test]
fn worker_that_acks_cancel_yields_acknowledged() {
    // The worker reads the Cancel line and acknowledges with a
    // Cancelled envelope inside the grace period.
    let child = spawn_ready_worker(&format!(
        "read line; printf '%s\\n' '{cancelled}'",
        cancelled = cancelled_line()
    ));
    let mut supervisor = supervised_host(child);

    let outcome = supervisor.cancel("req-1", "user pressed stop");
    let SupervisorOutcome::Acknowledged {
        request_id, reason, ..
    } = outcome
    else {
        panic!("expected Acknowledged; got {outcome:?}");
    };
    assert_eq!(request_id, "req-1");
    assert_eq!(reason, "user pressed stop");
}

#[test]
fn foreign_request_progress_is_rejected_as_protocol_violation() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"progress\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"other-request\",\"stage\":\"sneaky\",\"percent\":99}'; sleep 100",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_millis(300), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("expected ForceTerminated; got {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    let progress = record
        .last_progress
        .expect("mismatched progress must surface as a protocol violation");
    assert!(
        progress
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        progress.stage
    );
}

#[test]
fn post_request_protocol_schema_mismatch_fails_closed_with_request_context() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"progress\",\"schema_version\":\"threeterm.protocol/0\",\"request_id\":\"req-1\",\"stage\":\"wrong-schema\",\"percent\":1}'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("schema-mismatched response must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.starts_with("envelope_schema_mismatch"),
        "response schema mismatch must be classified structurally: {:?}",
        record.stage
    );
}

#[test]
fn foreign_request_artifact_is_rejected_before_staging() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"artifact\",\"schema_version\":\"threeterm.protocol/1\",\"header\":{\"request_id\":\"other-request\",\"source_revision_id\":\"rev-0\",\"operation\":\"extrude\",\"feature_id\":\"box-1\",\"cache_key\":{\"source_revision_id\":\"rev-0\",\"worker_fingerprint\":{\"worker_kind\":\"occt\",\"worker_schema_version\":\"threeterm.workers.occt/1\",\"protocol_schema_version\":\"threeterm.protocol/1\"},\"operation\":\"extrude\",\"feature_id\":\"box-1\",\"artifact_kind\":\"brep\",\"semantic_input_sha256\":\"1111111111111111111111111111111111111111111111111111111111111111\",\"deterministic_settings_sha256\":\"2222222222222222222222222222222222222222222222222222222222222222\"},\"worker_fingerprint\":{\"worker_kind\":\"occt\",\"worker_schema_version\":\"threeterm.workers.occt/1\",\"protocol_schema_version\":\"threeterm.protocol/1\"},\"artifact_kind\":\"brep\",\"staging_name\":\"foreign.brep\",\"byte_count\":0,\"sha256\":\"\"}}'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("foreign artifact must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    let progress = record
        .last_progress
        .expect("foreign artifact must surface a protocol violation");
    assert!(
        progress
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        progress.stage
    );
}

#[test]
fn foreign_request_completion_is_rejected_before_result_promotion() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"other-request\",\"result\":{\"ok\":true}}'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("foreign completion must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record
            .stage
            .starts_with("protocol_violation:completed_request_id_mismatch"),
        "stage: {:?}",
        record.stage
    );
}

#[test]
fn foreign_request_failure_is_not_adopted_as_the_active_failure() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"failed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"other-request\",\"code\":\"foreign\",\"detail\":\"not this request\"}'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("foreign failure must not be accepted: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert_eq!(record.failed_code, None);
    assert!(
        record
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        record.stage
    );
}

#[test]
fn foreign_cancellation_acknowledgement_is_rejected_with_active_request_id() {
    let child = spawn_ready_worker(
        "read line; printf '%s\\n' '{\"kind\":\"cancelled\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"other-request\",\"reason\":\"wrong request\"}'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.cancel("req-1", "stop");
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("foreign cancellation acknowledgement must fail closed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record
            .stage
            .starts_with("protocol_violation:mismatched_request_id:"),
        "stage: {:?}",
        record.stage
    );
}

#[test]
fn malformed_cancellation_acknowledgement_is_diagnosed_and_worker_is_killed() {
    let child = spawn_ready_worker(
        "read line; printf '%s\\n' '{\\\"kind\\\":\\\"cancelled\\\",\\\"schema_version\\\":'; sleep 100",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.cancel("req-1", "stop");
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("malformed cancellation acknowledgement must fail closed: {outcome:?}");
    };
    assert_eq!(
        record
            .protocol_diagnostic
            .expect("malformed ack has structured diagnostic")
            .code
            .as_str(),
        "malformed_cancellation_acknowledgement"
    );
    assert_eq!(record.exit_signal, Some(9));
}

#[test]
fn schema_mismatched_cancellation_acknowledgement_is_rejected() {
    let child = spawn_ready_worker(
        "read line; printf '%s\\n' '{\"kind\":\"cancelled\",\"schema_version\":\"threeterm.protocol/0\",\"request_id\":\"req-1\",\"reason\":\"stop\"}'; sleep 100",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.cancel("req-1", "stop");
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("schema-mismatched cancellation acknowledgement must fail closed: {outcome:?}");
    };
    assert_eq!(
        record
            .protocol_diagnostic
            .expect("schema mismatch has structured diagnostic")
            .code
            .as_str(),
        "cancellation_acknowledgement_schema_mismatch"
    );
    assert_eq!(record.exit_signal, Some(9));
}

#[test]
fn cancellation_token_is_observed_mid_flight_well_before_the_deadline() {
    // A silent worker (never emits a terminal envelope) with a request
    // grace of 2s. The cancel token is set 300ms after the request
    // starts; the supervisor must observe it on a receive slice (~50ms)
    // and enter the cancellation lifecycle, so the outcome is a
    // cancellation (not the request grace expiring).
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    let child = spawn_ready_worker("read line; sleep 100");
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(2), Box::new(host), None);
    let cancel = Arc::new(AtomicBool::new(false));

    let started = Instant::now();
    let trigger = Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        trigger.store(true, Ordering::SeqCst);
    });
    let mut request = sample_request();
    request.request_id = "req-1".to_string();
    let outcome = supervisor.request_with_cancel(request, cancel.as_ref());
    let elapsed = started.elapsed();
    // The cancellation lifecycle runs the cancel-ack wait inside the
    // supervisor grace; the whole exchange finishes by the 2s grace
    // because the token was observed mid-flight.
    assert!(
        elapsed < Duration::from_secs(4),
        "cancellation must be observed mid-flight; took {elapsed:?}"
    );
    match outcome {
        SupervisorOutcome::ForceTerminated { record } => {
            assert!(
                record.stage.starts_with("cancel_"),
                "expected a cancellation lifecycle stage; got {:?}",
                record.stage
            );
        }
        other => panic!("expected ForceTerminated; got {other:?}"),
    }
}

#[test]
fn completed_after_stdout_flood_never_returns_success() {
    // The worker emits WorkerReady, then writes a valid Completed and a
    // stdout flood in one shell write. The supervisor must settle the
    // reader state and fail the terminal outcome closed — never return
    // Completed. Keeping both records in the same write makes the race
    // deterministic without delaying production termination.
    use threeterm_protocol::supervisor::{Request, SupervisorOutcome};
    let fixture = "trap '' PIPE; printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; read line; printf '%s\\n%65536s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}' ''; sleep 1";
    let child = spawn_shell(fixture);
    let host = SubprocessWorkerHost::with_limits(
        child,
        StreamLimits {
            stdout_bytes: 16 * 1024,
            stderr_bytes: 2048,
        },
    )
    .expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(5), Box::new(host), None);

    let outcome = supervisor.request(Request {
        request_id: "req-1".to_string(),
        command_id: "extrude".to_string(),
        args: serde_json::json!({}),
        revision_id: String::new(),
    });
    match outcome {
        SupervisorOutcome::ForceTerminated { record } => {
            assert_eq!(record.request_id, "req-1");
            // The overflow surfaces either as a stream_overflow stage
            // (settled before terminal acceptance) or as a
            // worker_recv_error naming the bound (caught mid-receive).
            // Both fail closed; the invariant is that Completed is
            // never returned.
            assert!(
                record.stage.contains("overflow")
                    || record.stage.contains("worker_recv_error")
                    || record.stage.contains("completed_terminal_finalize_failed"),
                "stage must name the overflow; got {:?}",
                record.stage
            );
        }
        SupervisorOutcome::Completed { .. } => {
            panic!("Completed must never be returned after a stream overflow");
        }
        other => panic!("expected ForceTerminated; got {other:?}"),
    }
}

#[test]
fn completed_after_stderr_flood_never_returns_success() {
    let fixture = "trap '' PIPE; printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; read line; printf '%s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}'; while true; do head -c 65536 /dev/zero 1>&2; done";
    let child = spawn_shell(fixture);
    let host = SubprocessWorkerHost::with_limits(
        child,
        StreamLimits {
            stdout_bytes: 64 * 1024,
            stderr_bytes: 4096,
        },
    )
    .expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(5), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("Completed must never be returned after a stderr overflow: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.contains("stderr") || record.stage.contains("overflow"),
        "stderr overflow must remain visible in the structured failure: {:?}",
        record.stage
    );
    assert!(
        record.stderr_tail.len() <= 4096,
        "diagnostic stderr tail must remain bounded: {}",
        record.stderr_tail.len()
    );
}

#[test]
fn completion_followed_by_trailing_envelope_fails_closed_before_promotion() {
    // Both terminal and trailing frames are written in one pipe chunk. The
    // supervisor must inspect the bounded transport's pending frames before
    // accepting the terminal result.
    let fixture = "printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; read line; printf '%s\\n%s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}' '{\"kind\":\"progress\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"stage\":\"trailing\",\"percent\":100}'; sleep 1";
    let child = spawn_shell(fixture);
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(5), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("trailing data must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.contains("trailing") || record.stage.contains("protocol"),
        "termination must identify the trailing protocol data: {:?}",
        record.stage
    );
}

#[test]
fn unterminated_frame_below_the_bound_fails_closed_at_the_request_deadline() {
    let fixture = "printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; read line; printf '%s' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}'; sleep 1";
    let child = spawn_shell(fixture);
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_millis(300), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("unterminated frame must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.starts_with("grace_exceeded")
            || record.stage.starts_with("worker_closed")
            || record
                .stage
                .starts_with("completed_terminal_finalize_failed"),
        "unterminated frame must fail at the bounded request boundary: {:?}",
        record.stage
    );
}

#[test]
fn oversized_unterminated_frame_is_rejected_by_the_frame_bound() {
    let fixture = format!(
        "printf '%s\\n' '{ready}'; read line; head -c {} /dev/zero",
        MAX_FRAME_BUFFER + 1,
        ready = worker_ready_line(),
    );
    let child = spawn_shell(&fixture);
    let host = SubprocessWorkerHost::with_limits(
        child,
        StreamLimits {
            stdout_bytes: MAX_FRAME_BUFFER + 4096,
            stderr_bytes: 4096,
        },
    )
    .expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(5), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("oversized unterminated frame must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.contains("frame buffer") || record.stage.contains("worker_recv_error"),
        "frame-bound rejection must identify the bounded parser failure: {:?}",
        record.stage
    );
}

#[test]
fn malformed_worker_frame_fails_closed_with_the_active_request_context() {
    let child = spawn_ready_worker("printf '%s\\n' 'not-json'; sleep 1");
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("malformed frame must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.contains("worker_recv_error")
            || record.stage.contains("frame is not valid JSON"),
        "malformed frame must produce a structured protocol failure: {:?}",
        record.stage
    );
}

#[test]
fn trailing_data_on_a_worker_frame_fails_closed_before_result_acceptance() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}trailing'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("trailing frame data must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.contains("worker_recv_error")
            || record.stage.contains("trailing")
            || record.stage.contains("frame is not valid JSON"),
        "trailing frame data must be a protocol failure: {:?}",
        record.stage
    );
}

#[test]
fn worker_request_envelope_in_the_response_direction_fails_closed() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"request\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"command_id\":\"extrude\",\"args\":{},\"revision_id\":\"rev-0\"}'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("worker Request must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert_eq!(
        record.stage,
        "protocol_violation:worker_sent_host_only_envelope"
    );
}

#[test]
fn worker_cancel_envelope_in_the_response_direction_fails_closed() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"cancel\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"reason\":\"foreign direction\"}'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("worker Cancel must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert_eq!(
        record.stage,
        "protocol_violation:worker_sent_host_only_envelope"
    );
}

#[test]
fn completion_followed_by_raw_trailing_bytes_fails_closed() {
    let child = spawn_ready_worker(
        "printf '%s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}'; printf '%s' 'trailing-bytes'; sleep 1",
    );
    let host = SubprocessWorkerHost::with_limits(child, LIMITS).expect("transport starts");
    let mut supervisor = Supervisor::new(Duration::from_secs(1), Box::new(host), None);

    let outcome = supervisor.request(sample_request());
    let SupervisorOutcome::ForceTerminated { record } = outcome else {
        panic!("raw trailing bytes must not return Completed: {outcome:?}");
    };
    assert_eq!(record.request_id, "req-1");
    assert!(
        record.stage.contains("trailing") || record.stage.contains("worker_recv_error"),
        "raw trailing bytes must be rejected: {:?}",
        record.stage
    );
}

//! Bounded subprocess transport integration tests.
//!
//! Wires the production `SubprocessWorkerHost` against real fixture
//! subprocesses to prove the bounds the supervised worker contract
//! requires: stdout and stderr byte caps that fail closed on overflow,
//! and a host-side input bound that rejects oversized frames at send.
//! The fixtures are plain `sh` processes so the tests are deterministic
//! and run anywhere (no OCCT needed).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use std::os::unix::process::CommandExt;

use threeterm_protocol::frame::MAX_FRAME_BUFFER;
use threeterm_protocol::worker::{
    Envelope, StreamLimits, SubprocessWorkerHost, WorkerError, WorkerHost,
};

fn spawn_flooding_worker() -> std::process::Child {
    // Emits the canonical WorkerReady line, then floods stdout with
    // small valid frames forever. The worker itself never exits, so the
    // host must fail closed and terminate it.
    let fixture = "printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; while true; do printf '%s\\n' '{\"kind\":\"progress\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"stage\":\"flood\",\"percent\":1}'; done";
    Command::new("sh")
        .arg("-c")
        .arg(fixture)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("flooding worker starts")
}

/// Drives `recv` until the worker fails closed or the deadline expires.
/// Returns the first non-tick error the transport surfaced. Receive
/// slices return `TimedOut` as poll ticks, so they are skipped.
fn recv_until_error(host: &mut SubprocessWorkerHost, deadline: Instant) -> WorkerError {
    loop {
        match host.recv(deadline) {
            Ok(_envelope) => continue,
            Err(WorkerError::TimedOut) => continue,
            Err(error) => return error,
        }
    }
}

#[test]
fn flooding_stdout_worker_fails_closed_with_structured_overflow_error() {
    let child = spawn_flooding_worker();
    let limits = StreamLimits {
        stdout_bytes: 16 * 1024,
        stderr_bytes: 1024,
    };
    let mut host =
        SubprocessWorkerHost::with_limits(child, limits).expect("flooding worker transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    let error = recv_until_error(&mut host, deadline);

    match error {
        WorkerError::StreamOverflow { stream, limit } => {
            assert_eq!(stream, "stdout");
            assert_eq!(limit, 16 * 1024);
        }
        other => panic!("expected StreamOverflow; got {other:?}"),
    }

    // The flooding worker must be terminated and reaped; it never exits
    // on its own.
    host.terminate().expect("flooding worker terminates");
}

#[test]
fn flooding_stdout_worker_is_dead_after_terminate() {
    let child = spawn_flooding_worker();
    let pid = child.id();
    let limits = StreamLimits {
        stdout_bytes: 16 * 1024,
        stderr_bytes: 1024,
    };
    let mut host =
        SubprocessWorkerHost::with_limits(child, limits).expect("flooding worker transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    let _ = recv_until_error(&mut host, deadline);
    host.terminate().expect("flooding worker terminates");

    let still_alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
    assert!(
        !still_alive,
        "flooding worker must be reaped, pid {pid} still alive"
    );
}

#[test]
fn flooding_stderr_worker_fails_closed_and_preserves_bounded_tail() {
    // Emits WorkerReady on stdout, then floods stderr with binary data
    // and never exits. The stderr cap must fail the host closed while
    // keeping at most `cap` bytes of the tail for diagnostics.
    let fixture = "printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; while true; do head -c 65536 /dev/zero 1>&2; done";
    let child = Command::new("sh")
        .arg("-c")
        .arg(fixture)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stderr-flooding worker starts");
    let limits = StreamLimits {
        stdout_bytes: 16 * 1024,
        stderr_bytes: 2048,
    };
    let mut host = SubprocessWorkerHost::with_limits(child, limits)
        .expect("stderr-flooding worker transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    let error = recv_until_error(&mut host, deadline);
    match error {
        WorkerError::StreamOverflow { stream, limit } => {
            assert_eq!(stream, "stderr");
            assert_eq!(limit, 2048);
        }
        other => panic!("expected StreamOverflow; got {other:?}"),
    }

    let tail = host.stderr_tail();
    assert!(
        host.stderr_tail_bytes().len() <= 2048,
        "raw stderr tail must not exceed the cap"
    );
    assert!(
        tail.len() <= 2048,
        "bounded stderr tail must not exceed the cap; got {} bytes",
        tail.len()
    );
    assert!(
        !tail.is_empty(),
        "the captured stderr tail must be preserved"
    );
    host.terminate().expect("stderr-flooding worker terminates");
}

#[test]
fn oversized_host_frame_is_rejected_at_send() {
    let child = Command::new("sh")
        .arg("-c")
        .arg("exec cat >/dev/null")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("draining worker starts");
    let mut host = SubprocessWorkerHost::new(child).expect("draining worker transport starts");

    let oversized = Envelope::Request {
        schema_version: threeterm_protocol::schema_version().to_string(),
        request_id: "req-1".to_string(),
        command_id: "extrude".to_string(),
        args: serde_json::json!({
            "payload": "x".repeat(MAX_FRAME_BUFFER + 1),
        }),
        revision_id: "".to_string(),
    };

    match host.send(&oversized) {
        Err(WorkerError::Protocol(detail)) => {
            assert!(
                detail.contains("bound"),
                "oversize rejection should name the bound; got {detail:?}"
            );
        }
        other => panic!("expected Protocol rejection; got {other:?}"),
    }
    host.terminate().expect("draining worker terminates");
}

#[test]
fn terminal_completion_racing_a_stdout_overflow_fails_closed() {
    // The worker emits WorkerReady, a valid Completed, and then floods
    // stdout past the bound. The terminal outcome must fail closed even
    // though the completion envelope arrived first. SIGPIPE is ignored
    // so the flood continues after the host stops reading, keeping the
    // worker alive until the overflow flag is observed.
    let fixture = "trap '' PIPE; printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; printf '%s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}'; while true; do printf '%s\\n' '{\"kind\":\"progress\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"stage\":\"flood\",\"percent\":1}'; done";
    let child = Command::new("sh")
        .arg("-c")
        .arg(fixture)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("flooding-after-completion worker starts");
    let limits = StreamLimits {
        stdout_bytes: 16 * 1024,
        stderr_bytes: 1024,
    };
    let mut host = SubprocessWorkerHost::with_limits(child, limits).expect("transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    // The worker delivers a valid Completed then floods stdout past the
    // bound. Whether the completion is delivered first or the overflow
    // flag lands first, the host must fail closed with StreamOverflow —
    // never a clean completion.
    let error = loop {
        match host.recv(deadline) {
            Ok(_envelope) => continue,
            Err(WorkerError::TimedOut) => continue,
            Err(error) => break error,
        }
    };
    match error {
        WorkerError::StreamOverflow { stream, .. } => {
            assert_eq!(stream, "stdout");
        }
        other => panic!("expected StreamOverflow; got {other:?}"),
    }
    host.terminate().expect("flooding worker terminates");
}

#[test]
fn clean_exit_after_overflow_never_accepts_completion() {
    // The worker emits WorkerReady, floods stdout past the bound, then
    // exits cleanly. Even a clean exit must not rescue the flood: the
    // host fails closed with a structured overflow error. SIGPIPE is
    // ignored so the flood continues after the host stops reading.
    let fixture = "trap '' PIPE; printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; printf '%65536s\\n' ''; sleep 1; exit 0";
    let child = Command::new("sh")
        .arg("-c")
        .arg(fixture)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("flooding worker starts");
    let limits = StreamLimits {
        stdout_bytes: 16 * 1024,
        stderr_bytes: 1024,
    };
    let mut host = SubprocessWorkerHost::with_limits(child, limits).expect("transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    let error = loop {
        match host.recv(deadline) {
            Ok(_) => continue,
            Err(WorkerError::TimedOut) => continue,
            Err(error) => break error,
        }
    };
    match error {
        WorkerError::StreamOverflow { stream, .. } => {
            assert_eq!(stream, "stdout");
        }
        other => panic!("expected StreamOverflow; got {other:?}"),
    }
    host.terminate().expect("flooding worker terminates");
}

#[test]
fn worker_whose_stdout_stays_open_past_the_drain_window_is_contained() {
    // The worker emits WorkerReady and Completed, then exits cleanly,
    // but leaves a daemonized (setsid) descendant holding stdout open. The
    // host must contain the inherited pipe even after the leader exits.
    let fixture = "printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'; printf '%s\\n' '{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{\"ok\":true}}'; setsid sh -c 'sleep 5' >&1 & exit 0";
    let child = Command::new("sh")
        .arg("-c")
        .arg(fixture)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("daemonizing worker starts");
    let mut host = SubprocessWorkerHost::new(child).expect("transport starts");

    let deadline = Instant::now() + Duration::from_secs(5);
    assert!(
        matches!(host.recv(deadline), Ok(Envelope::WorkerReady { .. })),
        "worker ready must arrive"
    );
    assert!(
        matches!(host.recv(deadline), Ok(Envelope::Completed { .. })),
        "completion must arrive"
    );

    host.terminate()
        .expect("termination must contain the daemonized descendant");
}

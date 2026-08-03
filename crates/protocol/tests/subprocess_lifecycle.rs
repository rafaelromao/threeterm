//! Subprocess lifecycle integration tests.
//!
//! Wires the production `SubprocessWorkerHost` against real fixture
//! subprocesses to prove the lifecycle contract: workers live in their
//! own process group so termination kills and reaps the whole tree, and
//! signal-based exits surface the actual Linux signal instead of a bare
//! closed-stream error.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use threeterm_protocol::worker::{
    Envelope, StreamLimits, SubprocessWorkerHost, WorkerError, WorkerHost,
};

const SMALL_LIMITS: StreamLimits = StreamLimits {
    stdout_bytes: 16 * 1024,
    stderr_bytes: 2048,
};

fn worker_ready_line() -> &'static str {
    "{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}"
}

fn spawn_in_group(fixture: &str) -> std::process::Child {
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

fn pid_alive(pid: u32) -> bool {
    // A zombie (state Z) is dead but not yet reaped; in containers PID 1
    // may never reap, so "alive" means the process is in a runnable
    // state, not merely that a /proc entry persists.
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
        return false;
    };
    matches!(rest.split_whitespace().next(), Some("R" | "S" | "D"))
}

fn wait_until(deadline: Instant, mut predicate: impl FnMut() -> bool) -> bool {
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    predicate()
}

#[test]
fn crashed_worker_surfaces_the_actual_signal() {
    let fixture = format!(
        "printf '%s\\n' '{worker}'; kill -SEGV $$",
        worker = worker_ready_line()
    );
    let child = spawn_in_group(&fixture);
    let mut host =
        SubprocessWorkerHost::with_limits(child, SMALL_LIMITS).expect("transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    match host.recv(deadline).expect("worker ready arrives") {
        Envelope::WorkerReady { worker_id, .. } => assert_eq!(worker_id, "fixture"),
        other => panic!("expected WorkerReady; got {other:?}"),
    }

    let error = host
        .recv(deadline)
        .expect_err("crashed worker must fail the host closed");
    match error {
        WorkerError::Signalled { signal } => {
            assert_eq!(signal, 11, "SIGSEGV must be reported as signal 11");
        }
        other => panic!("expected Signalled; got {other:?}"),
    }
    assert_eq!(
        host.exit_signal(),
        Some(11),
        "the reaped exit status must preserve the signal"
    );
}

#[test]
fn terminate_kills_and_reaps_descendants_in_the_process_group() {
    let desc_marker = PathBuf::from(format!(
        "{}/threeterm-desc-{}.pid",
        std::env::temp_dir().display(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&desc_marker);
    let fixture = format!(
        "printf '%s\\n' '{worker}'; sleep 300 & echo $! > {marker}; wait",
        worker = worker_ready_line(),
        marker = desc_marker.display()
    );
    let child = spawn_in_group(&fixture);
    let worker_pid = child.id();
    let mut host =
        SubprocessWorkerHost::with_limits(child, SMALL_LIMITS).expect("transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    assert!(
        wait_until(deadline, || desc_marker.exists()),
        "fixture must record its descendant pid"
    );
    let descendant_pid: u32 = std::fs::read_to_string(&desc_marker)
        .expect("descendant pid file reads")
        .trim()
        .parse()
        .expect("descendant pid parses");
    assert!(
        pid_alive(descendant_pid),
        "descendant must be alive before terminate"
    );

    host.terminate().expect("terminate kills the process group");

    // SIGKILL delivery is asynchronous; wait for both processes to die
    // (descendants become zombies, state Z, which the container's PID 1
    // may never reap).
    let death_deadline = Instant::now() + Duration::from_secs(5);
    assert!(
        wait_until(death_deadline, || !pid_alive(worker_pid)),
        "worker must be reaped after terminate (pid {worker_pid})"
    );
    assert!(
        wait_until(death_deadline, || !pid_alive(descendant_pid)),
        "descendant must be killed with the worker (pid {descendant_pid})"
    );
    let _ = std::fs::remove_file(&desc_marker);
}

#[test]
fn force_terminate_reports_the_kill_signal() {
    // The worker never exits on its own; the host must SIGKILL the
    // process group and the reaped exit status must report SIGKILL.
    let fixture = format!(
        "printf '%s\\n' '{worker}'; sleep 300",
        worker = worker_ready_line()
    );
    let child = spawn_in_group(&fixture);
    let mut host =
        SubprocessWorkerHost::with_limits(child, SMALL_LIMITS).expect("transport starts");

    host.terminate().expect("force terminate succeeds");
    assert_eq!(
        host.exit_signal(),
        Some(9),
        "SIGKILL must be reported as signal 9 after force termination"
    );
}

#[test]
fn clean_worker_exit_reports_no_signal() {
    let fixture = format!(
        "printf '%s\\n' '{worker}'; printf '%s\\n' '{{\"kind\":\"completed\",\"schema_version\":\"threeterm.protocol/1\",\"request_id\":\"req-1\",\"result\":{{\"ok\":true}}}}'; exit 0",
        worker = worker_ready_line()
    );
    let child = spawn_in_group(&fixture);
    let mut host =
        SubprocessWorkerHost::with_limits(child, SMALL_LIMITS).expect("transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    let _ = host.recv(deadline).expect("worker ready arrives");
    let _ = host.recv(deadline).expect("completed envelope arrives");
    host.terminate().expect("clean worker terminates");
    assert_eq!(host.exit_signal(), None, "a clean exit carries no signal");
}

#[test]
fn terminate_kills_descendants_even_after_the_leader_exits_first() {
    // The leader emits WorkerReady, spawns a background descendant, and
    // exits cleanly. Termination must still kill the process group so
    // the descendant does not survive the reaped leader.
    let desc_marker = PathBuf::from(format!(
        "{}/threeterm-desc-leader-{}.pid",
        std::env::temp_dir().display(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&desc_marker);
    let fixture = format!(
        "printf '%s\\n' '{worker}'; sleep 300 & echo $! > {marker}; exit 0",
        worker = worker_ready_line(),
        marker = desc_marker.display()
    );
    let child = spawn_in_group(&fixture);
    let mut host =
        SubprocessWorkerHost::with_limits(child, SMALL_LIMITS).expect("transport starts");

    let deadline = Instant::now() + Duration::from_secs(10);
    assert!(
        wait_until(deadline, || desc_marker.exists()),
        "fixture must record its descendant pid"
    );
    let descendant_pid: u32 = std::fs::read_to_string(&desc_marker)
        .expect("descendant pid file reads")
        .trim()
        .parse()
        .expect("descendant pid parses");
    assert!(
        pid_alive(descendant_pid),
        "descendant must be alive before terminate"
    );

    host.terminate().expect("terminate kills the process group");

    // SIGKILL delivery is asynchronous; wait for the descendant to die
    // (it becomes a zombie, state Z, which the container's PID 1 may
    // never reap).
    let death_deadline = Instant::now() + Duration::from_secs(5);
    assert!(
        wait_until(death_deadline, || !pid_alive(descendant_pid)),
        "descendant must be killed with the group (pid {descendant_pid})"
    );
    let _ = std::fs::remove_file(&desc_marker);
}

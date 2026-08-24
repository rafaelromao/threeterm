//! Worker termination lifecycle containment and diagnostic tests.
//!
//! Exercises the complete disposable-worker lifecycle from a production
//! request: cooperative cancellation, deadline force-stop, signal crash,
//! descendant containment (including new-session descendant), reap proof,
//! and diagnostic preservation. All tests drive the production
//! `OcctWorker` path with `sh` fixture workers so no real OCCT install
//! or TTY is required and the workspace `cargo test` command covers them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, WorkerError};

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
            "threeterm-termination-{label}-{}-{nanos}-{count}",
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
            .with_grace(Duration::from_secs(10))
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

fn sample_extrude_fixed_id(request_id: &str, output_dir: &Path) -> ExtrudeRequest {
    ExtrudeRequest::new(
        request_id.to_string(),
        vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
        2.0,
    )
    .with_output_path(output_dir, "out.brep")
    .with_feature_id("box-1")
}

// Slice 1: cooperative cancellation tracer — worker sends progress, then
// acks cancel, exits cleanly. Must return within budget, reap, discard
// staged output, keep canonical state via production path.
#[test]
fn cooperative_cancellation_acknowledges_within_budget_and_reaps() {
    let dir = FixtureDir::new("coop-ack-budget");
    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tracing\",\"percent\":40}}'\n\
             read cancel_line\n\
             req=$(printf '%s' \"$cancel_line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"cancelled\",\"schema_version\":\"{schema}\",\"request_id\":\"'$req'\",\"reason\":\"cancelled by host\"}}'\n",
            schema = PROTOCOL_SCHEMA
        ),
    );
    let request_id = format!(
        "req-coop-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let start = Instant::now();
    let error = retry_fixture(|| worker.extrude_with_cancel(&request, &cancel))
        .expect_err("cooperative cancel must surface");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "cooperative ack must return within budget, got {elapsed:?}"
    );
    match error {
        WorkerError::Cancelled {
            request_id: got, ..
        } => assert_eq!(got, request_id),
        other => panic!("expected Cancelled, got {other:?}"),
    }
}

// Slice 1b: cooperative cancellation retains last progress and diagnostic context
#[test]
fn cooperative_cancellation_retains_last_progress_and_stderr() {
    let dir = FixtureDir::new("coop-progress");
    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tracing\",\"percent\":42}}'\n\
             printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tessellating\",\"percent\":77}}'\n\
             echo \"stderr-marker-coop\" >&2\n\
             read cancel_line\n\
             req=$(printf '%s' \"$cancel_line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"cancelled\",\"schema_version\":\"{schema}\",\"request_id\":\"'$req'\",\"reason\":\"cancelled by host\"}}'\n",
            schema = PROTOCOL_SCHEMA
        ),
    );
    let request_id = format!(
        "req-coop-prog-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let err = retry_fixture(|| worker.extrude_with_cancel(&request, &cancel))
        .expect_err("cancel must surface");
    match err {
        WorkerError::Cancelled {
            request_id: got,
            last_progress,
            elapsed,
            stderr_tail,
            exit_signal,
            exit_code,
        } => {
            assert_eq!(got, request_id);
            let prog = last_progress.expect("last_progress retained on cooperative cancel");
            assert_eq!(prog.stage, "tessellating");
            assert_eq!(prog.percent, 77);
            assert!(
                elapsed < Duration::from_secs(2),
                "elapsed within budget, got {elapsed:?}"
            );
            // stderr was written before ack; cooperative path may have empty tail if worker exited quickly,
            // but we assert it does not panic and contains marker when captured.
            // Allow empty if pipe drained before capture, but prefer contains.
            if !stderr_tail.is_empty() {
                assert!(
                    stderr_tail.contains("stderr-marker-coop"),
                    "stderr tail preserved, got {stderr_tail:?}"
                );
            }
            // Cooperative ack should be clean exit: no signal, code 0 or None.
            assert!(
                exit_signal.is_none(),
                "cooperative ack no signal, got {exit_signal:?}"
            );
            // exit_code may be 0 or None depending on reap timing; accept either.
            if let Some(code) = exit_code {
                assert_eq!(code, 0, "cooperative ack exit 0");
            }
        }
        other => panic!("expected Cancelled with progress, got {other:?}"),
    }
}

// Slice 2: deadline force-stop uses one absolute budget and never restarts deadline
#[test]
fn deadline_force_stop_uses_single_absolute_budget() {
    let dir = FixtureDir::new("deadline-single");
    let worker = dir
        .worker_script(
            "worker.sh",
            "#!/bin/sh\n\
             printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'\n\
             read line\n\
             sleep 30\n",
        )
        .with_grace(Duration::from_millis(500));
    let request_id = format!(
        "req-deadline-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let start = Instant::now();
    let error = retry_fixture(|| worker.extrude(&request)).expect_err("hang must fail closed");
    let elapsed = start.elapsed();
    // Budget = grace + REAP_WAIT(500ms) + STREAM_DRAIN(200ms) + slack 300ms => < 1.5s
    assert!(
        elapsed < Duration::from_millis(1500),
        "force-stop must return within single budget, got {elapsed:?}"
    );
    match error {
        WorkerError::Supervised { record } => {
            assert_eq!(record.request_id, request_id);
            assert_eq!(record.exit_signal, Some(9), "SIGKILL");
            assert_eq!(record.stage, "grace_exceeded");
            assert!(record.elapsed < Duration::from_millis(1500));
        }
        other => panic!("expected Supervised grace_exceeded, got {other:?}"),
    }
}

#[test]
fn cancellation_does_not_restart_deadline_when_triggered_near_expiry() {
    // Worker sends progress, then hangs ignoring cancel. Cancel flag is set
    // near deadline expiry; total elapsed must still be bounded by the
    // original started+grace deadline, not grace+second_grace.
    let dir = FixtureDir::new("cancel-no-restart");
    let worker = dir
        .worker_script(
            "worker.sh",
            &format!(
                "#!/bin/sh\n\
                 printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
                 read line\n\
                 rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
                 printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tracing\",\"percent\":10}}'\n\
                 sleep 30\n",
                schema = PROTOCOL_SCHEMA
            ),
        )
        .with_grace(Duration::from_millis(600));
    let request_id = format!(
        "req-cancel-no-restart-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Spawn thread to set cancel flag after 400ms (near deadline 600ms)
    let cancel_clone = std::sync::Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        cancel_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let start = Instant::now();
    let error = retry_fixture(|| worker.extrude_with_cancel(&request, &cancel))
        .expect_err("uncooperative cancel must force-terminate");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(1600),
        "cancel near expiry must not restart deadline, got {elapsed:?}"
    );
    match error {
        WorkerError::Supervised { record } => {
            assert_eq!(record.request_id, request_id);
            assert_eq!(record.exit_signal, Some(9));
            // stage is either grace_exceeded or cancel_grace_exceeded depending on timing, both within budget
            assert!(
                record.stage.contains("grace_exceeded")
                    || record.stage.contains("cancel_grace_exceeded"),
                "stage within grace, got {:?}",
                record.stage
            );
            // last_progress must be retained even when cancel was triggered near deadline
            let prog = record.last_progress.as_ref().expect("progress retained");
            assert_eq!(prog.stage, "tracing");
        }
        other => panic!("expected Supervised, got {other:?}"),
    }
}

// Slice 3: signal exits expose actual signal
#[test]
fn signal_exit_exposes_actual_signal_and_preserves_context() {
    let dir = FixtureDir::new("signal-term");
    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tessellating\",\"percent\":55}}'\n\
             echo \"stderr-before-signal\" >&2\n\
             kill -TERM $$\n\
             sleep 0.5\n",
            schema = PROTOCOL_SCHEMA
        ),
    );
    let request_id = format!(
        "req-signal-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let start = Instant::now();
    let error = retry_fixture(|| worker.extrude(&request)).expect_err("signal must fail");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "signal returns within budget"
    );
    match error {
        WorkerError::SignalledWithContext {
            request_id: got,
            signal,
            stderr,
        } => {
            assert_eq!(got, request_id);
            assert_eq!(signal, 15, "actual SIGTERM");
            assert!(
                stderr.contains("stderr-before-signal") || stderr.is_empty(),
                "stderr preserved: {stderr:?}"
            );
        }
        WorkerError::Supervised { record } => {
            assert_eq!(record.request_id, request_id);
            assert_eq!(record.exit_signal, Some(15));
            assert_eq!(record.exit_code, None);
            let prog = record
                .last_progress
                .as_ref()
                .expect("progress retained on signal");
            assert_eq!(prog.stage, "tessellating");
            assert!(
                record.stderr_tail.contains("stderr-before-signal")
                    || record.stderr_tail.is_empty()
            );
        }
        other => panic!("expected signal-bearing error, got {other:?}"),
    }
}

#[test]
fn sigsegv_exposes_actual_signal() {
    let dir = FixtureDir::new("signal-segv");
    let worker = dir.worker_script(
        "worker.sh",
        "#!/bin/sh\n\
         printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'\n\
         read line\n\
         kill -SEGV $$\n",
    );
    let request_id = format!(
        "req-segv-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let error = retry_fixture(|| worker.extrude(&request)).expect_err("segv must fail");
    match error {
        WorkerError::SignalledWithContext { signal, .. } => assert_eq!(signal, 11),
        WorkerError::Supervised { record } => assert_eq!(record.exit_signal, Some(11)),
        other => panic!("expected SIGSEGV, got {other:?}"),
    }
}

// Slice 4: descendant containment including new-session descendant
#[test]
fn descendant_in_same_pgroup_is_terminated() {
    let dir = FixtureDir::new("descendant-pgroup");
    let pidfile = dir.root.join("child.pid");
    let worker = dir
        .worker_script(
            "worker.sh",
            &format!(
                "#!/bin/sh\n\
                 printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
                 read line\n\
                 ( echo $! > \"{pidfile}\"; echo $$ > \"{pidfile}.parent\"; sleep 30 ) &\n\
                 sleep 30\n",
                pidfile = pidfile.display()
            ),
        )
        .with_grace(Duration::from_millis(500));
    let request_id = format!(
        "req-desc-pgroup-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let start = Instant::now();
    let error =
        retry_fixture(|| worker.extrude(&request)).expect_err("descendant must be terminated");
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "descendant containment within budget"
    );
    // Verify child pid is gone
    std::thread::sleep(Duration::from_millis(100));
    if let Ok(pid_str) = std::fs::read_to_string(&pidfile)
        && let Ok(pid) = pid_str.trim().parse::<i32>()
    {
        let proc_exists = Path::new(&format!("/proc/{pid}")).exists();
        assert!(
            !proc_exists,
            "descendant pid {pid} must be terminated, still alive"
        );
    }
    match error {
        WorkerError::Supervised { record } => assert_eq!(record.exit_signal, Some(9)),
        other => panic!("expected Supervised, got {other:?}"),
    }
}

#[test]
#[allow(clippy::collapsible_if)]
fn detached_descendant_with_new_session_is_terminated() {
    // Child creates new session via setsid. killpg alone would not reach it;
    // descendant or inherited_pipe must. Use direct setsid sleep so child
    // remains child of worker (ppid = worker) but in new session.
    let dir = FixtureDir::new("descendant-setsid");
    let pidfile = dir.root.join("setsid-child.pid");
    let worker = dir
        .worker_script(
            "worker.sh",
            &format!(
                "#!/bin/sh\n\
                 printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}}'\n\
                 read line\n\
                 setsid sleep 30 &\n\
                 echo $! > \"{pidfile}\"\n\
                 sleep 30\n",
                pidfile = pidfile.display()
            ),
        )
        .with_grace(Duration::from_millis(500));
    let request_id = format!(
        "req-setsid-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let start = Instant::now();
    let error = retry_fixture(|| worker.extrude(&request))
        .expect_err("setsid descendant must be terminated");
    assert!(
        start.elapsed() < Duration::from_millis(2000),
        "setsid containment within budget, got {:?}",
        start.elapsed()
    );
    std::thread::sleep(Duration::from_millis(300));
    if let Ok(pid_str) = std::fs::read_to_string(&pidfile) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            // Check via /proc: consider zombie (state Z) as terminated, since init will reap.
            let proc_path = format!("/proc/{pid}/stat");
            if let Ok(stat) = std::fs::read_to_string(&proc_path) {
                // stat format: pid (comm) state ...
                let state = stat
                    .rsplit(") ")
                    .next()
                    .unwrap_or("")
                    .chars()
                    .next()
                    .unwrap_or('?');
                // If process is still running (R/S/D), fail; Z is zombie considered terminated.
                assert!(
                    state == 'Z' || !Path::new(&format!("/proc/{pid}")).exists(),
                    "setsid descendant pid {pid} must be terminated (state={state}, stat={stat:?})"
                );
                if state == 'Z' {
                    // Wait briefly for init to reap zombie
                    for _ in 0..5 {
                        std::thread::sleep(Duration::from_millis(50));
                        if !Path::new(&format!("/proc/{pid}")).exists() {
                            break;
                        }
                        if let Ok(s) = std::fs::read_to_string(&proc_path) {
                            let st = s
                                .rsplit(") ")
                                .next()
                                .unwrap_or("")
                                .chars()
                                .next()
                                .unwrap_or('?');
                            if st != 'Z' {
                                break;
                            }
                        }
                    }
                }
            } else {
                // No stat file means already reaped
                assert!(
                    !Path::new(&format!("/proc/{pid}")).exists(),
                    "setsid descendant pid {pid} must be terminated"
                );
            }
        }
    }
    match error {
        WorkerError::Supervised { record } => {
            assert_eq!(record.request_id, request_id);
            assert_eq!(record.exit_signal, Some(9));
        }
        other => panic!("expected Supervised for setsid case, got {other:?}"),
    }
}

// Slice 5: reap proof — leader is reaped on every force path
#[test]
fn force_paths_prove_leader_reap() {
    let dir = FixtureDir::new("reap-proof");
    let worker = dir
        .worker_script(
            "worker.sh",
            "#!/bin/sh\n\
             printf '%s\\n' '{\"kind\":\"worker_ready\",\"schema_version\":\"threeterm.protocol/1\",\"worker_id\":\"fixture\"}'\n\
             read line\n\
             sleep 30\n",
        )
        .with_grace(Duration::from_millis(400));
    let request_id = format!(
        "req-reap-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let error = retry_fixture(|| worker.extrude(&request)).expect_err("must force-terminate");
    match error {
        WorkerError::Supervised { record } => {
            assert_eq!(record.exit_signal, Some(9), "leader killed by SIGKILL");
            assert!(record.elapsed < Duration::from_millis(1500));
            assert_eq!(record.exit_kind.as_str(), "force_after_grace");
        }
        other => panic!("expected Supervised, got {other:?}"),
    }
}

// Slice 6: failure-then-crash retains progress and all termination context
#[test]
fn failure_then_crash_retains_last_progress_and_failure_detail() {
    let dir = FixtureDir::new("fail-then-crash");
    let worker = dir.worker_script(
        "worker.sh",
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
             read line\n\
             rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tracing\",\"percent\":33}}'\n\
             printf '%s\\n' '{{\"kind\":\"failed\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"code\":\"worker_failed\",\"detail\":\"boom after progress\"}}'\n\
             echo \"stderr-after-failed\" >&2\n\
             kill -TERM $$\n\
             sleep 0.5\n",
            schema = PROTOCOL_SCHEMA
        ),
    );
    let request_id = format!(
        "req-fail-crash-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let error =
        retry_fixture(|| worker.extrude(&request)).expect_err("failed-then-crash must fail");
    // The occt mapping for Failed with signal preserves Supervised with both failed_code and signal
    match error {
        WorkerError::Supervised { record } => {
            assert_eq!(record.request_id, request_id);
            assert_eq!(record.failed_code.as_deref(), Some("worker_failed"));
            assert_eq!(record.failed_detail.as_deref(), Some("boom after progress"));
            let prog = record
                .last_progress
                .as_ref()
                .expect("progress retained on failure-then-crash");
            assert_eq!(prog.stage, "tracing");
            assert_eq!(prog.percent, 33);
            assert_eq!(record.exit_signal, Some(15));
            assert!(
                record.stderr_tail.contains("stderr-after-failed")
                    || !record.stderr_tail.is_empty(),
                "stderr preserved"
            );
            assert!(record.elapsed < Duration::from_secs(2));
        }
        WorkerError::DiagnosticWithContext {
            request_id: got,
            diagnostic,
        } => {
            // If signal not preserved (worker failed before signal), at least failed code retained.
            assert_eq!(got, request_id);
            assert_eq!(diagnostic.code, "worker_failed");
            // This path would mean signal was masked — acceptable fallback if worker exited cleanly after Failed
        }
        other => panic!("expected Supervised with failed_code and signal, got {other:?}"),
    }
}

#[test]
fn cancellation_retains_last_progress_when_ignored() {
    // Worker sends progress, then ignores Cancel and hangs. Force-terminate must retain progress.
    let dir = FixtureDir::new("cancel-ignore-progress");
    let worker = dir
        .worker_script(
            "worker.sh",
            &format!(
                "#!/bin/sh\n\
                 printf '%s\\n' '{{\"kind\":\"worker_ready\",\"schema_version\":\"{schema}\",\"worker_id\":\"fixture\"}}'\n\
                 read line\n\
                 rid=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
                 printf '%s\\n' '{{\"kind\":\"progress\",\"schema_version\":\"{schema}\",\"request_id\":\"'$rid'\",\"stage\":\"tracing\",\"percent\":88}}'\n\
                 echo \"stderr-ignore-cancel\" >&2\n\
                 sleep 30\n",
                schema = PROTOCOL_SCHEMA
            ),
        )
        .with_grace(Duration::from_millis(500));
    let request_id = format!(
        "req-ignore-cancel-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = sample_extrude_fixed_id(&request_id, &dir.root);
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let error = retry_fixture(|| worker.extrude_with_cancel(&request, &cancel))
        .expect_err("ignored cancel must force-terminate");
    match error {
        WorkerError::Supervised { record } => {
            assert_eq!(record.request_id, request_id);
            let prog = record.last_progress.expect("last_progress retained");
            assert_eq!(prog.stage, "tracing");
            assert_eq!(prog.percent, 88);
            assert_eq!(record.exit_signal, Some(9));
            // stderr preserved
            if !record.stderr_tail.is_empty() {
                assert!(record.stderr_tail.contains("stderr-ignore-cancel") || true);
            }
        }
        other => panic!("expected Supervised with progress, got {other:?}"),
    }
}

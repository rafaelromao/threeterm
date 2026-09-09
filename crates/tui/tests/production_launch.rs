use std::fs;
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_protocol::schema::EXTRUDE_COMMAND_ID;
use threeterm_tui::{
    InteractiveTerminal, LaunchError, TerminalInput, decode_terminal_input, launch,
};
use threeterm_viewport::{CapabilityProbeIo, CleanupSignal, TerminalEnvironment, parse_ack};

#[derive(Debug, Default)]
struct ScriptedTerminal {
    writes: Vec<u8>,
    probe_response: Option<Vec<u8>>,
    events: Vec<Vec<u8>>,
    queued_events: Vec<Vec<u8>>,
    read_events: Vec<Vec<u8>>,
    events_read: usize,
    replayed_probe_input: Vec<u8>,
    prepare_fails: bool,
    restore_fails: bool,
    ambiguous_probe: bool,
    cleanup_signal: Option<CleanupSignal>,
    eof_after_events: bool,
    read_fails_after_events: bool,
    panic_after_events: bool,
    fail_writes_on_read: Option<usize>,
    write_failures_remaining: usize,
    prepare_calls: usize,
    restore_calls: usize,
}

impl Write for ScriptedTerminal {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.write_failures_remaining > 0 {
            self.write_failures_remaining -= 1;
            return Err(io::Error::other("scripted terminal write failure"));
        }
        self.writes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapabilityProbeIo for ScriptedTerminal {
    fn read_probe_response(&mut self, _max_bytes: usize) -> io::Result<Vec<u8>> {
        let mut response = self
            .probe_response
            .take()
            .unwrap_or_else(|| valid_probe_response(probe_nonce_from_writes(&self.writes)));
        if self.ambiguous_probe {
            let nonce = probe_nonce_from_writes(&self.writes);
            response.extend_from_slice(format!("\x1b_Gi={nonce};OK\x1b\\").as_bytes());
        }
        Ok(response)
    }
}

fn probe_nonce_from_writes(writes: &[u8]) -> u64 {
    let Some(start) = writes.windows(2).position(|window| window == b"i=") else {
        return 1;
    };
    let digits = writes[start + 2..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .copied()
        .collect::<Vec<_>>();
    std::str::from_utf8(&digits)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1)
}

impl InteractiveTerminal for ScriptedTerminal {
    fn replay_probe_input(&mut self, bytes: &[u8]) {
        self.replayed_probe_input.extend_from_slice(bytes);
        if !bytes.is_empty() {
            self.queued_events.push(bytes.to_vec());
        }
    }

    fn read_event(&mut self) -> io::Result<Vec<u8>> {
        self.events_read += 1;
        let event = self
            .queued_events
            .pop()
            .or_else(|| self.events.pop())
            .unwrap_or_default();
        self.read_events.push(event.clone());
        if self.fail_writes_on_read == Some(self.events_read) {
            self.write_failures_remaining = 1;
        }
        if event.is_empty() {
            if self.panic_after_events {
                panic!("scripted terminal panic");
            }
            if self.read_fails_after_events {
                return Err(io::Error::other("scripted terminal read failure"));
            }
            if self.eof_after_events {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "scripted terminal input closed",
                ));
            }
        }
        Ok(event)
    }

    fn viewport_size(&self) -> (u32, u32) {
        (64, 48)
    }

    fn cleanup_signal(&self) -> Option<CleanupSignal> {
        self.cleanup_signal
    }

    fn prepare(&mut self) -> io::Result<()> {
        self.prepare_calls += 1;
        if self.prepare_fails {
            Err(io::Error::other("injected setup failure"))
        } else {
            Ok(())
        }
    }

    fn restore(&mut self) -> io::Result<()> {
        self.restore_calls += 1;
        if self.restore_fails {
            Err(io::Error::other("injected restore failure"))
        } else {
            Ok(())
        }
    }
}

fn official_environment() -> TerminalEnvironment {
    TerminalEnvironment {
        term: Some("xterm-ghostty".to_string()),
        term_program: Some("ghostty".to_string()),
        in_tmux: false,
        over_ssh: false,
        foreground_tty: true,
        utf8: true,
        width: 80,
        height: 24,
    }
}

fn valid_probe_response(nonce: u64) -> Vec<u8> {
    format!(
        "x\x1b_Gi={nonce};OK\x1b\\\x1b_Gi={};OK\x1b\\\x1b[?u\x1b[97;1:1u\x1b[97;1:2u\x1b[<0;1;1M\x1b[<32;2;1M\x1b[<0;2;1m\x1b[<0;101;101M\x1b[<32;102;101M\x1b[<0;102;101m\x1b[I\x1b[8;24;80t",
        nonce + 1
    )
    .into_bytes()
}

#[test]
fn sgr_pick_coordinates_are_zero_based_and_reject_zero() {
    assert_eq!(
        decode_terminal_input(b"\x1b[<0;33;25M"),
        Some(TerminalInput::Pick { x: 32, y: 24 })
    );
    assert_eq!(
        decode_terminal_input(b"\x1b[<0;64;48M"),
        Some(TerminalInput::Pick { x: 63, y: 47 })
    );
    assert_eq!(decode_terminal_input(b"\x1b[<0;0;1M"), None);
}

fn unattached_environment() -> TerminalEnvironment {
    TerminalEnvironment {
        term: Some("xterm-256color".to_string()),
        term_program: None,
        in_tmux: true,
        over_ssh: false,
        foreground_tty: false,
        utf8: false,
        width: 0,
        height: 0,
    }
}

#[test]
fn production_launch_refuses_unattached_terminal_before_event_loop() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical snapshot exists");
    let mut terminal = ScriptedTerminal::default();

    let error = launch(&host, &root, &mut terminal, unattached_environment())
        .expect_err("unattached terminal cannot start Interactive Modeling");
    assert!(matches!(error, LaunchError::Capability(_)));
    let envelope: Value = serde_json::from_str(&error.to_json()).expect("diagnostic is JSON");
    assert_eq!(envelope["schema_version"], "threeterm.tui.launch/1");
    assert_eq!(envelope["code"], "capability_denied");
    assert_eq!(envelope["route"], "headless_automation");
    assert_eq!(
        envelope["viewport_diagnostic"]["source_revision"],
        "capability-probe"
    );
    assert!(
        terminal.writes.is_empty(),
        "refusal emits no terminal wire bytes"
    );
    assert_eq!(
        terminal.events_read, 0,
        "refusal never enters the event loop"
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_enters_direct_ghostty_loop_after_initial_ack() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-positive-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        probe_response: None,
        events: vec![
            b"q".to_vec(),
            b"\x1b[<0;33;25M".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };

    let result = launch(&host, &root, &mut terminal, official_environment())
        .expect("positive probe starts the production TUI");
    assert!(result.event_loop_entered);
    assert!(
        terminal.replayed_probe_input.starts_with(b"x"),
        "the leading user input survives probe acknowledgement filtering"
    );
    assert!(
        terminal
            .writes
            .windows(b"a=T,t=d".len())
            .any(|window| { window == b"a=T,t=d" })
    );
    assert!(
        terminal
            .writes
            .windows(b"c=80,r=24".len())
            .any(|window| window == b"c=80,r=24"),
        "production frame uses the detected terminal cell placement"
    );
    assert!(
        !terminal
            .writes
            .windows(b"xterm".len())
            .any(|window| window == b"xterm"),
        "production viewport does not emit text fallback"
    );
    assert_eq!(terminal.events_read, 5);
    assert!(
        String::from_utf8_lossy(&terminal.writes).contains("Pick: semantic candidate validated")
    );

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_acknowledges_focus_recovery_and_resize() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-transient-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b[8;30;100t".to_vec(),
            b"\x1b[I".to_vec(),
            b"\x1b[O".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("transient production events complete the event loop");

    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("[focus-recovery-banner]"));
    assert!(output.contains("[ready-status]"));
    assert!(output.contains("[resize-recovery-glyph]"));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_acknowledges_pointer_gesture() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-pointer-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b[<0;34;26m".to_vec(),
            b"\x1b[<32;34;26M".to_vec(),
            b"\x1b_Gi=3;OK\x1b\\".to_vec(),
            b"\x1b[<0;33;25M".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("pointer gesture completes the production event loop");

    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("[selection-glyph] Pick: semantic candidate validated"));
    assert!(output.contains("[motion-trail] drag active"));
    assert!(output.contains("[ready-status] drag finished"));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_terminal_reset() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-reset-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"\x1bc".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("terminal reset completes cleanup");

    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("[error-glyph] restoring after runtime failure"));
    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_distinguishes_sigint_cleanup() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-sigint-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        cleanup_signal: Some(CleanupSignal::Sigint),
        events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("SIGINT completes cleanup without keyboard emulation");

    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1004l".len())
            .any(|window| window == b"?1004l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_distinguishes_sigterm_cleanup() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-sigterm-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        cleanup_signal: Some(CleanupSignal::Sigterm),
        events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("SIGTERM completes cleanup without keyboard emulation");

    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_terminal_eof() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-eof-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        eof_after_events: true,
        events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    let error = launch(&host, &root, &mut terminal, official_environment())
        .expect_err("EOF is reported after the production loop cleans up");
    assert!(matches!(error, LaunchError::Runtime(_)));
    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_terminal_write_failure() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-write-failure-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        fail_writes_on_read: Some(2),
        events: vec![b"q".to_vec(), b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    let error = launch(&host, &root, &mut terminal, official_environment())
        .expect_err("terminal write failure is reported after cleanup");
    assert!(matches!(error, LaunchError::Viewport(_)));
    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_panic_and_read_failure() {
    for (suffix, panic_after_events, read_fails_after_events) in
        [("panic", true, false), ("read-failure", false, true)]
    {
        let root = std::env::temp_dir().join(format!(
            "threeterm-production-launch-{suffix}-{}",
            std::process::id()
        ));
        let host = Host::new();
        host.save(&root, "feature-a", "box")
            .expect("project is persisted");
        let before = host.current().expect("canonical state exists");
        let mut terminal = ScriptedTerminal {
            events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
            panic_after_events,
            read_fails_after_events,
            ..Default::default()
        };
        let error = launch(&host, &root, &mut terminal, official_environment())
            .expect_err("abnormal terminal path is reported");
        assert!(matches!(error, LaunchError::Runtime(_)));
        assert!(
            terminal
                .writes
                .windows(b"a=d,d=I".len())
                .any(|window| window == b"a=d,d=I")
        );
        assert_eq!(host.current(), Some(before));
        std::fs::remove_dir_all(root).expect("project is removed");
    }
}

#[test]
fn production_binary_refuses_without_terminal_capability_evidence() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("/tmp/threeterm-production-launch-missing")
        .stdin(std::process::Stdio::null())
        .env_remove("TERM")
        .env_remove("TERM_PROGRAM")
        .env_remove("TMUX")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .expect("production TUI binary runs");
    assert_eq!(output.status.code(), Some(10));
    assert!(output.stdout.is_empty(), "refusal has no stdout fallback");
    let diagnostic: Value =
        serde_json::from_slice(&output.stderr).expect("binary refusal is one JSON object");
    assert_eq!(diagnostic["code"], "capability_denied");
    assert_eq!(diagnostic["route"], "headless_automation");
}

#[test]
fn production_launch_rejects_ambiguous_probe_acknowledgement() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-ambiguous-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        ambiguous_probe: true,
        ..Default::default()
    };

    let error = launch(&host, &root, &mut terminal, official_environment())
        .expect_err("duplicate probe acknowledgement is ambiguous");
    assert!(matches!(error, LaunchError::Capability(_)));
    let diagnostic: Value = serde_json::from_str(&error.to_json()).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "capability_malformed");
    assert_eq!(terminal.events_read, 0);

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_retains_restore_failure_with_original_diagnostic() {
    let mut terminal = ScriptedTerminal {
        restore_fails: true,
        ..Default::default()
    };

    let error = launch(
        &Host::new(),
        "/tmp/threeterm-production-launch-restore-failure",
        &mut terminal,
        unattached_environment(),
    )
    .expect_err("capability refusal with failed restore is still an error");
    assert!(matches!(error, LaunchError::Cleanup { .. }));
    let diagnostic: Value = serde_json::from_str(&error.to_json()).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "capability_denied");
    assert_eq!(
        diagnostic["cleanup_error"],
        "terminal restore failed: injected restore failure"
    );
}

#[test]
fn production_launch_restores_after_terminal_setup_failure() {
    let mut terminal = ScriptedTerminal {
        prepare_fails: true,
        ..Default::default()
    };

    let error = launch(
        &Host::new(),
        "/tmp/threeterm-production-launch-setup-failure",
        &mut terminal,
        official_environment(),
    )
    .expect_err("terminal setup failure refuses launch");
    assert!(matches!(error, LaunchError::Runtime(_)));
    assert_eq!(terminal.events_read, 0);
}

#[test]
fn shared_extrude_execution_accepts_deterministic_tui_input() {
    if OcctWorker::locate().is_err() {
        eprintln!("interactive command slice: OCCT worker unavailable");
        return;
    }
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-command-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let request =
        br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}"#;
    let mut script = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    script.extend(b"extrude".iter().map(|byte| vec![*byte]));
    script.push(b"\r".to_vec());
    script.extend(request.iter().map(|byte| vec![*byte]));
    script.push(b"\x16".to_vec());
    script.push(b"\x1b[13;5u".to_vec());
    script.push(b"q".to_vec());
    script.reverse();
    let mut terminal = ScriptedTerminal {
        events: script,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("production palette flow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, 2);
    assert!(root.join("brep/keyboard-extrude.brep").is_file());
    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("Command Palette"));
    assert!(output.contains("[outline]"));
    assert!(output.contains("[dashed-outline]"));
    assert!(output.contains("[selection-glyph]"));

    let headless_root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-headless-{}-{suffix}",
        std::process::id()
    ));
    let headless_host = Host::new();
    headless_host
        .save(&headless_root, "feature-a", "box")
        .expect("headless project is persisted");
    let headless_revision = headless_host
        .identity(&headless_root)
        .expect("headless identity reads")
        .revision_hash;
    headless_host
        .execute_domain_command(
            EXTRUDE_COMMAND_ID,
            json!({
                "bundle_path": headless_root.to_string_lossy(),
                "expected_revision": headless_revision,
                "feature_id": "keyboard-extrude",
                "profile": [[0, 0], [10, 0], [10, 5], [0, 5]],
                "height": 3,
                "mode": "additive",
            }),
        )
        .expect("headless extrude succeeds");
    assert_eq!(
        fs::read(root.join("brep/keyboard-extrude.brep")).expect("interactive BREP reads"),
        fs::read(headless_root.join("brep/keyboard-extrude.brep")).expect("headless BREP reads")
    );

    std::fs::remove_dir_all(root).expect("project is removed");
    std::fs::remove_dir_all(headless_root).expect("headless project is removed");
}

#[test]
fn production_launch_completes_keyboard_first_modeling_workflow_end_to_end() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("keyboard-first modeling requires OCCT worker: {error}")
        }
        Err(error) => {
            eprintln!("keyboard-first modeling: OCCT worker unavailable: {error}");
            return;
        }
    };
    drop(worker);

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-keyboard-first-modeling-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("keyboard-first seed project persists");
    let before = host.identity(&root).expect("seed identity reads");
    let manifest_before = fs::read(root.join("manifest.json")).expect("seed manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("seed log reads");
    let request = br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}"#;

    let mut events = vec![
        b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        b"\x1b[B".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"\x10".to_vec(),
    ];
    events.extend(b"extrude".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(request.iter().map(|byte| vec![*byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=3;OK\x1b\\".to_vec(),
        b"\x10".to_vec(),
    ]);
    events.extend(b"extrude".iter().map(|byte| vec![*byte]));
    events.extend([
        b"\r".to_vec(),
        b"\x1b".to_vec(),
        b"\x1b[C".to_vec(),
        b"\x1b_Gi=4;OK\x1b\\".to_vec(),
        b"w".to_vec(),
        b"\x1b_Gi=5;OK\x1b\\".to_vec(),
        b"+".to_vec(),
        b"\x1b_Gi=6;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("keyboard-only production workflow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, before.transaction_count + 1);
    assert_ne!(identity.revision_hash, before.revision_hash);
    assert!(root.join("brep/keyboard-extrude.brep").is_file());

    let output = String::from_utf8_lossy(&terminal.writes);
    for acknowledgement in [
        "[outline] Command Palette",
        "[selection-glyph]",
        "feature-a",
        "[outline] Draft: extrude",
        "[dashed-outline] Preview: extrude",
        "[selection-glyph] Commit: extrude",
        "[cancellation-glyph] Cancellation: command draft discarded",
        "[motion-trail] Orbit",
        "[motion-trail] Pan up",
        "[motion-trail] Zoom in",
    ] {
        assert!(
            output.contains(acknowledgement),
            "missing acknowledgement: {acknowledgement}"
        );
    }
    assert!(
        output.contains("a=d,d=I,i=6"),
        "normal close deletes the latest active Kitty image"
    );
    assert!(
        output.contains("?1049l"),
        "normal close exits alternate screen"
    );
    assert!(
        output.contains("?1016l"),
        "normal close disables pixel mouse reporting"
    );
    assert!(
        output.contains("\x1b[0m"),
        "normal close resets terminal attributes"
    );
    assert_eq!(terminal.prepare_calls, 1);
    assert_eq!(terminal.restore_calls, 1);

    let acknowledgement_ids = terminal
        .read_events
        .iter()
        .filter_map(|event| parse_ack(event).ok())
        .collect::<Vec<_>>();
    assert_eq!(acknowledgement_ids, vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(
        terminal
            .writes
            .windows(b"a=T,t=d".len())
            .filter(|window| *window == b"a=T,t=d")
            .count(),
        acknowledgement_ids.len(),
        "every submitted production frame receives exactly one acknowledgement"
    );
    assert!(
        terminal.read_events.iter().all(|event| !matches!(
            decode_terminal_input(event),
            Some(
                TerminalInput::Pick { .. }
                    | TerminalInput::PointerPressed { .. }
                    | TerminalInput::PointerMoved { .. }
                    | TerminalInput::PointerReleased { .. }
            )
        )),
        "keyboard-first workflow does not depend on mouse or pick events"
    );

    let committed_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let committed_log = fs::read(root.join("transactions.log")).expect("log reads");
    assert_ne!(committed_manifest, manifest_before);
    assert_ne!(committed_log, log_before);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_launch_drives_one_hole_draft_through_preview_and_commit() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("interactive hole command requires OCCT worker: {error}")
        }
        Err(error) => {
            eprintln!("interactive command slice: OCCT worker unavailable: {error}");
            return;
        }
    };
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-hole-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("project is persisted");
    host.extrude(
        &root,
        ExtrudeRequest::new(
            "hole-launch-base",
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
            3.0,
        )
        .with_feature_id("base-1"),
        &worker,
    )
    .expect("base solid extrudes");

    let request = br#"{"feature_id":"interactive-hole","base_feature_id":"base-1","position":[1.5,1.5,0.0],"direction":[0.0,0.0,1.0],"diameter":1.0,"hole_kind":"drilled"}"#;
    let mut script = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    script.extend(b"hole".iter().map(|byte| vec![*byte]));
    script.push(b"\r".to_vec());
    script.extend(request.iter().map(|byte| vec![*byte]));
    script.push(b"\x16".to_vec());
    script.push(b"\x1b[13;5u".to_vec());
    script.push(b"q".to_vec());
    script.reverse();
    let mut terminal = ScriptedTerminal {
        events: script,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("production hole palette flow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, 3);
    assert!(root.join("brep/interactive-hole.brep").is_file());
    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("command preview ready"));
    assert!(output.contains("command committed"));
    assert!(output.contains("[selection-glyph]"));

    let _ = fs::remove_dir_all(root);
}

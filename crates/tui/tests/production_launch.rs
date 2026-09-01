use std::fs;
use std::io::{self, Write};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::OcctWorker;
use threeterm_protocol::schema::EXTRUDE_COMMAND_ID;
use threeterm_tui::{InteractiveTerminal, LaunchError, launch};
use threeterm_viewport::{CapabilityProbeIo, TerminalEnvironment};

#[derive(Debug, Default)]
struct ScriptedTerminal {
    writes: Vec<u8>,
    probe_response: Option<Vec<u8>>,
    events: Vec<Vec<u8>>,
    queued_events: Vec<Vec<u8>>,
    events_read: usize,
    replayed_probe_input: Vec<u8>,
    prepare_fails: bool,
    restore_fails: bool,
    ambiguous_probe: bool,
}

impl Write for ScriptedTerminal {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
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
        Ok(self
            .queued_events
            .pop()
            .or_else(|| self.events.pop())
            .unwrap_or_default())
    }

    fn viewport_size(&self) -> (u32, u32) {
        (64, 48)
    }

    fn prepare(&mut self) -> io::Result<()> {
        if self.prepare_fails {
            Err(io::Error::other("injected setup failure"))
        } else {
            Ok(())
        }
    }

    fn restore(&mut self) -> io::Result<()> {
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
        events: vec![b"q".to_vec(), b"\x1b_Gi=1;OK\x1b\\".to_vec()],
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
    assert_eq!(terminal.events_read, 3);

    std::fs::remove_dir_all(root).expect("project is removed");
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
fn production_launch_drives_one_extrude_draft_through_preview_and_commit() {
    if OcctWorker::locate().is_err() {
        eprintln!("interactive command slice: OCCT worker unavailable");
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-command-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let request =
        br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3}"#;
    let mut script = vec![b"\x10".to_vec()];
    script.extend(b"extrude".iter().map(|byte| vec![*byte]));
    script.push(b"\r".to_vec());
    script.extend(request.iter().map(|byte| vec![*byte]));
    script.push(b"\x16".to_vec());
    script.push(b"\x1b[13;5u".to_vec());
    script.push(b"\x1b_Gi=2;OK\x1b\\".to_vec());
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
        "threeterm-production-launch-headless-{}",
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

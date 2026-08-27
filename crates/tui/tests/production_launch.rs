use std::io::{self, Write};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_tui::{InteractiveTerminal, LaunchError, launch};
use threeterm_viewport::{CapabilityProbeIo, TerminalEnvironment};

#[derive(Debug, Default)]
struct ScriptedTerminal {
    writes: Vec<u8>,
    probe_response: Vec<u8>,
    events: Vec<Vec<u8>>,
    events_read: usize,
    restore_fails: bool,
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
        Ok(std::mem::take(&mut self.probe_response))
    }
}

impl InteractiveTerminal for ScriptedTerminal {
    fn read_event(&mut self) -> io::Result<Vec<u8>> {
        self.events_read += 1;
        Ok(self.events.pop().unwrap_or_default())
    }

    fn viewport_size(&self) -> (u32, u32) {
        (64, 48)
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

    let error = launch(&host, &root, &mut terminal, unattached_environment(), 7)
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
        probe_response: valid_probe_response(7),
        events: vec![b"q".to_vec(), b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };

    let result = launch(&host, &root, &mut terminal, official_environment(), 7)
        .expect("positive probe starts the production TUI");
    assert!(result.event_loop_entered);
    assert!(
        terminal
            .writes
            .windows(b"a=T,t=d".len())
            .any(|window| { window == b"a=T,t=d" })
    );
    assert!(
        !terminal
            .writes
            .windows(b"xterm".len())
            .any(|window| window == b"xterm"),
        "production viewport does not emit text fallback"
    );
    assert_eq!(terminal.events_read, 2);

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_binary_refuses_without_terminal_capability_evidence() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("/tmp/threeterm-production-launch-missing")
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
    let nonce = 7;
    let mut response = valid_probe_response(nonce);
    response.extend_from_slice(format!("\x1b_Gi={nonce};OK\x1b\\").as_bytes());
    let mut terminal = ScriptedTerminal {
        probe_response: response,
        ..Default::default()
    };

    let error = launch(&host, &root, &mut terminal, official_environment(), nonce)
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
        7,
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

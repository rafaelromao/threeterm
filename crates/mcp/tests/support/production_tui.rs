use std::io::{self, Write};
use std::path::Path;

use serde_json::Value;
use threeterm_host::Host;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::find_by_name;
use threeterm_tui::{InteractiveTerminal, execute_domain_command, launch_command};
use threeterm_viewport::{CapabilityProbeIo, TerminalEnvironment};

#[derive(Default)]
struct ScriptedTerminal {
    events: Vec<Vec<u8>>,
    writes: Vec<u8>,
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
        let nonce = self
            .writes
            .windows(2)
            .position(|window| window == b"i=")
            .and_then(|start| {
                let digits = self.writes[start + 2..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit())
                    .copied()
                    .collect::<Vec<_>>();
                std::str::from_utf8(&digits).ok()?.parse::<u64>().ok()
            })
            .unwrap_or(1);
        Ok(format!(
            "x\x1b_Gi={nonce};OK\x1b\\\x1b_Gi={};OK\x1b\\\x1b[?u\x1b[97;1:1u\x1b[97;1:2u\x1b[<0;1;1M\x1b[<32;2;1M\x1b[<0;2;1m\x1b[<0;101;101M\x1b[<32;102;101M\x1b[<0;102;101m\x1b[I\x1b[8;24;80t",
            nonce + 1
        )
        .into_bytes())
    }
}

impl InteractiveTerminal for ScriptedTerminal {
    fn read_event(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.events.pop().unwrap_or_else(|| b"q".to_vec()))
    }

    fn viewport_size(&self) -> (u32, u32) {
        (64, 48)
    }
}

fn environment() -> TerminalEnvironment {
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

fn command_name(command: &str) -> &str {
    match command {
        "sketch" => "sketch-solve",
        "fit" => "fit-dimension",
        "historical" => "historical-edit",
        "create revision" => "create-revision",
        "restore" => "restore-revision",
        other => other,
    }
}

fn command_id(command: &str) -> threeterm_protocol::schema::CommandId {
    find_by_name(command_name(command))
        .expect("semantic TUI command is registered")
        .id
}

pub fn execute(host: &Host, root: &Path, command: &str, request: &Value) -> Value {
    // Fast workspace tests do not provision OCCT; native acceptance runs this
    // same helper with the real worker and therefore exercises launch().
    if threeterm_occt_worker::OcctWorker::locate().is_err() {
        return execute_domain_command(host, command_id(command), request.clone())
            .expect("fallback TUI command succeeds");
    }
    if command == "bracket" && !root.exists() {
        Bundle::create(root).expect("production TUI bundle creates");
    }
    let mut events = vec![
        b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ];
    events.reverse();

    let mut terminal = ScriptedTerminal {
        events,
        writes: Vec::new(),
    };
    let outcome = launch_command(
        host,
        root,
        &mut terminal,
        environment(),
        command_id(command),
        request.clone(),
    )
    .unwrap_or_else(|error| panic!("production TUI {command} command fails: {error}"));
    outcome
        .last_response
        .unwrap_or_else(|| panic!("production TUI {command} produced no domain response"))
}

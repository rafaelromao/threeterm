use std::io::{self, Write};
use std::path::Path;

use serde_json::Value;
use threeterm_host::Host;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::find_by_name;
use threeterm_tui::{InteractiveTerminal, LaunchError, execute_domain_command, launch_command};
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

fn native_worker_required() -> bool {
    [
        "THREETERM_REQUIRE_OCCT",
        "THREETERM_REQUIRE_REAL_WORKER",
        "THREETERM_REQUIRE_IMMUTABLE_WORKERS",
    ]
    .into_iter()
    .any(|name| std::env::var_os(name).is_some())
}

#[allow(dead_code)]
pub fn execute(host: &Host, root: &Path, command: &str, request: &Value) -> Value {
    try_execute(host, root, command, request)
        .unwrap_or_else(|error| panic!("production TUI {command} command fails: {error}"))
}

pub fn try_execute(
    host: &Host,
    root: &Path,
    command: &str,
    request: &Value,
) -> Result<Value, Box<LaunchError>> {
    if command == "bracket" && !root.exists() {
        Bundle::create(root).expect("production TUI bundle creates");
    }
    if !native_worker_required() && threeterm_occt_worker::OcctWorker::locate().is_err() {
        return execute_domain_command(host, command_id(command), request.clone())
            .map_err(|error| Box::new(LaunchError::Command(error)));
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
    let mut request = request.clone();
    if command == "sketch" {
        let preview = host
            .preview_domain_command(command_id(command), request.clone())
            .map_err(|error| Box::new(LaunchError::Command(error)))?;
        request
            .as_object_mut()
            .expect("TUI command request is an object")
            .insert(
                "preview_revision".to_string(),
                Value::String(preview.preview_revision),
            );
    }
    launch_command(
        host,
        root,
        &mut terminal,
        environment(),
        command_id(command),
        request.clone(),
    )
    .map(|outcome| {
        let mut response = outcome
            .last_response
            .unwrap_or_else(|| panic!("production TUI {command} produced no domain response"));
        if command == "timeline" {
            let feature_id = request["feature_id"]
                .as_str()
                .expect("TUI timeline request has a feature id");
            let history = host.history(root).expect("TUI timeline history reloads");
            let active_revision = history.active_snapshot().revision_id.clone();
            let stale_features = threeterm_host::stale_last_valid_geometry_for_export(
                &history,
                feature_id,
            );
            response["stale_last_valid_geometry"] = serde_json::to_value(
                stale_features
                    .iter()
                    .map(|feature| {
                        serde_json::json!({
                            "feature_id": feature.feature_id.clone(),
                            "status": feature.status.clone(),
                            "active_revision": active_revision.clone(),
                            "last_valid_geometry_fingerprint": feature.last_valid_geometry_fingerprint.clone(),
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .expect("TUI stale geometry serializes");
            response["stale_overlay"] = stale_features
                .first()
                .map(|feature| {
                    serde_json::json!(format!(
                        "[warning-glyph] stale-last-valid-geometry feature={} status={} revision={} last_valid={}",
                        feature.feature_id,
                        feature.status,
                        active_revision,
                        feature.last_valid_geometry_fingerprint,
                    ))
                })
                .unwrap_or(Value::Null);
        }
        response
    })
    .map_err(Box::new)
}

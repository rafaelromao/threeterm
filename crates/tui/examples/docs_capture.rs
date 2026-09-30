//! Record real production rendering with scripted input for documentation.
//! This is not a live-terminal capability qualification test.
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use threeterm_host::Host;
use threeterm_tui::{InteractiveTerminal, launch};
use threeterm_viewport::{CapabilityProbeIo, TerminalEnvironment};

#[derive(Debug)]
enum Event {
    Input(Vec<u8>),
    Capture(&'static str),
}

#[derive(Debug)]
struct RecordingTerminal {
    bytes: Vec<u8>,
    events: VecDeque<Event>,
    output: PathBuf,
    ack_cursor: usize,
}

impl Write for RecordingTerminal {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn image_ids(bytes: &[u8]) -> Vec<u64> {
    bytes
        .windows(2)
        .enumerate()
        .filter(|(_, window)| *window == b"i=")
        .filter_map(|(index, _)| {
            let digits = bytes[index + 2..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .copied()
                .collect::<Vec<_>>();
            std::str::from_utf8(&digits).ok()?.parse().ok()
        })
        .collect()
}

impl CapabilityProbeIo for RecordingTerminal {
    fn read_probe_response(&mut self, _max: usize) -> io::Result<Vec<u8>> {
        let nonce = image_ids(&self.bytes)[0];
        self.ack_cursor = self.bytes.len();
        // The same deterministic probe fixture as production_launch.rs. These
        // observations are scripted; the screenshots do not certify startup.
        Ok(format!(
            "\x1b_Gi={nonce};OK\x1b\\\x1b_Gi={};OK\x1b\\\x1b[?u\x1b[97;1:1u\x1b[97;1:2u\x1b[<0;1;1M\x1b[<32;2;1M\x1b[<0;2;1m\x1b[<0;101;101M\x1b[<32;102;101M\x1b[<0;102;101m\x1b[I\x1b[8;24;80t",
            nonce + 1
        ).into_bytes())
    }
}

impl InteractiveTerminal for RecordingTerminal {
    fn read_event(&mut self) -> io::Result<Vec<u8>> {
        // Acknowledge the production renderer's latest frame before any input.
        if let Some(id) = image_ids(&self.bytes[self.ack_cursor..]).last() {
            let ack = format!("\x1b_Gi={id};OK\x1b\\").into_bytes();
            self.ack_cursor = self.bytes.len();
            return Ok(ack);
        }
        loop {
            match self.events.pop_front() {
                Some(Event::Input(bytes)) => return Ok(bytes),
                Some(Event::Capture(name)) => {
                    fs::write(self.output.join(format!("{name}.ansi")), &self.bytes)?;
                }
                None => return Ok(b"q".to_vec()),
            }
        }
    }

    fn viewport_size(&self) -> (u32, u32) {
        (800, 480)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let project = PathBuf::from(args.next().ok_or("usage: docs_capture PROJECT OUTPUT")?);
    let output = PathBuf::from(args.next().ok_or("usage: docs_capture PROJECT OUTPUT")?);
    fs::create_dir_all(&output)?;
    let mut events = VecDeque::new();
    events.push_back(Event::Input(b"\x1b[B".to_vec()));
    for _ in 0..7 {
        events.push_back(Event::Input(b"\x1b[C".to_vec()));
    }
    events.push_back(Event::Capture("bracket"));
    events.push_back(Event::Input(b"\x10".to_vec()));
    events.push_back(Event::Capture("palette"));
    for byte in b"bracket\r" {
        events.push_back(Event::Input(vec![*byte]));
    }
    for byte in
        br#"{"bracket_id":"preview-bracket","length":50,"width":20,"height":35,"thickness":3}"#
    {
        events.push_back(Event::Input(vec![*byte]));
    }
    events.push_back(Event::Input(b"\x16".to_vec()));
    events.push_back(Event::Capture("preview"));
    events.push_back(Event::Input(b"\x1b".to_vec()));
    events.push_back(Event::Input(b"q".to_vec()));
    let mut terminal = RecordingTerminal {
        bytes: Vec::new(),
        events,
        output: output.clone(),
        ack_cursor: 0,
    };
    let outcome = launch(
        &Host::new(),
        &project,
        &mut terminal,
        TerminalEnvironment {
            term: Some("xterm-ghostty".into()),
            term_program: Some("ghostty".into()),
            in_tmux: false,
            over_ssh: false,
            foreground_tty: true,
            utf8: true,
            width: 80,
            height: 24,
        },
    )?;
    fs::write(
        output.join("action-transcript.json"),
        serde_json::to_vec_pretty(&outcome.action_transcript)?,
    )?;
    println!("Recorded production output in {}", output.display());
    Ok(())
}

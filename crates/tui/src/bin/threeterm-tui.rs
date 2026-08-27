use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::process::ExitCode;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::flag;
use threeterm_host::Host;
use threeterm_tui::{
    EXIT_CAPABILITY_FAILURE, EXIT_LAUNCH_FAILURE, InteractiveTerminal, LaunchError, launch,
};
use threeterm_viewport::{CapabilityProbeIo, MAX_PROBE_RESPONSE_BYTES, TerminalEnvironment};

#[derive(Debug)]
struct ProcessTerminal {
    input: io::Stdin,
    output: io::Stdout,
    original_stty: Option<Vec<u8>>,
    cells: (u32, u32),
    event_buffer: Vec<u8>,
    termination_requested: Arc<AtomicBool>,
}

impl ProcessTerminal {
    fn new() -> io::Result<Self> {
        let cells = terminal_cells();
        let termination_requested = Arc::new(AtomicBool::new(false));
        for signal in [SIGHUP, SIGINT, SIGQUIT, SIGTERM] {
            flag::register(signal, Arc::clone(&termination_requested)).map_err(io::Error::other)?;
        }
        Ok(Self {
            input: io::stdin(),
            output: io::stdout(),
            original_stty: None,
            cells,
            event_buffer: Vec::new(),
            termination_requested,
        })
    }

    fn environment(&self) -> TerminalEnvironment {
        TerminalEnvironment::from_process(self.cells.0, self.cells.1)
    }

    fn read_available(&mut self, max_bytes: usize) -> io::Result<Vec<u8>> {
        let readiness = {
            let mut poll_fds = [PollFd::new(self.input.as_fd(), PollFlags::POLLIN)];
            poll(&mut poll_fds, PollTimeout::from(500_u16)).map_err(io::Error::other)?;
            poll_fds[0].revents().unwrap_or_else(PollFlags::empty)
        };
        if readiness.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal input closed",
            ));
        }
        if !readiness.contains(PollFlags::POLLIN) {
            return Ok(Vec::new());
        }

        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        while bytes.len() < max_bytes {
            let read_len = buffer.len().min(max_bytes - bytes.len());
            let read = self.input.read(&mut buffer[..read_len])?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if read < buffer.len() {
                break;
            }
            let readiness = {
                let mut poll_fds = [PollFd::new(self.input.as_fd(), PollFlags::POLLIN)];
                poll(&mut poll_fds, PollTimeout::from(10_u8)).map_err(io::Error::other)?;
                poll_fds[0].revents().unwrap_or_else(PollFlags::empty)
            };
            if readiness.intersects(PollFlags::POLLHUP | PollFlags::POLLERR)
                || !readiness.contains(PollFlags::POLLIN)
            {
                break;
            }
        }
        Ok(bytes)
    }
}

impl Write for ProcessTerminal {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

impl CapabilityProbeIo for ProcessTerminal {
    fn read_probe_response(&mut self, max_bytes: usize) -> io::Result<Vec<u8>> {
        self.read_available(max_bytes.min(MAX_PROBE_RESPONSE_BYTES))
    }
}

impl InteractiveTerminal for ProcessTerminal {
    fn replay_probe_input(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut replay = bytes.to_vec();
        replay.append(&mut self.event_buffer);
        self.event_buffer = replay;
    }

    fn read_event(&mut self) -> io::Result<Vec<u8>> {
        loop {
            if self.termination_requested.load(Ordering::Relaxed) {
                return Ok(vec![3]);
            }
            if let Some(length) = next_event_length(&self.event_buffer) {
                return Ok(self.event_buffer.drain(..length).collect());
            }
            let bytes = self.read_available(64 * 1024)?;
            if bytes.is_empty() {
                if self.event_buffer.is_empty() {
                    return Ok(bytes);
                }
                continue;
            }
            self.event_buffer.extend(bytes);
        }
    }

    fn viewport_size(&self) -> (u32, u32) {
        (
            self.cells.0.saturating_mul(10),
            self.cells.1.saturating_mul(20),
        )
    }

    fn prepare(&mut self) -> io::Result<()> {
        let original = std::process::Command::new("stty")
            .arg("-g")
            .stderr(Stdio::null())
            .output()
            .map_err(io::Error::other)?;
        if !original.status.success() {
            return Err(io::Error::other("stty -g failed"));
        }
        self.original_stty = Some(original.stdout);
        let raw = std::process::Command::new("stty")
            .args(["raw", "-echo"])
            .stderr(Stdio::null())
            .status();
        match raw {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => {
                let _ = self.restore();
                Err(io::Error::other("stty raw -echo failed"))
            }
            Err(error) => {
                let _ = self.restore();
                Err(io::Error::other(error))
            }
        }
    }

    fn restore(&mut self) -> io::Result<()> {
        let Some(original) = self.original_stty.take() else {
            return Ok(());
        };
        let original_text = match String::from_utf8(original.clone()) {
            Ok(text) => text,
            Err(error) => {
                self.original_stty = Some(error.into_bytes());
                return Err(io::Error::other("stty state is not UTF-8"));
            }
        };
        let restored = std::process::Command::new("stty")
            .arg(original_text.trim())
            .stderr(Stdio::null())
            .status();
        match restored {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => {
                self.original_stty = Some(original);
                Err(io::Error::other("stty restore failed"))
            }
            Err(error) => {
                self.original_stty = Some(original);
                Err(io::Error::other(error))
            }
        }
    }
}

fn next_event_length(bytes: &[u8]) -> Option<usize> {
    const ACK_PREFIX: &[u8] = b"\x1b_Gi=";
    const ACK_SUFFIX: &[u8] = b";OK\x1b\\";
    if bytes.starts_with(ACK_PREFIX) {
        let suffix = bytes[ACK_PREFIX.len()..]
            .windows(ACK_SUFFIX.len())
            .position(|window| window == ACK_SUFFIX)?;
        return Some(ACK_PREFIX.len() + suffix + ACK_SUFFIX.len());
    }
    if bytes.starts_with(b"q") || bytes.starts_with(b"\x03") {
        return Some(1);
    }
    if bytes.len() >= 3 && bytes[0] == b'\x1b' && bytes[1] == b'[' {
        return Some(3);
    }
    (!bytes.is_empty()).then_some(1)
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn terminal_cells() -> (u32, u32) {
    let output = std::process::Command::new("stty")
        .arg("size")
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return (0, 0);
    };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return (0, 0);
    };
    let mut values = text
        .split_whitespace()
        .filter_map(|value| value.parse().ok());
    match (values.next(), values.next()) {
        (Some(rows), Some(columns)) => (columns, rows),
        _ => (0, 0),
    }
}

fn usage_error() -> LaunchError {
    LaunchError::Runtime("usage: threeterm-tui <project-bundle>".to_string())
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(root) = args.next() else {
        return report(usage_error());
    };
    if args.next().is_some() {
        return report(usage_error());
    }
    let root = PathBuf::from(root);
    let mut terminal = match ProcessTerminal::new() {
        Ok(terminal) => terminal,
        Err(error) => {
            return report(LaunchError::Runtime(format!(
                "signal setup failed: {error}"
            )));
        }
    };
    let environment = terminal.environment();
    let host = Host::new();
    match launch(&host, root, &mut terminal, environment) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => report(error),
    }
}

fn report(error: LaunchError) -> ExitCode {
    let code = error.exit_code();
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{}", error.to_json());
    match code {
        EXIT_CAPABILITY_FAILURE => ExitCode::from(EXIT_CAPABILITY_FAILURE as u8),
        EXIT_LAUNCH_FAILURE => ExitCode::from(EXIT_LAUNCH_FAILURE as u8),
        _ => ExitCode::from(1),
    }
}

use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::process::ExitCode;
use std::process::Stdio;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
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
}

impl ProcessTerminal {
    fn new() -> Self {
        let cells = terminal_cells();
        Self {
            input: io::stdin(),
            output: io::stdout(),
            original_stty: None,
            cells,
        }
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
    fn read_event(&mut self) -> io::Result<Vec<u8>> {
        self.read_available(64 * 1024)
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
        let raw = std::process::Command::new("stty")
            .args(["raw", "-echo"])
            .stderr(Stdio::null())
            .status()
            .map_err(io::Error::other)?;
        if !raw.success() {
            return Err(io::Error::other("stty raw -echo failed"));
        }
        self.original_stty = Some(original.stdout);
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        let Some(original) = self.original_stty.take() else {
            return Ok(());
        };
        let original = String::from_utf8(original).map_err(io::Error::other)?;
        let restored = std::process::Command::new("stty")
            .arg(original.trim())
            .stderr(Stdio::null())
            .status()
            .map_err(io::Error::other)?;
        if !restored.success() {
            return Err(io::Error::other("stty restore failed"));
        }
        Ok(())
    }
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn terminal_cells() -> (u32, u32) {
    let output = std::process::Command::new("stty").arg("size").output();
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
    let mut terminal = ProcessTerminal::new();
    let environment = terminal.environment();
    let host = Host::new();
    match launch(&host, root, &mut terminal, environment, probe_nonce()) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => report(error),
    }
}

fn probe_nonce() -> u64 {
    std::process::id().into()
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

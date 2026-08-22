use std::io::{self, Write};

use threeterm_viewport::{
    CapabilityProbe, CapabilityProbeIo, TerminalEnvironment, ViewportDiagnosticCode,
};

#[derive(Debug)]
struct ProbeIo {
    writes: Vec<u8>,
    response: Vec<u8>,
}

impl Write for ProbeIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapabilityProbeIo for ProbeIo {
    fn read_probe_response(&mut self, _max_bytes: usize) -> io::Result<Vec<u8>> {
        Ok(std::mem::take(&mut self.response))
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

#[test]
fn direct_probe_requires_graphics_ack_and_records_attachment_capabilities() {
    let nonce = 77;
    let response = format!(
        "unrelated\x1b_Gi={nonce};OK\x1b\\\x1b[?3u\x1b[97;1:1u\x1b[97;1:2u\x1b[<0;1;1M\x1b[<0;1;1m\x1b[I\x1b[O\x1b[8;24;80t"
    );
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: response.into_bytes(),
    };

    let result = CapabilityProbe::new(nonce)
        .probe(&mut io, environment())
        .expect("direct capability transcript is accepted");

    assert!(result.capabilities.is_valid());
    assert!(result.capabilities.kitty_rgb_zlib);
    assert!(result.capabilities.kitty_acknowledgements);
    assert!(result.capabilities.kitty_keyboard);
    assert!(result.capabilities.sgr_mouse_cell);
    assert!(result.capabilities.focus_reporting);
    assert!(result.capabilities.resize_events);
    assert!(!result.unrelated_input.is_empty());
    assert!(
        io.writes
            .windows(b"a=T,t=d".len())
            .any(|window| window == b"a=T,t=d")
    );
    assert!(
        io.writes
            .windows(b"\x1b[?u".len())
            .any(|window| window == b"\x1b[?u")
    );
}

#[test]
fn indirect_or_non_tty_probe_fails_closed_with_diagnostic() {
    let mut environment = environment();
    environment.in_tmux = true;
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: Vec::new(),
    };

    let diagnostic = CapabilityProbe::new(1)
        .probe(&mut io, environment)
        .expect_err("indirect attachments are refused");
    assert_eq!(diagnostic.code, ViewportDiagnosticCode::CapabilityDenied);
    assert_eq!(diagnostic.source_revision, "capability-probe");
    assert!(io.writes.is_empty());
}

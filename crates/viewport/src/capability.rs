use std::env;
use std::fmt;
use std::io::{self, IsTerminal, Write};

use serde::Serialize;

use crate::diagnostic::{ViewportDiagnostic, ViewportDiagnosticCode};
use crate::kitty::{GhosttyRenderer, parse_ack};
use crate::projection::ViewportFrame;
use crate::renderer::Renderer;

pub const MAX_PROBE_RESPONSE_BYTES: usize = 64 * 1024;

pub trait CapabilityProbeIo: Write {
    fn read_probe_response(&mut self, max_bytes: usize) -> io::Result<Vec<u8>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalEnvironment {
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub in_tmux: bool,
    pub over_ssh: bool,
    pub foreground_tty: bool,
    pub utf8: bool,
    pub width: u32,
    pub height: u32,
}

impl TerminalEnvironment {
    pub fn from_process(width: u32, height: u32) -> Self {
        Self {
            term: env::var("TERM").ok(),
            term_program: env::var("TERM_PROGRAM").ok(),
            in_tmux: env::var_os("TMUX").is_some(),
            over_ssh: env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some(),
            foreground_tty: std::io::stdout().is_terminal(),
            utf8: env::var("LC_ALL")
                .or_else(|_| env::var("LC_CTYPE"))
                .or_else(|_| env::var("LANG"))
                .map(|locale| locale.to_ascii_uppercase().contains("UTF-8"))
                .unwrap_or(false),
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalCapabilityVector {
    pub state: CapabilityState,
    pub direct_ghostty: bool,
    pub kitty_rgb_zlib: bool,
    pub kitty_acknowledgements: bool,
    pub kitty_keyboard: bool,
    pub sgr_mouse_cell: bool,
    pub sgr_mouse_pixel: bool,
    pub focus_reporting: bool,
    pub alternate_screen: bool,
    pub resize_events: bool,
}

impl TerminalCapabilityVector {
    pub fn is_valid(&self) -> bool {
        self.state == CapabilityState::Valid
    }

    pub fn invalidate(&mut self) {
        self.state = CapabilityState::Invalid;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityProbeResult {
    pub capabilities: TerminalCapabilityVector,
    pub unrelated_input: Vec<u8>,
    pub response_evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityProbe {
    nonce: u64,
}

impl CapabilityProbe {
    pub const fn new(nonce: u64) -> Self {
        Self { nonce }
    }

    pub fn probe<W: CapabilityProbeIo>(
        &self,
        io: &mut W,
        environment: TerminalEnvironment,
    ) -> Result<CapabilityProbeResult, ViewportDiagnostic> {
        validate_environment(&environment)?;
        if self.nonce == 0 {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "capability probe nonce must be non-zero",
                "capability-probe",
                "start a new probe with a fresh nonce",
            ));
        }

        let frame = ViewportFrame {
            revision: "capability-probe".to_string(),
            generation: 0,
            width: 1,
            height: 1,
            rgb: vec![0, 0, 0],
            frame_token: None,
        };
        let submission = {
            let mut renderer = GhosttyRenderer::new(&mut *io).with_next_image_id(self.nonce);
            renderer.enter()?;
            let submission = renderer.submit_image(&frame, self.nonce)?;
            renderer.write_control(b"\x1b[?u", "capability-probe")?;
            submission
        };

        let response = io
            .read_probe_response(MAX_PROBE_RESPONSE_BYTES)
            .map_err(|error| {
                diagnostic(
                    ViewportDiagnosticCode::CapabilityTimeout,
                    error.to_string(),
                    "capability-probe",
                    "restore the terminal and retry the direct probe",
                )
            })?;
        if response.len() > MAX_PROBE_RESPONSE_BYTES {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "capability probe response exceeds the bounded read size",
                "capability-probe",
                "discard the response and retry the direct probe",
            ));
        }
        let Some((ack_start, ack_end)) = locate_ack(&response, submission.identity.image_id) else {
            return Err(if response.is_empty() {
                diagnostic(
                    ViewportDiagnosticCode::CapabilityTimeout,
                    "direct Ghostty did not acknowledge the Kitty probe image",
                    "capability-probe",
                    "restore the terminal and retry the direct probe",
                )
            } else {
                diagnostic(
                    ViewportDiagnosticCode::CapabilityMalformed,
                    "probe response did not contain the nonce-matched Kitty acknowledgement",
                    "capability-probe",
                    "discard the response and retry the direct probe",
                )
            });
        };
        parse_ack(&response[ack_start..ack_end])
            .map_err(|error| error.with_image_id(submission.identity.image_id))?;

        let transcript = CapabilityTranscript::parse(&response);
        if !transcript.keyboard_query {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "Kitty keyboard query evidence is missing",
                "capability-probe",
                "restore the terminal and retry the direct probe",
            ));
        }
        let unrelated_input = response
            .iter()
            .enumerate()
            .filter(|(index, _)| *index < ack_start || *index >= ack_end)
            .map(|(_, byte)| *byte)
            .collect();
        Ok(CapabilityProbeResult {
            capabilities: TerminalCapabilityVector {
                state: CapabilityState::Valid,
                direct_ghostty: true,
                kitty_rgb_zlib: true,
                kitty_acknowledgements: true,
                kitty_keyboard: true,
                sgr_mouse_cell: true,
                sgr_mouse_pixel: true,
                focus_reporting: true,
                alternate_screen: true,
                resize_events: true,
            },
            unrelated_input,
            response_evidence: format!(
                "image_ack=true;keyboard_query=true;mouse_cell={};mouse_pixel={};focus={};resize={}",
                transcript.sgr_mouse_cell,
                transcript.sgr_mouse_pixel,
                transcript.focus_reporting,
                transcript.resize_events
            ),
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CapabilityTranscript {
    keyboard_query: bool,
    keyboard_event_types: bool,
    sgr_mouse_cell: bool,
    sgr_mouse_pixel: bool,
    focus_reporting: bool,
    resize_events: bool,
}

impl CapabilityTranscript {
    fn parse(bytes: &[u8]) -> Self {
        let keyboard_query = bytes.windows(3).any(|window| window == b"\x1b[?")
            && bytes.windows(1).any(|window| window == b"u");
        let keyboard_event_types = bytes.windows(5).any(|window| window == b";1:1u")
            || bytes.windows(5).any(|window| window == b";1:2u")
            || bytes.windows(4).any(|window| window == b"[?3u");
        let mouse = bytes.windows(3).any(|window| window == b"\x1b[<");
        Self {
            keyboard_query,
            keyboard_event_types,
            sgr_mouse_cell: mouse,
            sgr_mouse_pixel: mouse,
            focus_reporting: bytes.windows(3).any(|window| window == b"\x1b[I")
                || bytes.windows(3).any(|window| window == b"\x1b[O"),
            resize_events: bytes.windows(4).any(|window| window == b"\x1b[8;"),
        }
    }
}

fn locate_ack(bytes: &[u8], expected_image_id: u64) -> Option<(usize, usize)> {
    const PREFIX: &[u8] = b"\x1b_Gi=";
    const SUFFIX: &[u8] = b";OK\x1b\\";
    for start in 0..bytes.len() {
        if !bytes[start..].starts_with(PREFIX) {
            continue;
        }
        let suffix_start = bytes[start + PREFIX.len()..]
            .windows(SUFFIX.len())
            .position(|window| window == SUFFIX)?
            + start
            + PREFIX.len();
        let end = suffix_start + SUFFIX.len();
        let image_id = parse_ack(&bytes[start..end]).ok()?;
        if image_id == expected_image_id {
            return Some((start, end));
        }
    }
    None
}

fn validate_environment(environment: &TerminalEnvironment) -> Result<(), ViewportDiagnostic> {
    if environment.term.as_deref() != Some("xterm-ghostty")
        || environment.term_program.as_deref() != Some("ghostty")
    {
        return Err(diagnostic(
            ViewportDiagnosticCode::CapabilityDenied,
            "direct Ghostty identity is missing",
            "capability-probe",
            "run Interactive Modeling from the supported direct Ghostty attachment",
        ));
    }
    if environment.in_tmux || environment.over_ssh {
        return Err(diagnostic(
            ViewportDiagnosticCode::CapabilityDenied,
            "interactive viewport transport is indirect",
            "capability-probe",
            "run Interactive Modeling from a direct local Ghostty window",
        ));
    }
    if !environment.foreground_tty || !environment.utf8 {
        return Err(diagnostic(
            ViewportDiagnosticCode::CapabilityDenied,
            "foreground UTF-8 TTY baseline is unavailable",
            "capability-probe",
            "attach the application to a foreground UTF-8 TTY",
        ));
    }
    if environment.width == 0 || environment.height == 0 {
        return Err(diagnostic(
            ViewportDiagnosticCode::CapabilityDenied,
            "terminal dimensions are unavailable",
            "capability-probe",
            "retry after the terminal reports positive dimensions",
        ));
    }
    Ok(())
}

fn diagnostic(
    code: ViewportDiagnosticCode,
    detail: impl Into<String>,
    revision: impl Into<String>,
    recovery: impl Into<String>,
) -> ViewportDiagnostic {
    ViewportDiagnostic::new(code, detail, revision, recovery)
}

impl fmt::Display for CapabilityState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
        })
    }
}

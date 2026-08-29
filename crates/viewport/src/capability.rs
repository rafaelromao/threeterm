use std::env;
use std::fmt;
use std::io::{self, IsTerminal, Write};

use serde::Serialize;

use crate::diagnostic::{ViewportDiagnostic, ViewportDiagnosticCode};
use crate::kitty::{ENTER_SEQUENCE, GhosttyRenderer, parse_ack};
use crate::projection::ViewportFrame;
use crate::renderer::Renderer;

pub const MAX_PROBE_RESPONSE_BYTES: usize = 64 * 1024;

pub trait CapabilityProbeIo: Write {
    fn read_probe_response(&mut self, max_bytes: usize) -> io::Result<Vec<u8>>;
}

#[derive(Debug)]
struct ProbeWriter<'a, W: CapabilityProbeIo> {
    inner: &'a mut W,
    bytes: Vec<u8>,
}

impl<'a, W: CapabilityProbeIo> ProbeWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            bytes: Vec::new(),
        }
    }
}

impl<W: CapabilityProbeIo> Write for ProbeWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.inner.write_all(bytes)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
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

    fn identifies_direct_ghostty(&self) -> bool {
        self.term.as_deref() == Some("xterm-ghostty")
            && self.term_program.as_deref() == Some("ghostty")
            && !self.in_tmux
            && !self.over_ssh
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

    pub fn supports_interactive(&self) -> bool {
        self.is_valid()
            && self.direct_ghostty
            && self.kitty_rgb_zlib
            && self.kitty_acknowledgements
            && self.kitty_keyboard
            && self.sgr_mouse_cell
            && self.sgr_mouse_pixel
            && self.focus_reporting
            && self.alternate_screen
            && self.resize_events
    }

    pub fn invalidate(&mut self) {
        self.state = CapabilityState::Invalid;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityProbeResult {
    pub probe_nonce: u64,
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
        let (submission, replacement, wire) = {
            let mut capture = ProbeWriter::new(&mut *io);
            let mut renderer =
                GhosttyRenderer::for_probe(&mut capture).with_next_image_id(self.nonce);
            renderer.enter()?;
            let submission = renderer.submit_image(&frame, self.nonce)?;
            let replacement_token = self.nonce.checked_add(1).ok_or_else(|| {
                diagnostic(
                    ViewportDiagnosticCode::CapabilityMalformed,
                    "capability probe nonce cannot produce a replacement identity",
                    "capability-probe",
                    "start a new probe with a smaller nonce",
                )
            })?;
            let replacement = renderer.submit_image(
                &ViewportFrame {
                    generation: 1,
                    ..frame.clone()
                },
                replacement_token,
            )?;
            renderer.write_control(b"\x1b[?u", "capability-probe")?;
            drop(renderer);
            (submission, replacement, capture.bytes)
        };

        if count_occurrences(&wire, b"a=T,t=d") < 2
            || count_occurrences(&wire, b"a=d,d=I") < 2
            || !wire
                .windows(ENTER_SEQUENCE.len())
                .any(|window| window == ENTER_SEQUENCE)
        {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "capability probe did not complete the direct image lifecycle",
                "capability-probe",
                "discard the attachment and retry the direct probe",
            ));
        }

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
        let acknowledgement_ids = parse_acknowledgements(&response).map_err(|_| {
            diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "probe response contained malformed or ambiguous Kitty acknowledgements",
                "capability-probe",
                "discard the response and retry the direct probe",
            )
        })?;
        if !acknowledgement_ids.is_empty()
            && (acknowledgement_ids.iter().any(|image_id| {
                *image_id != submission.identity.image_id
                    && *image_id != replacement.identity.image_id
            }) || acknowledgement_ids
                .iter()
                .filter(|image_id| **image_id == submission.identity.image_id)
                .count()
                > 1
                || acknowledgement_ids
                    .iter()
                    .filter(|image_id| **image_id == replacement.identity.image_id)
                    .count()
                    > 1)
        {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "probe response contained stale, duplicate, or conflicting Kitty acknowledgements",
                "capability-probe",
                "discard the response and retry the direct probe",
            ));
        }
        let Some((first_ack_start, first_ack_end)) =
            locate_ack(&response, submission.identity.image_id)
        else {
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
        let Some((replacement_ack_start, replacement_ack_end)) =
            locate_ack(&response, replacement.identity.image_id)
        else {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "probe response omitted the replacement Kitty image acknowledgement",
                "capability-probe",
                "discard the response and retry the direct probe",
            ));
        };
        let first_acknowledged = parse_ack(&response[first_ack_start..first_ack_end])
            .map(|image_id| image_id == submission.identity.image_id)
            .map_err(|error| error.with_image_id(submission.identity.image_id))?;
        let replacement_acknowledged =
            parse_ack(&response[replacement_ack_start..replacement_ack_end])
                .map(|image_id| image_id == replacement.identity.image_id)
                .map_err(|error| error.with_image_id(replacement.identity.image_id))?;
        let kitty_acknowledgements = first_acknowledged && replacement_acknowledged;

        let transcript = CapabilityTranscript::from_setup_and_response(
            ENTER_SEQUENCE,
            &response,
            environment.width,
            environment.height,
        );
        if !transcript.is_complete() {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityMalformed,
                "direct attachment capability observations are incomplete",
                "capability-probe",
                "restore the terminal and retry the direct probe",
            ));
        }
        let unrelated_input = response
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                !(*index >= first_ack_start && *index < first_ack_end)
                    && !(*index >= replacement_ack_start && *index < replacement_ack_end)
            })
            .map(|(_, byte)| *byte)
            .collect();
        Ok(CapabilityProbeResult {
            probe_nonce: self.nonce,
            capabilities: TerminalCapabilityVector {
                state: CapabilityState::Valid,
                direct_ghostty: environment.identifies_direct_ghostty() && kitty_acknowledgements,
                kitty_rgb_zlib: kitty_acknowledgements,
                kitty_acknowledgements,
                kitty_keyboard: transcript.keyboard_query && transcript.keyboard_event_types,
                sgr_mouse_cell: transcript.sgr_mouse_cell,
                sgr_mouse_pixel: transcript.sgr_mouse_pixel,
                focus_reporting: transcript.focus_reporting,
                alternate_screen: transcript.alternate_screen,
                resize_events: transcript.resize_events,
            },
            unrelated_input,
            response_evidence: format!(
                "direct_ghostty={};image_ack={};rgb_zlib={};keyboard_query={};mouse_cell={};mouse_pixel={};focus={};alternate_screen={};resize={}",
                environment.identifies_direct_ghostty() && kitty_acknowledgements,
                kitty_acknowledgements,
                kitty_acknowledgements,
                transcript.keyboard_query && transcript.keyboard_event_types,
                transcript.sgr_mouse_cell,
                transcript.sgr_mouse_pixel,
                transcript.focus_reporting,
                transcript.alternate_screen,
                transcript.resize_events
            ),
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityTranscript {
    pub keyboard_query: bool,
    pub keyboard_event_types: bool,
    pub sgr_mouse_cell: bool,
    pub sgr_mouse_pixel: bool,
    pub focus_reporting: bool,
    pub alternate_screen: bool,
    pub resize_events: bool,
}

impl CapabilityTranscript {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::from_bytes_with_terminal_size(bytes, 80, 24)
    }

    pub fn from_setup_and_response(setup: &[u8], response: &[u8], width: u32, height: u32) -> Self {
        let mut transcript = Self::from_bytes_with_terminal_size(response, width, height);
        transcript.alternate_screen = setup
            .windows(ENTER_SEQUENCE.len())
            .any(|window| window == ENTER_SEQUENCE);
        transcript
    }

    fn from_bytes_with_terminal_size(bytes: &[u8], width: u32, height: u32) -> Self {
        let keyboard_query = bytes.windows(3).any(|window| window == b"\x1b[?")
            && bytes.windows(1).any(|window| window == b"u");
        let keyboard_event_types = bytes.windows(5).any(|window| window == b";1:1u")
            && bytes.windows(5).any(|window| window == b";1:2u");
        let mouse = mouse_evidence(bytes, width, height);
        Self {
            keyboard_query,
            keyboard_event_types,
            sgr_mouse_cell: mouse.cell_press && mouse.cell_drag && mouse.cell_release,
            sgr_mouse_pixel: mouse.pixel_press && mouse.pixel_drag && mouse.pixel_release,
            focus_reporting: bytes.windows(3).any(|window| window == b"\x1b[I")
                || bytes.windows(3).any(|window| window == b"\x1b[O"),
            alternate_screen: bytes
                .windows(b"\x1b[?1049h".len())
                .any(|window| window == b"\x1b[?1049h"),
            resize_events: bytes.windows(4).any(|window| window == b"\x1b[8;"),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.keyboard_query
            && self.keyboard_event_types
            && self.sgr_mouse_cell
            && self.sgr_mouse_pixel
            && self.focus_reporting
            && self.alternate_screen
            && self.resize_events
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MouseEvidence {
    cell_press: bool,
    cell_drag: bool,
    cell_release: bool,
    pixel_press: bool,
    pixel_drag: bool,
    pixel_release: bool,
}

fn mouse_evidence(bytes: &[u8], width: u32, height: u32) -> MouseEvidence {
    let mut evidence = MouseEvidence::default();
    const PREFIX: &[u8] = b"\x1b[<";
    for start in 0..bytes.len() {
        if !bytes[start..].starts_with(PREFIX) {
            continue;
        }
        let payload = &bytes[start + PREFIX.len()..];
        let Some(end) = payload
            .iter()
            .position(|byte| *byte == b'M' || *byte == b'm')
        else {
            continue;
        };
        let fields: Vec<_> = payload[..end].split(|byte| *byte == b';').collect();
        if fields.len() != 3 {
            continue;
        }
        let Ok(button) = std::str::from_utf8(fields[0])
            .unwrap_or_default()
            .parse::<u16>()
        else {
            continue;
        };
        let Ok(x) = std::str::from_utf8(fields[1])
            .unwrap_or_default()
            .parse::<u32>()
        else {
            continue;
        };
        let Ok(y) = std::str::from_utf8(fields[2])
            .unwrap_or_default()
            .parse::<u32>()
        else {
            continue;
        };
        let pixel = x > width || y > height;
        let release = payload[end] == b'm';
        let drag = !release && button & 32 != 0;
        let press = !release && !drag;
        match (pixel, press, drag, release) {
            (false, true, false, false) => evidence.cell_press = true,
            (false, false, true, false) => evidence.cell_drag = true,
            (false, false, false, true) => evidence.cell_release = true,
            (true, true, false, false) => evidence.pixel_press = true,
            (true, false, true, false) => evidence.pixel_drag = true,
            (true, false, false, true) => evidence.pixel_release = true,
            _ => {}
        }
    }
    evidence
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

fn parse_acknowledgements(bytes: &[u8]) -> Result<Vec<u64>, ()> {
    const PREFIX: &[u8] = b"\x1b_Gi=";
    const SUFFIX: &[u8] = b";OK\x1b\\";
    let mut cursor = 0;
    let mut image_ids = Vec::new();
    while cursor < bytes.len() {
        let Some(offset) = bytes[cursor..]
            .windows(PREFIX.len())
            .position(|window| window == PREFIX)
        else {
            break;
        };
        let start = cursor + offset;
        let Some(suffix_offset) = bytes[start + PREFIX.len()..]
            .windows(SUFFIX.len())
            .position(|window| window == SUFFIX)
        else {
            return Err(());
        };
        let suffix_start = start + PREFIX.len() + suffix_offset;
        let end = suffix_start + SUFFIX.len();
        image_ids.push(parse_ack(&bytes[start..end]).map_err(|_| ())?);
        cursor = end;
    }
    Ok(image_ids)
}

fn count_occurrences(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
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

use std::fmt;
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use threeterm_host::Host;
use threeterm_theme::{PaletteSources, ThemeContext, resolve_palette};
use threeterm_viewport::{
    CapabilityProbe, CapabilityProbeIo, CapabilityProbeResult, KittyPlacement, TerminalEnvironment,
    ViewportDiagnostic, ViewportDiagnosticCode, parse_ack,
};

use crate::{
    FocusCaptureEvent, InteractionEvent, InteractionMode, TerminalInput, TerminalInputDecoder,
    TuiViewportSession, decode_terminal_input,
};

pub const LAUNCH_SCHEMA_VERSION: &str = "threeterm.tui.launch/1";
pub const EXIT_CAPABILITY_FAILURE: i32 = 10;
pub const EXIT_LAUNCH_FAILURE: i32 = 11;
const INTERACTIVE_MODELING_ROUTE: &str = "interactive_modeling_unavailable";

pub trait InteractiveTerminal: CapabilityProbeIo + Write {
    fn read_event(&mut self) -> io::Result<Vec<u8>>;

    fn viewport_size(&self) -> (u32, u32);

    fn refresh_viewport_size(&mut self) -> (u32, u32) {
        self.viewport_size()
    }

    fn cleanup_signal(&self) -> Option<threeterm_viewport::CleanupSignal> {
        None
    }

    fn replay_probe_input(&mut self, _bytes: &[u8]) {}

    fn prepare(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchOutcome {
    pub event_loop_entered: bool,
}

#[derive(Debug)]
pub enum LaunchError {
    Capability(ViewportDiagnostic),
    Project(String),
    Viewport(ViewportDiagnostic),
    Runtime(String),
    Cleanup { source: Box<Self>, detail: String },
}

#[derive(Debug, Serialize)]
struct LaunchDiagnostic<'a> {
    schema_version: &'static str,
    code: &'static str,
    detail: String,
    source_revision: String,
    viewport_diagnostic: Option<&'a ViewportDiagnostic>,
    route: &'static str,
    recovery: String,
}

impl LaunchError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Capability(_) => EXIT_CAPABILITY_FAILURE,
            Self::Project(_) | Self::Viewport(_) | Self::Runtime(_) => EXIT_LAUNCH_FAILURE,
            Self::Cleanup { source, .. } => source.exit_code(),
        }
    }

    pub fn to_json(&self) -> String {
        if let Self::Cleanup { source, detail } = self {
            let mut envelope: Value =
                serde_json::from_str(&source.to_json()).expect("launch diagnostic is JSON");
            envelope["cleanup_error"] = Value::String(detail.clone());
            return serde_json::to_string(&envelope).expect("launch diagnostic is serializable");
        }
        let diagnostic = match self {
            Self::Capability(viewport) => LaunchDiagnostic {
                schema_version: LAUNCH_SCHEMA_VERSION,
                code: viewport.code.as_str(),
                detail: viewport.detail.clone(),
                source_revision: viewport.source_revision.clone(),
                viewport_diagnostic: Some(viewport),
                route: "headless_automation",
                recovery: viewport.recovery.clone(),
            },
            Self::Project(detail) => LaunchDiagnostic {
                schema_version: LAUNCH_SCHEMA_VERSION,
                code: "project_load_failed",
                detail: detail.clone(),
                source_revision: "unknown".to_string(),
                viewport_diagnostic: None,
                route: INTERACTIVE_MODELING_ROUTE,
                recovery: "repair or create the requested Project Generation before launching Interactive Modeling".to_string(),
            },
            Self::Viewport(viewport) => LaunchDiagnostic {
                schema_version: LAUNCH_SCHEMA_VERSION,
                code: viewport.code.as_str(),
                detail: viewport.detail.clone(),
                source_revision: viewport.source_revision.clone(),
                viewport_diagnostic: Some(viewport),
                route: INTERACTIVE_MODELING_ROUTE,
                recovery: viewport.recovery.clone(),
            },
            Self::Runtime(detail) => LaunchDiagnostic {
                schema_version: LAUNCH_SCHEMA_VERSION,
                code: "runtime_failure",
                detail: detail.clone(),
                source_revision: "unknown".to_string(),
                viewport_diagnostic: None,
                route: INTERACTIVE_MODELING_ROUTE,
                recovery: "restore the terminal and retry Interactive Modeling from the official attachment".to_string(),
            },
            Self::Cleanup { .. } => unreachable!("cleanup diagnostics are handled above"),
        };
        serde_json::to_string(&diagnostic).expect("launch diagnostic is serializable")
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_json())
    }
}

impl std::error::Error for LaunchError {}

pub fn launch<W: InteractiveTerminal>(
    host: &Host,
    root: impl AsRef<Path>,
    terminal: &mut W,
    environment: TerminalEnvironment,
) -> Result<LaunchOutcome, LaunchError> {
    let root = root.as_ref();
    let prepared = environment.foreground_tty;
    if prepared && let Err(error) = terminal.prepare() {
        return Err(with_restore_error(
            LaunchError::Runtime(format!("terminal setup failed: {error}")),
            terminal.restore(),
        ));
    }
    let placement = KittyPlacement {
        columns: environment.width,
        rows: environment.height,
    };

    let probe = match CapabilityProbe::new(fresh_probe_nonce()).probe(terminal, environment) {
        Ok(probe) => probe,
        Err(error) => {
            return Err(with_restore_error(
                LaunchError::Capability(error),
                terminal.restore(),
            ));
        }
    };
    if !probe.capabilities.supports_interactive() {
        let diagnostic = ViewportDiagnostic::new(
            threeterm_viewport::ViewportDiagnosticCode::CapabilityDenied,
            format!(
                "capability vector is insufficient: {}",
                probe.response_evidence
            ),
            "capability-probe",
            "complete a fresh direct-Ghostty capability probe before starting Interactive Modeling",
        )
        .with_evidence(probe.response_evidence.clone());
        return Err(with_restore_error(
            LaunchError::Capability(diagnostic),
            terminal.restore(),
        ));
    }

    if let Err(error) = host.load_with_geometry_replay(root) {
        return Err(with_restore_error(
            LaunchError::Project(error.to_string()),
            terminal.restore(),
        ));
    }
    let theme = resolve_palette(PaletteSources {
        cli: None,
        environment: std::env::var("THREETERM_PALETTE").ok().as_deref(),
        config: None,
    })
    .map(ThemeContext::from)
    .map_err(|error| {
        with_restore_error(
            LaunchError::Viewport(ViewportDiagnostic::new(
                ViewportDiagnosticCode::PaletteInvalid,
                format!("active palette could not be resolved: {error:?}"),
                "palette",
                "unset THREETERM_PALETTE or choose an embedded palette",
            )),
            terminal.restore(),
        )
    })?;
    let (width, height) = terminal.viewport_size();
    let launch_result = run_session(
        host, root, width, height, placement, terminal, &probe, theme,
    );
    let launch_result = with_restore_result(launch_result, terminal.restore());
    launch_result?;

    Ok(LaunchOutcome {
        event_loop_entered: true,
    })
}

fn fresh_probe_nonce() -> u64 {
    static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    (clock ^ NEXT_NONCE.fetch_add(1, Ordering::Relaxed)).max(1)
}

fn with_restore_error(source: LaunchError, restore: io::Result<()>) -> LaunchError {
    match restore {
        Ok(()) => source,
        Err(error) => LaunchError::Cleanup {
            source: Box::new(source),
            detail: format!("terminal restore failed: {error}"),
        },
    }
}

fn with_restore_result(
    result: Result<(), LaunchError>,
    restore: io::Result<()>,
) -> Result<(), LaunchError> {
    match restore {
        Ok(()) => result,
        Err(error) => match result {
            Ok(()) => Err(LaunchError::Runtime(format!(
                "terminal restore failed: {error}"
            ))),
            Err(source) => Err(LaunchError::Cleanup {
                source: Box::new(source),
                detail: format!("terminal restore failed: {error}"),
            }),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run_session<W: InteractiveTerminal>(
    host: &Host,
    root: &Path,
    width: u32,
    height: u32,
    placement: KittyPlacement,
    terminal: &mut W,
    probe: &CapabilityProbeResult,
    theme: ThemeContext,
) -> Result<(), LaunchError> {
    let session_result = TuiViewportSession::from_host_with_probe_and_theme(
        host,
        width,
        height,
        threeterm_viewport::GhosttyRenderer::new(terminal).with_placement(placement),
        probe,
        theme,
    );
    let launch_result = match session_result {
        Ok(mut session) => {
            let result = match catch_unwind(AssertUnwindSafe(|| {
                run_event_loop(&mut session, host, root, &probe.unrelated_input)
            })) {
                Ok(result) => result,
                Err(payload) => Err(LaunchError::Runtime(format!(
                    "interactive TUI panicked: {}",
                    panic_detail(payload)
                ))),
            };
            let cleanup = session.cleanup();
            drop(session);
            match (result, cleanup) {
                (Ok(()), Ok(())) => Ok(()),
                (Ok(()), Err(error)) => Err(LaunchError::Viewport(error)),
                (Err(error), Ok(())) => Err(error),
                (Err(error), Err(cleanup)) => Err(LaunchError::Cleanup {
                    source: Box::new(error),
                    detail: cleanup.to_string(),
                }),
            }
        }
        Err(error) => return Err(LaunchError::Viewport(error)),
    };
    launch_result?;
    Ok(())
}

fn run_event_loop<W: InteractiveTerminal>(
    session: &mut TuiViewportSession<threeterm_viewport::GhosttyRenderer<&mut W>>,
    host: &Host,
    root: &Path,
    replayed_probe_input: &[u8],
) -> Result<(), LaunchError> {
    let initial = session
        .render_current()
        .map_err(LaunchError::Viewport)?
        .started
        .ok_or_else(|| {
            LaunchError::Viewport(ViewportDiagnostic::new(
                ViewportDiagnosticCode::ProjectionFailed,
                "initial viewport frame was not submitted",
                "unknown",
                "restore the terminal and retry Interactive Modeling",
            ))
        })?;
    acknowledge_frame(session, initial.frame_token)?;
    session
        .coordinator_mut()
        .renderer_mut()
        .writer_mut()
        .replay_probe_input(replayed_probe_input);

    let mut input_decoder = TerminalInputDecoder::default();
    loop {
        if let Some(signal) = session
            .coordinator_mut()
            .renderer_mut()
            .writer_mut()
            .cleanup_signal()
        {
            handle_cleanup_signal(session, signal)?;
            return Ok(());
        }
        let bytes = match session
            .coordinator_mut()
            .renderer_mut()
            .writer_mut()
            .read_event()
        {
            Ok(bytes) => bytes,
            Err(error) => {
                if let Some(signal) = session
                    .coordinator_mut()
                    .renderer_mut()
                    .writer_mut()
                    .cleanup_signal()
                {
                    handle_cleanup_signal(session, signal)?;
                    return Ok(());
                }
                return Err(LaunchError::Runtime(format!(
                    "terminal input failed: {error}"
                )));
            }
        };
        let mut events = input_decoder.feed(&bytes);
        if bytes == b"\x1b" {
            events.extend(input_decoder.flush());
        }
        for event in events {
            if (event == b"q" || event == b"\x03") && !session.command_input_active() {
                session
                    .handle_close()
                    .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?;
                return Ok(());
            }
            if let Some((image_id, _)) = acknowledgement(&event) {
                let Some(active) = session.coordinator().in_flight().cloned() else {
                    return Err(LaunchError::Viewport(ViewportDiagnostic::new(
                        ViewportDiagnosticCode::AcknowledgementMismatch,
                        "Kitty acknowledgement arrived with no active frame",
                        "unknown",
                        "restore the terminal and retry Interactive Modeling",
                    )));
                };
                session
                    .acknowledge(threeterm_viewport::FrameAcknowledgement {
                        frame_token: active.frame_token,
                        image_id,
                    })
                    .map_err(LaunchError::Viewport)?;
                continue;
            }
            if let Some(input) = decode_terminal_input(&event) {
                let overlays = match input {
                    TerminalInput::FocusLost => vec![
                        session
                            .handle_focus_event(FocusCaptureEvent::FocusLost)
                            .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?
                            .overlay,
                    ],
                    TerminalInput::FocusIn
                        if session.state().focus == crate::FocusState::Focused =>
                    {
                        vec!["[ready-status] focus already active".to_string()]
                    }
                    TerminalInput::FocusIn => {
                        let recovery = session
                            .handle_focus_event(FocusCaptureEvent::FocusIn)
                            .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?;
                        let ready = session
                            .handle_focus_event(FocusCaptureEvent::RecoveryCompleted)
                            .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?;
                        vec![recovery.overlay, ready.overlay]
                    }
                    TerminalInput::Resize { rows, columns } => {
                        let (width, height) = session
                            .coordinator_mut()
                            .renderer_mut()
                            .writer_mut()
                            .refresh_viewport_size();
                        let (width, height) = if width == 0 || height == 0 {
                            (columns.saturating_mul(10), rows.saturating_mul(20))
                        } else {
                            (width, height)
                        };
                        let resized =
                            session.resize(width, height).map_err(|error| match error {
                                crate::TuiViewportError::Viewport(error) => {
                                    LaunchError::Viewport(error)
                                }
                                crate::TuiViewportError::Tui(error) => {
                                    LaunchError::Runtime(format!("{error:?}"))
                                }
                            })?;
                        vec![resized.started.overlay, resized.completed.overlay]
                    }
                    TerminalInput::Pick { x, y } if !session.command_input_active() => {
                        match session.pick_at(host, x, y) {
                            Ok(outcome) => {
                                let mut overlays = vec![outcome.overlay];
                                if let Some(candidate) = outcome.candidates.first().cloned() {
                                    let pressed = session
                                        .handle_focus_event(FocusCaptureEvent::PointerPressed {
                                            tool: crate::InteractionTool::Selection,
                                            origin: crate::PointerOrigin {
                                                column: x as u16,
                                                row: y as u16,
                                            },
                                            candidate: Some(candidate),
                                        })
                                        .map_err(|error| {
                                            LaunchError::Runtime(format!("{error:?}"))
                                        })?;
                                    overlays.push(pressed.overlay);
                                }
                                overlays
                            }
                            Err(error) => {
                                vec![format!("[error-glyph] Pick rejected: {error:?}")]
                            }
                        }
                    }
                    TerminalInput::PointerPressed { button, x, y }
                        if !session.command_input_active() =>
                    {
                        let tool = match button & 3 {
                            1 => crate::InteractionTool::Orbit,
                            2 => crate::InteractionTool::Pan,
                            _ => crate::InteractionTool::Selection,
                        };
                        if session.state().interaction_mode != InteractionMode::ModelessReady {
                            vec!["[ready-status] pointer press ignored".to_string()]
                        } else {
                            vec![
                                session
                                    .handle_focus_event(FocusCaptureEvent::PointerPressed {
                                        tool,
                                        origin: crate::PointerOrigin {
                                            column: x as u16,
                                            row: y as u16,
                                        },
                                        candidate: None,
                                    })
                                    .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?
                                    .overlay,
                            ]
                        }
                    }
                    TerminalInput::PointerMoved { .. } if !session.command_input_active() => {
                        if let crate::CaptureState::PointerCapture(capture) =
                            session.state().capture
                        {
                            let moved = session
                                .handle_focus_event(FocusCaptureEvent::PointerMoved {
                                    candidate: None,
                                })
                                .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?;
                            let mut overlays = vec![moved.overlay];
                            if session.state().interaction_mode == InteractionMode::ModelessReady {
                                overlays.push(
                                    session
                                        .handle_interaction_event(InteractionEvent::StartDrag {
                                            tool: capture.tool,
                                        })
                                        .map_err(|error| {
                                            LaunchError::Runtime(format!("{error:?}"))
                                        })?
                                        .overlay,
                                );
                            }
                            overlays
                        } else {
                            vec!["[ready-status] pointer motion ignored".to_string()]
                        }
                    }
                    TerminalInput::PointerReleased { .. } if !session.command_input_active() => {
                        if matches!(
                            session.state().interaction_mode,
                            InteractionMode::DragActive { .. }
                        ) {
                            vec![
                                session
                                    .handle_interaction_event(InteractionEvent::FinishDrag)
                                    .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?
                                    .overlay,
                            ]
                        } else if matches!(
                            session.state().capture,
                            crate::CaptureState::PointerCapture(_)
                        ) {
                            vec![
                                session
                                    .handle_focus_event(FocusCaptureEvent::PointerReleased)
                                    .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?
                                    .overlay,
                            ]
                        } else {
                            vec!["[ready-status] pointer release ignored".to_string()]
                        }
                    }
                    TerminalInput::PointerPressed { .. }
                    | TerminalInput::PointerMoved { .. }
                    | TerminalInput::PointerReleased { .. } => {
                        vec!["[ready-status] pointer input ignored".to_string()]
                    }
                    TerminalInput::TerminalReset => {
                        let transition =
                            session
                                .report_terminal_reset("terminal reset detected")
                                .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?;
                        let revision = session.state().canonical_revision;
                        session
                            .coordinator_mut()
                            .renderer_mut()
                            .write_control(
                                format!("\r\n{}\r\n", transition.overlay).as_bytes(),
                                &revision,
                            )
                            .map_err(LaunchError::Viewport)?;
                        return Ok(());
                    }
                    _ => vec![
                        session
                            .process_keyboard_input(&event, host, root)
                            .map_err(|error| match error {
                                crate::TuiViewportError::Viewport(error) => {
                                    LaunchError::Viewport(error)
                                }
                                crate::TuiViewportError::Tui(error) => {
                                    LaunchError::Runtime(format!("{error:?}"))
                                }
                            })?
                            .overlay,
                    ],
                };
                let revision = session.state().canonical_revision;
                for overlay in overlays {
                    let overlay = format!("\r\n{overlay}\r\n");
                    session
                        .coordinator_mut()
                        .renderer_mut()
                        .write_control(overlay.as_bytes(), &revision)
                        .map_err(LaunchError::Viewport)?;
                }
            } else {
                let revision = session.state().canonical_revision;
                session
                    .coordinator_mut()
                    .renderer_mut()
                    .write_control(
                        b"\r\n[error-glyph] Failure: unsupported terminal input\r\n",
                        &revision,
                    )
                    .map_err(LaunchError::Viewport)?;
            }
        }
    }
}

fn acknowledge_frame<W: InteractiveTerminal>(
    session: &mut TuiViewportSession<threeterm_viewport::GhosttyRenderer<&mut W>>,
    frame_token: u64,
) -> Result<(), LaunchError> {
    let bytes = session
        .coordinator_mut()
        .renderer_mut()
        .writer_mut()
        .read_event()
        .map_err(|error| {
            LaunchError::Runtime(format!(
                "initial frame acknowledgement read failed: {error}"
            ))
        })?;
    let Some((image_id, _)) = acknowledgement(&bytes) else {
        return Err(LaunchError::Viewport(ViewportDiagnostic::new(
            ViewportDiagnosticCode::AcknowledgementTimeout,
            "initial viewport frame was not acknowledged",
            "unknown",
            "restore the terminal and retry Interactive Modeling",
        )));
    };
    session
        .acknowledge(threeterm_viewport::FrameAcknowledgement {
            frame_token,
            image_id,
        })
        .map_err(LaunchError::Viewport)?;
    Ok(())
}

fn handle_cleanup_signal<W: InteractiveTerminal>(
    session: &mut TuiViewportSession<threeterm_viewport::GhosttyRenderer<&mut W>>,
    signal: threeterm_viewport::CleanupSignal,
) -> Result<(), LaunchError> {
    let _ =
        match signal {
            threeterm_viewport::CleanupSignal::Sigint => session
                .handle_sigint()
                .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?,
            threeterm_viewport::CleanupSignal::Sigterm => session
                .handle_sigterm()
                .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?,
            threeterm_viewport::CleanupSignal::Panic => session
                .handle_panic("terminal panic signal")
                .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?,
            threeterm_viewport::CleanupSignal::Close
            | threeterm_viewport::CleanupSignal::Normal => session
                .handle_close()
                .map_err(|error| LaunchError::Runtime(format!("{error:?}")))?,
        };
    Ok(())
}

fn panic_detail(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn acknowledgement(bytes: &[u8]) -> Option<(u64, usize)> {
    const PREFIX: &[u8] = b"\x1b_Gi=";
    const SUFFIX: &[u8] = b";OK\x1b\\";
    let start = bytes
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)?;
    let suffix_start = bytes[start + PREFIX.len()..]
        .windows(SUFFIX.len())
        .position(|window| window == SUFFIX)?
        + start
        + PREFIX.len();
    let end = suffix_start + SUFFIX.len();
    Some((parse_ack(&bytes[start..end]).ok()?, end))
}

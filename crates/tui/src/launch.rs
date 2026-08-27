use std::fmt;
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use threeterm_host::Host;
use threeterm_viewport::{
    CapabilityProbe, CapabilityProbeIo, CapabilityProbeResult, TerminalEnvironment,
    ViewportDiagnostic, ViewportDiagnosticCode, parse_ack,
};

use crate::{TuiViewportSession, decode_arrow_key};

pub const LAUNCH_SCHEMA_VERSION: &str = "threeterm.tui.launch/1";
pub const EXIT_CAPABILITY_FAILURE: i32 = 10;
pub const EXIT_LAUNCH_FAILURE: i32 = 11;

pub trait InteractiveTerminal: CapabilityProbeIo + Write {
    fn read_event(&mut self) -> io::Result<Vec<u8>>;

    fn viewport_size(&self) -> (u32, u32);

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
                route: "headless_automation",
                recovery: "repair or create the requested Project Generation before launching Interactive Modeling".to_string(),
            },
            Self::Viewport(viewport) => LaunchDiagnostic {
                schema_version: LAUNCH_SCHEMA_VERSION,
                code: viewport.code.as_str(),
                detail: viewport.detail.clone(),
                source_revision: viewport.source_revision.clone(),
                viewport_diagnostic: Some(viewport),
                route: "headless_automation",
                recovery: viewport.recovery.clone(),
            },
            Self::Runtime(detail) => LaunchDiagnostic {
                schema_version: LAUNCH_SCHEMA_VERSION,
                code: "runtime_failure",
                detail: detail.clone(),
                source_revision: "unknown".to_string(),
                viewport_diagnostic: None,
                route: "headless_automation",
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
    nonce: u64,
) -> Result<LaunchOutcome, LaunchError> {
    let prepared = environment.foreground_tty;
    if prepared {
        terminal
            .prepare()
            .map_err(|error| LaunchError::Runtime(format!("terminal setup failed: {error}")))?;
    }

    let probe = match CapabilityProbe::new(nonce).probe(terminal, environment) {
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

    if let Err(error) = host.load(root) {
        return Err(with_restore_error(
            LaunchError::Project(error.to_string()),
            terminal.restore(),
        ));
    }
    let (width, height) = terminal.viewport_size();
    let launch_result = run_session(host, width, height, terminal, &probe);
    let launch_result = with_restore_result(launch_result, terminal.restore());
    launch_result?;

    Ok(LaunchOutcome {
        event_loop_entered: true,
    })
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

fn run_session<W: InteractiveTerminal>(
    host: &Host,
    width: u32,
    height: u32,
    terminal: &mut W,
    probe: &CapabilityProbeResult,
) -> Result<(), LaunchError> {
    let session_result = TuiViewportSession::from_host_with_probe(
        host,
        width,
        height,
        threeterm_viewport::GhosttyRenderer::new(terminal),
        probe,
    );
    let launch_result = match session_result {
        Ok(mut session) => {
            let result = run_event_loop(&mut session);
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

    loop {
        let bytes = session
            .coordinator_mut()
            .renderer_mut()
            .writer_mut()
            .read_event()
            .map_err(|error| LaunchError::Runtime(format!("terminal input failed: {error}")))?;
        if bytes.is_empty() {
            continue;
        }
        if bytes == b"q" || bytes == b"\x03" {
            return Ok(());
        }
        if let Some((image_id, _)) = acknowledgement(&bytes) {
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
        if decode_arrow_key(&bytes).is_some() {
            session
                .process_terminal_input(&bytes)
                .map_err(|error| match error {
                    crate::TuiViewportError::Viewport(error) => LaunchError::Viewport(error),
                    crate::TuiViewportError::Tui(error) => {
                        LaunchError::Runtime(format!("{error:?}"))
                    }
                })?;
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

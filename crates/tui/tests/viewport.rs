use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_occt_worker::{LoftRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_theme::{PaletteSources, ThemeContext, resolve_palette};
use threeterm_tui::{TuiViewportError, TuiViewportSession};
use threeterm_viewport::{
    CapabilityProbeResult, CapabilityState, FrameAcknowledgement, GhosttyRenderer,
    TerminalCapabilityVector, ViewportDiagnosticCode,
};

#[derive(Debug, Default)]
struct RecordingWriter {
    bytes: Vec<u8>,
}

impl Write for RecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailingWriter {
    writes_before_failure: usize,
}

impl Write for FailingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.writes_before_failure == 0 {
            return Err(io::Error::other("injected terminal write failure"));
        }
        self.writes_before_failure -= 1;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-tui-viewport-{nanos}"))
}

fn admitted_renderer<W: Write>(writer: W) -> GhosttyRenderer<W> {
    let mut renderer = GhosttyRenderer::new(writer);
    renderer
        .admit(&valid_capabilities())
        .expect("test capability vector admits the renderer");
    renderer
}

fn valid_capabilities() -> TerminalCapabilityVector {
    TerminalCapabilityVector {
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
    }
}

fn probe_result() -> CapabilityProbeResult {
    CapabilityProbeResult {
        probe_nonce: 1,
        capabilities: valid_capabilities(),
        unrelated_input: Vec::new(),
        response_evidence: "test".to_string(),
    }
}

#[test]
fn host_backed_tui_submits_arrows_as_newest_camera_frames() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("first feature is persisted");
    host.save(&root, "feature-b", "fillet")
        .expect("second feature is persisted");
    let before = host.current().expect("canonical state exists");

    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host projection creates a viewport session");
    let first = session
        .process_terminal_input(b"\x1b[B")
        .expect("first arrow submits a frame");
    let _second = session
        .process_terminal_input(b"\x1b[C")
        .expect("second arrow becomes pending");
    let _third = session
        .process_terminal_input(b"\x1b[A")
        .expect("third arrow replaces the pending frame");

    assert!(first.submission.started.is_some());
    assert_eq!(kitty_transmissions(session.coordinator().renderer()), 1);
    assert_eq!(session.coordinator().dropped_frames().len(), 1);
    assert_eq!(session.coordinator().dropped_frames()[0].generation, 2);

    let first_identity = first.submission.started.expect("first frame is in flight");
    let first_ack = session
        .acknowledge(FrameAcknowledgement::from(&first_identity))
        .expect("first acknowledgement starts newest pending frame");
    assert_eq!(first_ack.visible.as_ref().unwrap().generation, 1);
    assert_eq!(first_ack.started.as_ref().unwrap().generation, 3);
    assert_eq!(kitty_transmissions(session.coordinator().renderer()), 2);
    let newest = first_ack.started.expect("newest frame is now in flight");
    let newest_ack = session
        .acknowledge(FrameAcknowledgement::from(&newest))
        .expect("newest frame is visible");
    assert_eq!(newest_ack.visible.as_ref().unwrap().generation, 3);
    assert_eq!(session.camera().yaw_degrees, 5);
    assert_eq!(session.camera().pitch_degrees, 20);
    assert_eq!(session.state().presentation_generation, 3);
    assert_eq!(host.current(), Some(before.clone()));

    let restoring = session
        .report_acknowledgement_timeout()
        .expect("renderer failure enters restoration");
    assert_eq!(
        restoring.state.lifecycle,
        threeterm_tui::LifecycleState::Restoring
    );
    assert_eq!(
        restoring
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(threeterm_tui::TuiDiagnosticCode::LifecycleFailure)
    );
    let headless = session
        .complete_viewport_restore()
        .expect("restore completes into the existing headless lifecycle");
    assert_eq!(
        headless.state.lifecycle,
        threeterm_tui::LifecycleState::HeadlessOnly
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_viewport_renders_a_committed_loft_tessellation() {
    let Ok(worker) = OcctWorker::locate() else {
        eprintln!(
            "production_viewport_renders_a_committed_loft_tessellation: OCCT worker unavailable"
        );
        return;
    };
    let root = temporary_bundle_root();
    Bundle::create(&root).expect("project bundle creates");
    let host = Host::new();
    host.load(&root).expect("project loads");
    host.loft(
        &root,
        LoftRequest::new(
            "viewport-loft-request",
            vec![
                vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
                vec![
                    [2.5, 2.5, 5.0],
                    [7.5, 2.5, 5.0],
                    [7.5, 7.5, 5.0],
                    [2.5, 7.5, 5.0],
                ],
            ],
        )
        .with_output_path(&root, "viewport-loft.brep")
        .with_feature_id("lofted-frustum"),
        &worker,
    )
    .expect("real loft commits through the host");

    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("committed loft loads through the production viewport path");
    let submitted = session
        .process_terminal_input(b"\x1b[B")
        .expect("viewport selection submits a frame");
    assert_eq!(
        session.state().selected_target.as_deref(),
        Some("lofted-frustum")
    );
    let identity = submitted.submission.started.expect("frame is in flight");
    let visible = session
        .acknowledge(FrameAcknowledgement::from(&identity))
        .expect("viewport acknowledgement makes the frame visible")
        .visible
        .expect("acknowledged frame is visible");
    assert!(
        visible
            .rgb
            .chunks_exact(3)
            .any(|pixel| pixel == [245, 194, 66]),
        "the selected committed loft must contribute solid pixels"
    );

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_viewport_history_selection_renders_stale_geometry_marker() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save_bracket(&root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
        .expect("history initializes");
    host.save(&root, "l-bracket-base", "history-feature")
        .expect("history feature is available to selection");
    host.historical_edit(&root, "l-bracket-base", "length", 0.0)
        .expect("failed edit commits its stale marker");
    let before = host.current().expect("canonical state exists");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host projection creates a viewport session");

    for _ in 0..10 {
        session
            .process_terminal_input(b"\x1b[B")
            .expect("selection enters the production viewport path");
        if session.state().selected_target.as_deref() == Some("l-bracket-base") {
            break;
        }
    }
    assert_eq!(
        session.state().selected_target.as_deref(),
        Some("l-bracket-base")
    );
    session
        .open_feature_timeline(&host, &root)
        .expect("history selection reloads stale geometry");
    let rendered = session
        .process_terminal_input(b"\x1b[B")
        .expect("history marker is rendered on live input");

    assert!(rendered.rendered.overlay.contains("[warning-glyph]"));
    assert!(
        rendered
            .rendered
            .overlay
            .contains("stale-last-valid-geometry")
    );
    assert!(rendered.rendered.overlay.contains("l-bracket-base"));
    assert_eq!(session.state().stale_last_valid_geometry.len(), 1);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn session_rejects_an_unadmitted_ghostty_renderer() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");

    let error = TuiViewportSession::from_host(
        &host,
        64,
        48,
        GhosttyRenderer::new(RecordingWriter::default()),
    )
    .expect_err("interactive sessions require capability admission");
    assert_eq!(error.code, ViewportDiagnosticCode::CapabilityDenied);
    TuiViewportSession::from_host_with_probe(
        &host,
        64,
        48,
        GhosttyRenderer::new(RecordingWriter::default()),
        &probe_result(),
    )
    .expect("a successful probe admits the production session");
    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

fn kitty_transmissions(renderer: &GhosttyRenderer<RecordingWriter>) -> usize {
    renderer
        .writer()
        .bytes
        .windows(b"a=T,t=d".len())
        .filter(|window| *window == b"a=T,t=d")
        .count()
}

#[test]
fn production_write_failure_is_structured_without_host_mutation() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");
    let mut session = TuiViewportSession::from_host(
        &host,
        64,
        48,
        admitted_renderer(FailingWriter {
            writes_before_failure: 1,
        }),
    )
    .expect("host projection creates a viewport session");

    let error = session
        .process_terminal_input(b"\x1b[B")
        .expect_err("terminal write failure is surfaced");
    match error {
        TuiViewportError::Viewport(diagnostic) => {
            assert_eq!(
                diagnostic.code,
                ViewportDiagnosticCode::TransportWriteFailed
            );
            assert_eq!(diagnostic.source_revision, before.revision_hash);
        }
        TuiViewportError::Tui(_) => panic!("terminal failure must retain viewport diagnostics"),
    }
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn host_viewport_path_emits_themed_marker_overlay_without_host_mutation() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");
    let theme = ThemeContext::from(
        resolve_palette(PaletteSources {
            cli: Some("sandman-light"),
            environment: None,
            config: None,
        })
        .expect("light palette resolves"),
    );
    let mut session = TuiViewportSession::from_host_with_theme(
        &host,
        64,
        48,
        admitted_renderer(RecordingWriter::default()),
        theme,
    )
    .expect("host-backed viewport accepts the resolved theme");

    let outcome = session
        .process_terminal_input(b"\x1b[B")
        .expect("the production viewport path renders the input");

    assert!(outcome.rendered.overlay.contains("[selection-glyph]"));
    assert!(outcome.rendered.overlay.contains("\x1b[38;2;"));
    assert!(outcome.rendered.overlay.ends_with("\x1b[0m"));
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

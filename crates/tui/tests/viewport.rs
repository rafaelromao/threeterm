use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_tui::{TuiViewportError, TuiViewportSession};
use threeterm_viewport::{
    FrameAcknowledgement, GhosttyRenderer, ViewportDiagnostic, ViewportDiagnosticCode,
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

#[derive(Debug, Default)]
struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("injected terminal write failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("injected terminal flush failure"))
    }
}

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-tui-viewport-{nanos}"))
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

    let mut session = TuiViewportSession::from_host(
        &host,
        64,
        48,
        GhosttyRenderer::new(RecordingWriter::default()),
    )
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
    assert_eq!(host.current(), Some(before.clone()));

    let failure = ViewportDiagnostic::new(
        ViewportDiagnosticCode::AcknowledgementTimeout,
        "Ghostty acknowledgement timed out",
        session.state().canonical_revision.clone(),
        "restore the terminal and run a fresh capability probe",
    );
    let restoring = session
        .report_viewport_failure(&failure)
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
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, GhosttyRenderer::new(FailingWriter))
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

use std::cell::Cell;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_tui::{LifecycleState, TuiViewportSession};
use threeterm_viewport::{
    CapabilityProbeResult, CapabilityState, GhosttyRenderer, TerminalCapabilityVector,
    TermiosRestorer,
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

#[derive(Debug, Clone)]
struct RecordingTermios(Rc<Cell<u8>>);

impl TermiosRestorer for RecordingTermios {
    fn restore(&mut self) -> Result<(), String> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-tui-cleanup-{nanos}"))
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
        capabilities: valid_capabilities(),
        unrelated_input: Vec::new(),
        response_evidence: "test".to_string(),
    }
}

#[test]
fn sigint_restores_shell_and_preserves_host() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");

    let termios_calls = Rc::new(Cell::new(0));
    let writer = RecordingWriter::default();
    let renderer =
        GhosttyRenderer::with_termios_restorer(writer, RecordingTermios(Rc::clone(&termios_calls)));
    let renderer = {
        let mut r = renderer;
        r.admit(&valid_capabilities()).expect("admit");
        r
    };

    let mut session =
        TuiViewportSession::from_host_with_probe(&host, 64, 48, renderer, &probe_result())
            .expect("session");

    // Create active image via input
    let _ = session
        .process_terminal_input(b"\x1b[B")
        .expect("first arrow submits a frame");

    // SIGINT handler – this is the tracer bullet
    let transition = session.handle_sigint().expect("sigint cleanup succeeds");

    assert_eq!(transition.state.lifecycle, LifecycleState::Closed);
    assert_eq!(host.current(), Some(before.clone()));
    assert_eq!(termios_calls.get(), 1);

    let bytes = session.coordinator().renderer().writer().bytes.clone();
    // must contain image deletion and terminal restore sequences
    assert!(
        bytes.windows(b"a=d,d=I".len()).any(|w| w == b"a=d,d=I"),
        "active image deleted"
    );
    assert!(bytes.windows(b"?1049l".len()).any(|w| w == b"?1049l"));
    assert!(bytes.windows(b"?1004l".len()).any(|w| w == b"?1004l"));
    assert!(bytes.windows(b"?25h".len()).any(|w| w == b"?25h"));
    assert!(bytes.windows(b"?1016l".len()).any(|w| w == b"?1016l"));
    assert!(bytes.windows(b"?1002l".len()).any(|w| w == b"?1002l"));
    assert!(bytes.windows(b"?2026l".len()).any(|w| w == b"?2026l"));
    assert!(bytes.windows(b"<u".len()).any(|w| w == b"<u"));

    // idempotent second call no extra bytes
    let size = bytes.len();
    let _ = session
        .handle_sigint()
        .expect_err("second sigint is idempotent invalid transition");
    assert_eq!(session.coordinator().renderer().writer().bytes.len(), size);
    assert_eq!(termios_calls.get(), 1);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn sigterm_parity_with_sigint() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");

    let termios_calls = Rc::new(Cell::new(0));
    let renderer = GhosttyRenderer::with_termios_restorer(
        RecordingWriter::default(),
        RecordingTermios(Rc::clone(&termios_calls)),
    );
    let renderer = {
        let mut r = renderer;
        r.admit(&valid_capabilities()).expect("admit");
        r
    };

    let mut session =
        TuiViewportSession::from_host_with_probe(&host, 64, 48, renderer, &probe_result())
            .expect("session");
    let _ = session
        .process_terminal_input(b"\x1b[B")
        .expect("first arrow submits a frame");

    let transition = session.handle_sigterm().expect("sigterm cleanup succeeds");
    assert_eq!(transition.state.lifecycle, LifecycleState::Closed);
    assert_eq!(host.current(), Some(before.clone()));
    assert_eq!(termios_calls.get(), 1);
    let bytes = session.coordinator().renderer().writer().bytes.clone();
    assert!(bytes.windows(b"a=d,d=I".len()).any(|w| w == b"a=d,d=I"));
    assert!(bytes.windows(b"?1049l".len()).any(|w| w == b"?1049l"));
    assert!(bytes.windows(b"?1016l".len()).any(|w| w == b"?1016l"));
    assert!(bytes.windows(b"?2026l".len()).any(|w| w == b"?2026l"));

    // sigterm after sigint idempotency
    let size = bytes.len();
    let _ = session
        .handle_sigint()
        .expect_err("sigint after sigterm invalid");
    assert_eq!(session.coordinator().renderer().writer().bytes.len(), size);
    assert_eq!(termios_calls.get(), 1);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn panic_cleanup_preserves_host_and_restores_terminal() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");

    let termios_calls = Rc::new(Cell::new(0));
    let renderer = GhosttyRenderer::with_termios_restorer(
        RecordingWriter::default(),
        RecordingTermios(Rc::clone(&termios_calls)),
    );
    let mut r = renderer;
    r.admit(&valid_capabilities()).expect("admit");
    let mut session = TuiViewportSession::from_host_with_probe(&host, 64, 48, r, &probe_result())
        .expect("session");
    let _ = session.process_terminal_input(b"\x1b[B").expect("submit");

    let transition = session
        .handle_panic("simulated panic")
        .expect("panic cleanup succeeds");
    assert_eq!(transition.state.lifecycle, LifecycleState::Closed);
    assert_eq!(host.current(), Some(before.clone()));
    assert_eq!(termios_calls.get(), 1);
    let bytes = session.coordinator().renderer().writer().bytes.clone();
    assert!(bytes.windows(b"a=d,d=I".len()).any(|w| w == b"a=d,d=I"));
    assert!(bytes.windows(b"?1049l".len()).any(|w| w == b"?1049l"));
    // second panic is idempotent
    let size = bytes.len();
    let _ = session
        .handle_panic("second")
        .expect_err("second panic invalid");
    assert_eq!(session.coordinator().renderer().writer().bytes.len(), size);
    assert_eq!(termios_calls.get(), 1);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn normal_close_regression() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");

    let termios_calls = Rc::new(Cell::new(0));
    let renderer = GhosttyRenderer::with_termios_restorer(
        RecordingWriter::default(),
        RecordingTermios(Rc::clone(&termios_calls)),
    );
    let mut r = renderer;
    r.admit(&valid_capabilities()).expect("admit");
    let mut session = TuiViewportSession::from_host_with_probe(&host, 64, 48, r, &probe_result())
        .expect("session");
    let _ = session.process_terminal_input(b"\x1b[B").expect("submit");

    let transition = session.handle_close().expect("close succeeds");
    assert_eq!(transition.state.lifecycle, LifecycleState::Closed);
    assert_eq!(host.current(), Some(before.clone()));
    assert_eq!(termios_calls.get(), 1);
    let bytes = session.coordinator().renderer().writer().bytes.clone();
    assert!(bytes.windows(b"a=d,d=I".len()).any(|w| w == b"a=d,d=I"));
    assert!(bytes.windows(b"?1049l".len()).any(|w| w == b"?1049l"));

    let size = bytes.len();
    let err = session.handle_close().expect_err("second close invalid");
    assert_eq!(
        err.code,
        threeterm_tui::TuiDiagnosticCode::InvalidTransition
    );
    assert_eq!(err.canonical_revision, before.revision_hash);
    assert_eq!(session.coordinator().renderer().writer().bytes.len(), size);
    assert_eq!(termios_calls.get(), 1);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn idempotent_multi_signal() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");

    let termios_calls = Rc::new(Cell::new(0));
    let renderer = GhosttyRenderer::with_termios_restorer(
        RecordingWriter::default(),
        RecordingTermios(Rc::clone(&termios_calls)),
    );
    let mut r = renderer;
    r.admit(&valid_capabilities()).expect("admit");
    let mut session = TuiViewportSession::from_host_with_probe(&host, 64, 48, r, &probe_result())
        .expect("session");
    let _ = session.process_terminal_input(b"\x1b[B").expect("submit");

    session.handle_sigint().expect("sigint");
    let bytes = session.coordinator().renderer().writer().bytes.clone();
    let size = bytes.len();
    let _ = session
        .handle_sigterm()
        .expect_err("sigterm after sigint invalid");
    assert_eq!(session.coordinator().renderer().writer().bytes.len(), size);
    let _ = session
        .handle_close()
        .expect_err("close after sigint invalid");
    assert_eq!(session.coordinator().renderer().writer().bytes.len(), size);
    assert_eq!(termios_calls.get(), 1);
    assert_eq!(session.state().lifecycle, LifecycleState::Closed);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn failure_retryable_preserves_host() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");

    #[derive(Debug)]
    struct FlakyWriter {
        bytes: Vec<u8>,
        failures_remaining: usize,
    }
    impl Write for FlakyWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.failures_remaining > 0 {
                self.failures_remaining -= 1;
                return Err(io::Error::other("injected"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut renderer = GhosttyRenderer::new(FlakyWriter {
        bytes: Vec::new(),
        failures_remaining: 0,
    });
    renderer.admit(&valid_capabilities()).expect("admit");
    let mut session =
        TuiViewportSession::from_host_with_probe(&host, 64, 48, renderer, &probe_result())
            .expect("session");
    let _ = session.process_terminal_input(b"\x1b[B").expect("submit");
    // inject failure for cleanup
    session
        .coordinator_mut()
        .renderer_mut()
        .writer_mut()
        .failures_remaining = 1;

    let err = session.handle_sigint().expect_err("first cleanup fails");
    assert_eq!(err.code, threeterm_tui::TuiDiagnosticCode::LifecycleFailure);
    assert_eq!(err.canonical_revision, before.revision_hash);
    assert!(err.detail.contains("SIGINT"));
    assert_eq!(host.current(), Some(before.clone()));

    // Flaky writer's single failure has been exhausted by the first cleanup attempt, so
    // the second coordinator cleanup should succeed and demonstrate retryability
    let retry = session.coordinator_mut().cleanup();
    assert!(
        retry.is_ok(),
        "coordinator cleanup should succeed after transient failure is exhausted: {retry:?}"
    );
    // Ensure host still preserved
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("cleanup");
}

use std::cell::Cell;
use std::io::{self, Write};
use std::rc::Rc;

use threeterm_viewport::{
    CapabilityState, GhosttyRenderer, Renderer, TerminalCapabilityVector, TermiosRestorer,
};

#[allow(dead_code)]
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

#[derive(Debug, Default)]
struct FlakyWriter {
    bytes: Vec<u8>,
    failures_remaining: usize,
}

impl Write for FlakyWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failures_remaining > 0 {
            self.failures_remaining -= 1;
            return Err(io::Error::other("injected cleanup write failure"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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

#[test]
fn panic_drop_cleans_terminal_best_effort() {
    let termios_calls = Rc::new(Cell::new(0));
    let writer_bytes = Rc::new(Cell::new(0)); // not used directly
    let bytes_holder = Rc::new(std::cell::RefCell::new(Vec::<u8>::new()));
    // Use a custom writer that shares bytes via Rc<RefCell> to inspect after panic Drop
    #[derive(Debug)]
    struct SharedWriter(Rc<std::cell::RefCell<Vec<u8>>>);
    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let shared = Rc::clone(&bytes_holder);
    let termios = RecordingTermios(Rc::clone(&termios_calls));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut renderer =
            GhosttyRenderer::with_termios_restorer(SharedWriter(Rc::clone(&shared)), termios);
        renderer.admit(&valid_capabilities()).expect("admit");
        renderer.enter().expect("enter");
        let frame = threeterm_viewport::ViewportFrame {
            revision: "revision-panic".to_string(),
            generation: 1,
            width: 1,
            height: 1,
            rgb: vec![1, 2, 3],
            frame_token: None,
        };
        renderer.submit_image(&frame, 1).expect("submit");
        panic!("simulated panic");
    }));
    assert!(result.is_err());
    assert_eq!(termios_calls.get(), 1);
    let bytes = bytes_holder.borrow().clone();
    assert!(bytes.windows(b"a=d,d=I".len()).any(|w| w == b"a=d,d=I"));
    assert!(bytes.windows(b"?1049l".len()).any(|w| w == b"?1049l"));
    assert!(bytes.windows(b"?1016l".len()).any(|w| w == b"?1016l"));
    assert!(bytes.windows(b"?2026l".len()).any(|w| w == b"?2026l"));
    let _ = writer_bytes;
}

#[test]
fn cleanup_failure_retryable_preserves_diagnostic() {
    let mut renderer = GhosttyRenderer::new(FlakyWriter {
        bytes: Vec::new(),
        failures_remaining: 0,
    });
    renderer.admit(&valid_capabilities()).expect("admit");
    renderer.enter().expect("enter");
    renderer
        .submit_image(
            &threeterm_viewport::ViewportFrame {
                revision: "revision-retry".to_string(),
                generation: 1,
                width: 1,
                height: 1,
                rgb: vec![1, 2, 3],
                frame_token: None,
            },
            1,
        )
        .expect("submit");
    renderer.writer_mut().failures_remaining = 1;
    let err = renderer.cleanup().expect_err("first cleanup fails");
    assert_eq!(
        err.code,
        threeterm_viewport::ViewportDiagnosticCode::CleanupFailed
    );
    assert!(err.detail.contains("retry terminal restoration") || err.recovery.contains("retry"));
    // retry succeeds
    renderer.cleanup().expect("retry succeeds");
    assert!(
        renderer
            .writer()
            .bytes
            .windows(b"?1049l".len())
            .any(|w| w == b"?1049l")
    );
}

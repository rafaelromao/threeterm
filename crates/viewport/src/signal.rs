use crate::diagnostic::ViewportDiagnostic;
use crate::renderer::Renderer;

/// Structured signal identity for renderer cleanup, used by signal handlers
/// and panic/close paths. All variants share the same terminal restoration
/// sequence: delete Active Viewport Image, disable Kitty keyboard, SGR mouse
/// pixel/cell/motion, focus reporting, synchronized output, show cursor,
/// reset attributes, exit alternate screen, and restore termios.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupSignal {
    Sigint,
    Sigterm,
    Panic,
    Close,
    Normal,
}

impl CleanupSignal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sigint => "SIGINT",
            Self::Sigterm => "SIGTERM",
            Self::Panic => "panic",
            Self::Close => "close",
            Self::Normal => "normal",
        }
    }
}

/// Perform renderer cleanup for the given signal, preserving the source
/// revision for diagnostics. The underlying `Renderer::cleanup` is idempotent
/// (GhosttyRenderer tracks `cleaned`) and retryable on transient write
/// failures (returns `CleanupFailed` with recovery hint).
#[allow(clippy::result_large_err)]
pub fn cleanup_on_signal<R: Renderer>(
    renderer: &mut R,
    signal: CleanupSignal,
    revision: &str,
) -> Result<(), ViewportDiagnostic> {
    match renderer.cleanup() {
        Ok(()) => Ok(()),
        Err(mut diagnostic) => {
            // Ensure the diagnostic carries the signal context if not already set
            if diagnostic.detail.is_empty() {
                diagnostic.detail = format!("{} cleanup failed", signal.as_str());
            }
            if diagnostic.recovery.is_empty() {
                // Preserve renderer's own recovery hint when present; only fill when missing
                diagnostic.recovery =
                    "retry terminal restoration from the owning lifecycle boundary".to_string();
            }
            // Preserve revision if diagnostic has unknown
            if diagnostic.source_revision == "unknown" {
                diagnostic.source_revision = revision.to_string();
            }
            Err(diagnostic)
        }
    }
}

/// Coordinator-level helper that shares the same signal-aware cleanup path
/// while also clearing pending/in-flight/visible state. This is the
/// production path used by `TuiViewportSession` signal handlers.
#[allow(clippy::result_large_err)]
pub fn cleanup_coordinator_on_signal<R: Renderer>(
    coordinator: &mut crate::renderer::RenderCoordinator<R>,
    signal: CleanupSignal,
    revision: &str,
) -> Result<(), ViewportDiagnostic> {
    // Delegate to coordinator cleanup which already handles pending/in_flight
    // clearing and renderer cleanup. We wrap to preserve signal context.
    match coordinator.cleanup() {
        Ok(()) => Ok(()),
        Err(mut diagnostic) => {
            if diagnostic.detail.is_empty() {
                diagnostic.detail = format!("{} cleanup failed", signal.as_str());
            }
            if diagnostic.source_revision == "unknown" {
                diagnostic.source_revision = revision.to_string();
            }
            Err(diagnostic)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kitty::GhosttyRenderer;
    use crate::{CapabilityState, TerminalCapabilityVector};
    use std::io::{self, Write};

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
    fn all_signals_share_same_cleanup_path() {
        for signal in [
            CleanupSignal::Sigint,
            CleanupSignal::Sigterm,
            CleanupSignal::Panic,
            CleanupSignal::Close,
            CleanupSignal::Normal,
        ] {
            let mut renderer = GhosttyRenderer::new(RecordingWriter::default());
            renderer.admit(&valid_capabilities()).expect("admit");
            renderer.enter().expect("enter");
            let frame = crate::ViewportFrame {
                revision: "rev".to_string(),
                generation: 1,
                width: 1,
                height: 1,
                rgb: vec![1, 2, 3],
                frame_token: None,
            };
            renderer.submit_image(&frame, 1).expect("submit");
            cleanup_on_signal(&mut renderer, signal, "rev").expect("cleanup");
            assert!(!renderer.is_valid());
            let bytes = renderer.writer().bytes.clone();
            assert!(bytes.windows(b"a=d,d=I".len()).any(|w| w == b"a=d,d=I"));
            assert!(bytes.windows(b"?1049l".len()).any(|w| w == b"?1049l"));
            // second call idempotent
            let size = bytes.len();
            let _ = cleanup_on_signal(&mut renderer, signal, "rev");
            assert_eq!(renderer.writer().bytes.len(), size);
        }
    }
}

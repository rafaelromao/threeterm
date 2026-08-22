use std::cell::Cell;
use std::io::{self, Write};
use std::rc::Rc;

use flate2::read::ZlibDecoder;
use std::io::Read;
use threeterm_viewport::{
    CapabilityState, FrameAcknowledgement, GhosttyRenderer, Renderer, TerminalCapabilityVector,
    TermiosRestorer, ViewportDiagnosticCode, ViewportFrame,
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

#[derive(Debug, Clone)]
struct RecordingTermios(Rc<Cell<u8>>);

impl TermiosRestorer for RecordingTermios {
    fn restore(&mut self) -> Result<(), String> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}

fn decode_base64(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in input.iter().copied().filter(|byte| *byte != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    output
}

fn admitted_backend() -> GhosttyRenderer<RecordingWriter> {
    let mut backend = GhosttyRenderer::new(RecordingWriter::default());
    backend
        .admit(&TerminalCapabilityVector {
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
        })
        .expect("test capability vector admits the renderer");
    backend
}

#[test]
fn unadmitted_renderer_rejects_frame_submission() {
    let frame = ViewportFrame {
        revision: "revision-gated".to_string(),
        generation: 1,
        width: 1,
        height: 1,
        rgb: vec![1, 2, 3],
        frame_token: None,
    };
    let mut backend = GhosttyRenderer::new(RecordingWriter::default());
    let diagnostic = backend
        .submit_image(&frame, 1)
        .expect_err("unadmitted renderers fail closed");
    assert_eq!(diagnostic.code, ViewportDiagnosticCode::CapabilityDenied);
    assert!(backend.writer().bytes.is_empty());
}

#[test]
fn ghostty_backend_emits_decodable_zlib_rgb_and_matches_acknowledgements() {
    let frame = ViewportFrame {
        revision: "revision-render".to_string(),
        generation: 7,
        width: 2,
        height: 1,
        rgb: vec![1, 2, 3, 200, 201, 202],
        frame_token: None,
    };
    let mut backend = admitted_backend();
    let submission = backend
        .submit_image(&frame, 41)
        .expect("RGB frame is emitted");
    let bytes = &backend.writer().bytes;
    assert!(bytes.starts_with(b"\x1b_G"));
    assert!(
        bytes
            .windows(b"a=T,t=d".len())
            .any(|window| window == b"a=T,t=d")
    );
    assert!(bytes.windows(b"o=z".len()).any(|window| window == b"o=z"));
    assert!(bytes.windows(b"m=0".len()).any(|window| window == b"m=0"));

    let start = bytes
        .iter()
        .position(|byte| *byte == b';')
        .expect("payload starts after the Kitty header")
        + 1;
    let end = bytes
        .windows(2)
        .position(|window| window == b"\x1b\\")
        .expect("Kitty command is terminated");
    let compressed = decode_base64(&bytes[start..end]);
    let mut decoded = Vec::new();
    ZlibDecoder::new(compressed.as_slice())
        .read_to_end(&mut decoded)
        .expect("payload is valid zlib");
    assert_eq!(decoded, frame.rgb);

    let ack = format!("\x1b_Gi={};OK\x1b\\", submission.identity.image_id);
    backend
        .acknowledge_bytes(41, ack.as_bytes())
        .expect("matching acknowledgement is accepted");
    let wrong = b"\x1b_Gi=999;OK\x1b\\";
    assert!(backend.acknowledge_bytes(41, wrong).is_err());
}

#[test]
fn image_ids_are_never_reused_after_acknowledged_replacement() {
    let frame = ViewportFrame {
        revision: "revision-render".to_string(),
        generation: 1,
        width: 1,
        height: 1,
        rgb: vec![1, 2, 3],
        frame_token: None,
    };
    let mut backend = admitted_backend();
    let first = backend
        .submit_image(&frame, 1)
        .expect("first frame is emitted");
    backend
        .acknowledge(&FrameAcknowledgement::from(&first.identity))
        .expect("first frame is acknowledged");
    let second = backend
        .submit_image(&frame.with_frame_token(2), 2)
        .expect("replacement frame is emitted");
    assert_ne!(first.identity.image_id, second.identity.image_id);
}

#[test]
fn large_rgb_frames_use_bounded_continuation_chunks() {
    let mut state = 0x9e3779b9u32;
    let rgb: Vec<u8> = (0..(200 * 200 * 3))
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 24) as u8
        })
        .collect();
    let frame = ViewportFrame {
        revision: "revision-chunks".to_string(),
        generation: 1,
        width: 200,
        height: 200,
        rgb,
        frame_token: None,
    };
    let mut backend = admitted_backend();
    backend
        .submit_image(&frame, 1)
        .expect("large RGB frame is emitted");
    let output = String::from_utf8(backend.writer().bytes.clone()).expect("wire is ASCII");
    let chunks: Vec<_> = output
        .split("\x1b\\")
        .filter(|chunk| chunk.contains("a=T,t=d"))
        .collect();
    assert!(chunks.len() > 1);
    for (index, chunk) in chunks.iter().enumerate() {
        let payload = chunk.split_once(';').expect("Kitty chunk has a payload").1;
        assert!(payload.len() <= 4096);
        if index + 1 == chunks.len() {
            assert!(chunk.contains("m=0"));
        } else {
            assert!(chunk.contains("m=1"));
        }
    }
}

#[test]
fn cleanup_restores_modes_deletes_the_active_image_and_is_idempotent() {
    let termios_calls = Rc::new(Cell::new(0));
    let mut backend = GhosttyRenderer::with_termios_restorer(
        RecordingWriter::default(),
        RecordingTermios(Rc::clone(&termios_calls)),
    );
    backend
        .admit(&TerminalCapabilityVector {
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
        })
        .expect("test capability vector admits the renderer");
    backend.enter().expect("terminal modes enter");
    let frame = ViewportFrame {
        revision: "revision-cleanup".to_string(),
        generation: 1,
        width: 1,
        height: 1,
        rgb: vec![1, 2, 3],
        frame_token: None,
    };
    backend
        .submit_image(&frame, 1)
        .expect("active image is emitted");
    backend.cleanup().expect("cleanup succeeds");
    let size = backend.writer().bytes.len();
    backend.cleanup().expect("cleanup can be repeated");
    assert_eq!(backend.writer().bytes.len(), size);
    assert_eq!(termios_calls.get(), 1);
    let output = &backend.writer().bytes;
    assert!(
        output
            .windows(b"?1049h".len())
            .any(|window| window == b"?1049h")
    );
    assert!(
        output
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        output
            .windows(b"?1004l".len())
            .any(|window| window == b"?1004l")
    );
    assert!(
        output
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert!(
        output
            .windows(b"?25h".len())
            .any(|window| window == b"?25h")
    );
}

#[test]
fn failed_cleanup_remains_retryable() {
    let mut backend = GhosttyRenderer::new(FlakyWriter {
        failures_remaining: 0,
        ..FlakyWriter::default()
    });
    backend
        .admit(&TerminalCapabilityVector {
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
        })
        .expect("test capability vector admits the renderer");
    backend.enter().expect("terminal modes enter");
    backend
        .submit_image(
            &ViewportFrame {
                revision: "revision-retry-cleanup".to_string(),
                generation: 1,
                width: 1,
                height: 1,
                rgb: vec![1, 2, 3],
                frame_token: None,
            },
            1,
        )
        .expect("active image is emitted");
    backend.writer_mut().failures_remaining = 1;
    let diagnostic = backend
        .cleanup()
        .expect_err("the injected cleanup failure is reported");
    assert_eq!(diagnostic.code, ViewportDiagnosticCode::CleanupFailed);
    backend
        .cleanup()
        .expect("cleanup retries after a transient write failure");
}

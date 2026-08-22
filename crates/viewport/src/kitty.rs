use std::fmt;
use std::io::{self, Write};

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::diagnostic::{ViewportDiagnostic, ViewportDiagnosticCode};
use crate::projection::{MAX_PIXELS, ViewportFrame};
use crate::renderer::{FrameAcknowledgement, FrameIdentity, Renderer, RendererSubmission};

pub const MAX_BASE64_CHUNK: usize = 4096;
pub const MAX_COMPRESSED_PAYLOAD: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KittyPlacement {
    pub columns: u32,
    pub rows: u32,
}

impl Default for KittyPlacement {
    fn default() -> Self {
        Self {
            columns: 1,
            rows: 1,
        }
    }
}

pub trait TermiosRestorer: fmt::Debug {
    fn restore(&mut self) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct NoopTermiosRestorer;

impl TermiosRestorer for NoopTermiosRestorer {
    fn restore(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct GhosttyRenderer<W: Write, T: TermiosRestorer = NoopTermiosRestorer> {
    writer: W,
    termios_restorer: T,
    placement: KittyPlacement,
    next_image_id: u64,
    active_submission: Option<FrameIdentity>,
    entered: bool,
    cleaned: bool,
    valid: bool,
}

impl<W: Write> GhosttyRenderer<W> {
    pub fn new(writer: W) -> Self {
        Self::with_termios_restorer(writer, NoopTermiosRestorer)
    }
}

impl<W: Write, T: TermiosRestorer> GhosttyRenderer<W, T> {
    pub fn with_termios_restorer(writer: W, termios_restorer: T) -> Self {
        Self {
            writer,
            termios_restorer,
            placement: KittyPlacement::default(),
            next_image_id: 1,
            active_submission: None,
            entered: false,
            cleaned: false,
            valid: true,
        }
    }

    pub fn with_placement(mut self, placement: KittyPlacement) -> Self {
        self.placement = placement;
        self
    }

    pub fn writer(&self) -> &W {
        &self.writer
    }

    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    pub fn write_control(
        &mut self,
        bytes: &[u8],
        revision: &str,
    ) -> Result<(), ViewportDiagnostic> {
        self.write_raw(bytes, revision, None, None, None)
    }

    pub fn with_next_image_id(mut self, image_id: u64) -> Self {
        self.next_image_id = image_id;
        self
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn enter(&mut self) -> Result<(), ViewportDiagnostic> {
        if self.entered {
            return Ok(());
        }
        self.write_raw(
            b"\x1b[?1049h\x1b[>3u\x1b[?1002h\x1b[?1006h\x1b[?1016h\x1b[?1004h\x1b[?2026h\x1b[?25l",
            "unknown",
            None,
            None,
            None,
        )?;
        self.entered = true;
        self.cleaned = false;
        Ok(())
    }

    pub fn acknowledge_bytes(
        &mut self,
        frame_token: u64,
        bytes: &[u8],
    ) -> Result<(), ViewportDiagnostic> {
        let image_id = parse_ack(bytes).map_err(|mut diagnostic| {
            diagnostic.frame_token = Some(frame_token);
            diagnostic
        })?;
        self.acknowledge(&FrameAcknowledgement {
            frame_token,
            image_id,
        })
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    fn emit_frame(
        &mut self,
        frame: &ViewportFrame,
        frame_token: u64,
    ) -> Result<RendererSubmission, ViewportDiagnostic> {
        if !self.valid {
            return Err(diagnostic(
                ViewportDiagnosticCode::CapabilityInvalidated,
                "Ghostty attachment is invalid",
                &frame.revision,
                "run a fresh capability probe before submitting a frame",
            )
            .with_frame_token(frame_token)
            .with_generation(frame.generation));
        }
        let pixel_count = u64::from(frame.width) * u64::from(frame.height);
        if pixel_count == 0 || pixel_count > MAX_PIXELS {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidDimensions,
                "frame dimensions exceed the renderer bound",
                &frame.revision,
                "discard the frame and rebuild it with supported dimensions",
            )
            .with_frame_token(frame_token)
            .with_generation(frame.generation));
        }
        let expected = pixel_count
            .checked_mul(3)
            .and_then(|size| usize::try_from(size).ok());
        if expected != Some(frame.rgb.len()) {
            return Err(diagnostic(
                ViewportDiagnosticCode::ProjectionFailed,
                "RGB payload length does not match frame dimensions",
                &frame.revision,
                "discard the disposable frame and rebuild the projection",
            )
            .with_frame_token(frame_token)
            .with_generation(frame.generation));
        }

        if let Some(active) = self.active_submission.clone() {
            self.write_command(
                &format!("a=d,d=I,i={}", active.image_id),
                &active.revision,
                Some(active.frame_token),
                Some(active.generation),
                Some(active.image_id),
            )?;
        }
        let image_id = self.next_image_id;
        self.next_image_id = self.next_image_id.checked_add(1).ok_or_else(|| {
            diagnostic(
                ViewportDiagnosticCode::TransportWriteFailed,
                "Kitty image identity exhausted",
                &frame.revision,
                "restart the interactive attachment",
            )
        })?;
        let compressed = compress_rgb(&frame.rgb).map_err(|error| {
            diagnostic(
                ViewportDiagnosticCode::ProjectionFailed,
                format!("RGB zlib compression failed: {error}"),
                &frame.revision,
                "discard the disposable frame and retry projection",
            )
            .with_frame_token(frame_token)
            .with_generation(frame.generation)
        })?;
        if compressed.len() > MAX_COMPRESSED_PAYLOAD {
            return Err(diagnostic(
                ViewportDiagnosticCode::ProjectionFailed,
                "compressed RGB payload exceeds the renderer bound",
                &frame.revision,
                "reduce viewport quality or dimensions",
            )
            .with_frame_token(frame_token)
            .with_generation(frame.generation));
        }
        let encoded = base64_encode(&compressed);
        let mut offset = 0;
        while offset < encoded.len() {
            let end = (offset + MAX_BASE64_CHUNK).min(encoded.len());
            let more = usize::from(end < encoded.len());
            let command = format!(
                "a=T,t=d,f=24,s={},v={},i={},o=z,q=0,c={},r={},m={};{}",
                frame.width,
                frame.height,
                image_id,
                self.placement.columns,
                self.placement.rows,
                more,
                &encoded[offset..end]
            );
            self.write_command(
                &command,
                &frame.revision,
                Some(frame_token),
                Some(frame.generation),
                Some(image_id),
            )?;
            offset = end;
        }
        if encoded.is_empty() {
            let command = format!(
                "a=T,t=d,f=24,s={},v={},i={},o=z,q=0,c={},r={},m=0;",
                frame.width, frame.height, image_id, self.placement.columns, self.placement.rows
            );
            self.write_command(
                &command,
                &frame.revision,
                Some(frame_token),
                Some(frame.generation),
                Some(image_id),
            )?;
        }
        let identity = FrameIdentity {
            frame_token,
            generation: frame.generation,
            revision: frame.revision.clone(),
            image_id,
        };
        self.active_submission = Some(identity.clone());
        Ok(RendererSubmission { identity })
    }

    fn cleanup_inner(&mut self) -> Result<(), ViewportDiagnostic> {
        if self.cleaned {
            return Ok(());
        }
        self.cleaned = true;
        let revision = self
            .active_submission
            .as_ref()
            .map_or("unknown", |submission| submission.revision.as_str())
            .to_string();
        let mut failures = Vec::new();
        if let Some(active) = self.active_submission.take()
            && let Err(error) = self.write_command(
                &format!("a=d,d=I,i={}", active.image_id),
                &revision,
                Some(active.frame_token),
                Some(active.generation),
                Some(active.image_id),
            )
        {
            failures.push(error.detail);
        }
        for sequence in [
            "\x1b[<u",
            "\x1b[?1016l\x1b[?1006l\x1b[?1002l\x1b[?1004l",
            "\x1b[?2026l",
            "\x1b[?25h\x1b[0m\x1b[?1049l",
        ] {
            if let Err(error) = self.write_raw(sequence.as_bytes(), &revision, None, None, None) {
                failures.push(error.detail);
            }
        }
        if let Err(error) = self.termios_restorer.restore() {
            failures.push(error);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(diagnostic(
                ViewportDiagnosticCode::CleanupFailed,
                failures.join("; "),
                revision,
                "retry terminal restoration from the owning lifecycle boundary",
            ))
        }
    }

    fn write_command(
        &mut self,
        command: &str,
        revision: &str,
        frame_token: Option<u64>,
        generation: Option<u64>,
        image_id: Option<u64>,
    ) -> Result<(), ViewportDiagnostic> {
        let mut bytes = Vec::with_capacity(command.len() + 8);
        bytes.extend_from_slice(b"\x1b_G");
        bytes.extend_from_slice(command.as_bytes());
        bytes.extend_from_slice(b"\x1b\\");
        self.write_raw(&bytes, revision, frame_token, generation, image_id)
    }

    fn write_raw(
        &mut self,
        bytes: &[u8],
        revision: &str,
        frame_token: Option<u64>,
        generation: Option<u64>,
        image_id: Option<u64>,
    ) -> Result<(), ViewportDiagnostic> {
        if let Err(error) = self
            .writer
            .write_all(bytes)
            .and_then(|_| self.writer.flush())
        {
            self.valid = false;
            let mut diagnostic = diagnostic(
                ViewportDiagnosticCode::TransportWriteFailed,
                error.to_string(),
                revision,
                "invalidate the attachment and restore the terminal",
            );
            diagnostic.frame_token = frame_token;
            diagnostic.generation = generation;
            diagnostic.image_id = image_id;
            return Err(diagnostic);
        }
        Ok(())
    }
}

impl<W: Write, T: TermiosRestorer> Renderer for GhosttyRenderer<W, T> {
    fn submit_image(
        &mut self,
        frame: &ViewportFrame,
        frame_token: u64,
    ) -> Result<RendererSubmission, ViewportDiagnostic> {
        self.emit_frame(frame, frame_token)
    }

    fn request_cancel(&mut self, active: Option<&FrameIdentity>) -> Result<(), ViewportDiagnostic> {
        let Some(active) = active else {
            return Ok(());
        };
        let Some(current) = self.active_submission.clone() else {
            return Ok(());
        };
        if &current != active {
            return Err(ViewportDiagnostic::new(
                ViewportDiagnosticCode::AcknowledgementMismatch,
                "cancel request does not match the active Kitty image",
                &current.revision,
                "discard the stale cancellation request",
            )
            .with_frame_token(active.frame_token)
            .with_generation(current.generation)
            .with_image_id(active.image_id));
        }
        self.write_command(
            &format!("a=d,d=I,i={}", current.image_id),
            &current.revision,
            Some(current.frame_token),
            Some(current.generation),
            Some(current.image_id),
        )
    }

    fn acknowledge(
        &mut self,
        acknowledgement: &FrameAcknowledgement,
    ) -> Result<(), ViewportDiagnostic> {
        let Some(current) = self.active_submission.as_ref() else {
            self.valid = false;
            return Err(ViewportDiagnostic::new(
                ViewportDiagnosticCode::AcknowledgementMismatch,
                "Kitty acknowledgement arrived with no active image",
                "unknown",
                "discard the late acknowledgement",
            )
            .with_frame_token(acknowledgement.frame_token)
            .with_image_id(acknowledgement.image_id));
        };
        if current.frame_token != acknowledgement.frame_token
            || current.image_id != acknowledgement.image_id
        {
            self.valid = false;
            return Err(ViewportDiagnostic::new(
                ViewportDiagnosticCode::AcknowledgementMismatch,
                "Kitty acknowledgement does not match the active image",
                &current.revision,
                "discard the acknowledgement and retain the current image",
            )
            .with_frame_token(acknowledgement.frame_token)
            .with_generation(current.generation)
            .with_image_id(acknowledgement.image_id));
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), ViewportDiagnostic> {
        self.cleanup_inner()
    }
}

impl<W: Write, T: TermiosRestorer> Drop for GhosttyRenderer<W, T> {
    fn drop(&mut self) {
        let _ = self.cleanup_inner();
    }
}

pub fn parse_ack(bytes: &[u8]) -> Result<u64, ViewportDiagnostic> {
    const PREFIX: &[u8] = b"\x1b_Gi=";
    const SUFFIX: &[u8] = b";OK\x1b\\";
    if !bytes.starts_with(PREFIX) || !bytes.ends_with(SUFFIX) {
        return Err(ViewportDiagnostic::new(
            ViewportDiagnosticCode::CapabilityMalformed,
            "Kitty acknowledgement has an unsupported shape",
            "unknown",
            "discard the response and re-probe the attachment",
        ));
    }
    let id_bytes = &bytes[PREFIX.len()..bytes.len() - SUFFIX.len()];
    if id_bytes.is_empty() || !id_bytes.iter().all(u8::is_ascii_digit) {
        return Err(ViewportDiagnostic::new(
            ViewportDiagnosticCode::CapabilityMalformed,
            "Kitty acknowledgement image identity is not numeric",
            "unknown",
            "discard the response and re-probe the attachment",
        ));
    }
    let image_id = std::str::from_utf8(id_bytes)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            ViewportDiagnostic::new(
                ViewportDiagnosticCode::CapabilityMalformed,
                "Kitty acknowledgement image identity is out of range",
                "unknown",
                "discard the response and re-probe the attachment",
            )
        })?;
    if image_id == 0 {
        return Err(ViewportDiagnostic::new(
            ViewportDiagnosticCode::CapabilityMalformed,
            "Kitty acknowledgement image identity must be non-zero",
            "unknown",
            "discard the response and re-probe the attachment",
        ));
    }
    Ok(image_id)
}

fn compress_rgb(rgb: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(rgb)?;
    encoder.finish()
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        output.push(TABLE[(first >> 2) as usize] as char);
        let second = chunk.get(1).copied();
        output.push(
            TABLE[((first & 0x03) << 4 | second.map_or(0, |value| value >> 4)) as usize] as char,
        );
        if let Some(second) = second {
            output.push(
                TABLE[((second & 0x0f) << 2 | chunk.get(2).copied().map_or(0, |value| value >> 6))
                    as usize] as char,
            );
        } else {
            output.push('=');
        }
        if let Some(third) = chunk.get(2).copied() {
            output.push(TABLE[(third & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn diagnostic(
    code: ViewportDiagnosticCode,
    detail: impl Into<String>,
    revision: impl Into<String>,
    recovery: impl Into<String>,
) -> ViewportDiagnostic {
    ViewportDiagnostic::new(code, detail, revision, recovery)
}

//! Newline-framed JSON envelope parser.
//!
//! See `frame::FrameParser` for the public API.

use std::fmt;

use serde_json::Value;

use crate::worker::Envelope;

/// Maximum cumulative buffer size for `FrameParser::push`. Envelopes that
/// would push the buffer past this limit emit `FrameError::PayloadTooLarge`
/// so a hostile or buggy peer cannot exhaust the host's memory by
/// refusing to send a newline.
pub const MAX_FRAME_BUFFER: usize = 4 * 1024 * 1024;

/// Incremental newline-framed JSON parser.
///
/// `FrameParser` accepts arbitrarily chunked byte input via `push` and
/// returns every complete envelope it can decode from the chunk. The
/// parser buffers any incomplete trailing line so the next `push` can
/// finish it. A single malformed frame aborts the current chunk with a
/// structured `FrameError` so the caller can route it into the
/// supervisor's diagnostic surface (closed issue #49: newline-framed JSON
/// control plane).
#[derive(Debug)]
pub struct FrameParser {
    buffer: Vec<u8>,
}

impl FrameParser {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Feed `bytes` into the parser. Returns every complete envelope
    /// found in `bytes` (joined with any carry-over from prior calls).
    /// A malformed frame aborts the current chunk with a structured
    /// `FrameError`; the buffered bytes are then dropped, so the caller
    /// can resync on the next chunk via `push`.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Envelope>, FrameError> {
        let size = self.buffer.len().saturating_add(bytes.len());
        if size > MAX_FRAME_BUFFER {
            self.buffer.clear();
            return Err(FrameError::PayloadTooLarge {
                size,
                max: MAX_FRAME_BUFFER,
            });
        }
        self.buffer.extend_from_slice(bytes);

        let mut envelopes = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line_bytes: Vec<u8> = self.buffer.drain(..=newline).collect();
            let line = &line_bytes[..line_bytes.len() - 1];
            if line.is_empty() {
                continue;
            }
            let line_str = std::str::from_utf8(line)
                .inspect_err(|_| {
                    self.buffer.clear();
                })
                .map_err(|_| FrameError::NonUtf8)?;
            let value: Value = serde_json::from_str(line_str).map_err(FrameError::InvalidJson)?;
            let kind_string = value
                .get("kind")
                .and_then(Value::as_str)
                .map(str::to_string);
            let envelope: Envelope = match kind_string.as_deref() {
                None => {
                    self.buffer.clear();
                    return Err(FrameError::MissingKind);
                }
                Some(kind) => serde_json::from_value(value)
                    .inspect_err(|_| {
                        self.buffer.clear();
                    })
                    .map_err(|error| {
                        let message = error.to_string();
                        if message.contains("unknown variant") || message.contains("unknown kind") {
                            FrameError::UnknownKind(kind.to_string())
                        } else {
                            FrameError::InvalidJson(error)
                        }
                    })?,
            };
            envelopes.push(envelope);
        }
        Ok(envelopes)
    }

    /// Drop any buffered bytes. Used after a malformed frame aborts the
    /// parser so the supervisor can resync.
    pub fn reset(&mut self) {
        self.buffer.clear();
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Structured failure modes emitted by `FrameParser`. Every variant is a
/// stable, presentation-neutral identifier the supervisor routes into
/// the diagnostic taxonomy. `Display` formats are stable for logs but
/// callers should switch on `FrameError` directly.
#[derive(Debug)]
pub enum FrameError {
    /// A frame contained a non-UTF8 byte. The line is dropped.
    NonUtf8,
    /// A frame's body was not valid JSON.
    InvalidJson(serde_json::Error),
    /// A frame's JSON body had no `kind` discriminator or had a non-string
    /// `kind`.
    MissingKind,
    /// A frame's `kind` was not a registered envelope discriminator.
    UnknownKind(String),
    /// The cumulative buffer exceeded `MAX_FRAME_BUFFER` without a
    /// newline-terminated frame.
    PayloadTooLarge { size: usize, max: usize },
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonUtf8 => formatter.write_str("frame contains non-UTF8 bytes"),
            Self::InvalidJson(error) => write!(formatter, "frame is not valid JSON: {error}"),
            Self::MissingKind => formatter.write_str("frame is missing the `kind` discriminator"),
            Self::UnknownKind(kind) => {
                write!(
                    formatter,
                    "frame carries an unknown envelope kind: {kind:?}"
                )
            }
            Self::PayloadTooLarge { size, max } => write!(
                formatter,
                "frame buffer exceeded maximum size: {size} > {max}"
            ),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJson(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(envelope: &Envelope) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(envelope).expect("envelope serializes");
        bytes.push(b'\n');
        bytes
    }

    #[test]
    fn push_decodes_a_single_complete_frame() {
        let envelope = Envelope::WorkerReady {
            schema_version: crate::schema_version().to_string(),
            worker_id: "occt-worker".to_string(),
        };
        let mut parser = FrameParser::new();
        let decoded = parser
            .push(&encode(&envelope))
            .expect("single complete frame decodes");
        assert_eq!(decoded, vec![envelope]);
    }

    #[test]
    fn push_buffers_a_truncated_trailing_line_until_the_next_chunk() {
        let envelope = Envelope::WorkerReady {
            schema_version: crate::schema_version().to_string(),
            worker_id: "occt-worker".to_string(),
        };
        let full = encode(&envelope);
        let split = full.len() / 2;
        let mut parser = FrameParser::new();
        let first = parser
            .push(&full[..split])
            .expect("partial frame does not error");
        assert!(
            first.is_empty(),
            "partial frame must not yield an envelope; got {first:?}"
        );
        let second = parser
            .push(&full[split..])
            .expect("completed frame decodes");
        assert_eq!(second, vec![envelope]);
    }

    #[test]
    fn push_decodes_two_frames_in_one_chunk() {
        let ready = Envelope::WorkerReady {
            schema_version: crate::schema_version().to_string(),
            worker_id: "occt-worker".to_string(),
        };
        let progress = Envelope::Progress {
            schema_version: crate::schema_version().to_string(),
            request_id: "req-1".to_string(),
            stage: "tessellating".to_string(),
            percent: 25,
        };
        let mut parser = FrameParser::new();
        let mut chunk = encode(&ready);
        chunk.extend_from_slice(&encode(&progress));
        let decoded = parser.push(&chunk).expect("two frames decode");
        assert_eq!(decoded, vec![ready, progress]);
    }

    #[test]
    fn push_rejects_non_utf8_bytes() {
        let mut parser = FrameParser::new();
        let error = parser
            .push(b"\xff\xfe\xfd\n")
            .expect_err("non-UTF8 frame must be rejected");
        assert!(
            matches!(error, FrameError::NonUtf8),
            "expected NonUtf8; got {error:?}"
        );
        assert!(parser.buffer.is_empty(), "buffer cleared after error");
    }

    #[test]
    fn push_rejects_invalid_json() {
        let mut parser = FrameParser::new();
        let error = parser
            .push(b"this is not json\n")
            .expect_err("non-JSON frame must be rejected");
        assert!(
            matches!(error, FrameError::InvalidJson(_)),
            "expected InvalidJson; got {error:?}"
        );
        assert!(parser.buffer.is_empty(), "buffer cleared after error");
    }

    #[test]
    fn push_rejects_frame_missing_kind_discriminator() {
        let mut parser = FrameParser::new();
        let error = parser
            .push(b"{\"schema_version\":\"threeterm.protocol/1\"}\n")
            .expect_err("frame without `kind` must be rejected");
        assert!(
            matches!(error, FrameError::MissingKind),
            "expected MissingKind; got {error:?}"
        );
        assert!(parser.buffer.is_empty(), "buffer cleared after error");
    }

    #[test]
    fn push_rejects_non_string_kind_discriminator() {
        let mut parser = FrameParser::new();
        let error = parser
            .push(b"{\"kind\":42}\n")
            .expect_err("frame with non-string `kind` must be rejected");
        assert!(
            matches!(error, FrameError::MissingKind),
            "expected MissingKind for non-string kind; got {error:?}"
        );
        assert!(parser.buffer.is_empty(), "buffer cleared after error");
    }

    #[test]
    fn reset_drops_buffered_bytes() {
        let mut parser = FrameParser::new();
        let _ = parser
            .push(b"{\"kind\":\"worker_ready\"")
            .expect("partial frame does not error");
        assert!(!parser.buffer.is_empty());
        parser.reset();
        assert!(parser.buffer.is_empty(), "reset clears the buffer");
    }
}

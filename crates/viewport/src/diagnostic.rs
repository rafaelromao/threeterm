use std::fmt;

use serde::Serialize;

pub const SCHEMA_VERSION: &str = "threeterm.viewport/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewportDiagnosticCode {
    InvalidDimensions,
    InvalidScene,
    ProjectionFailed,
    FrameDropped,
    FrameCancelled,
    RendererBusy,
    AcknowledgementMismatch,
    AcknowledgementTimeout,
    TransportWriteFailed,
    CapabilityDenied,
    CapabilityTimeout,
    CapabilityMalformed,
    CapabilityInvalidated,
    CleanupFailed,
}

impl ViewportDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidDimensions => "invalid_dimensions",
            Self::InvalidScene => "invalid_scene",
            Self::ProjectionFailed => "projection_failed",
            Self::FrameDropped => "frame_dropped",
            Self::FrameCancelled => "frame_cancelled",
            Self::RendererBusy => "renderer_busy",
            Self::AcknowledgementMismatch => "acknowledgement_mismatch",
            Self::AcknowledgementTimeout => "acknowledgement_timeout",
            Self::TransportWriteFailed => "transport_write_failed",
            Self::CapabilityDenied => "capability_denied",
            Self::CapabilityTimeout => "capability_timeout",
            Self::CapabilityMalformed => "capability_malformed",
            Self::CapabilityInvalidated => "capability_invalidated",
            Self::CleanupFailed => "cleanup_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewportDiagnostic {
    pub code: ViewportDiagnosticCode,
    pub detail: String,
    pub schema_version: &'static str,
    pub source_revision: String,
    pub frame_token: Option<u64>,
    pub generation: Option<u64>,
    pub image_id: Option<u64>,
    pub evidence: Option<String>,
    pub recovery: String,
}

impl ViewportDiagnostic {
    pub fn new(
        code: ViewportDiagnosticCode,
        detail: impl Into<String>,
        source_revision: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        Self {
            code,
            detail: detail.into(),
            schema_version: SCHEMA_VERSION,
            source_revision: source_revision.into(),
            frame_token: None,
            generation: None,
            image_id: None,
            evidence: None,
            recovery: recovery.into(),
        }
    }

    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = Some(generation);
        self
    }

    pub fn with_frame_token(mut self, frame_token: u64) -> Self {
        self.frame_token = Some(frame_token);
        self
    }

    pub fn with_image_id(mut self, image_id: u64) -> Self {
        self.image_id = Some(image_id);
        self
    }

    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence = Some(evidence.into());
        self
    }
}

impl fmt::Display for ViewportDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {} (revision={})",
            self.code.as_str(),
            self.detail,
            self.source_revision
        )
    }
}

impl std::error::Error for ViewportDiagnostic {}

//! Structured diagnostic taxonomy for the versioned command protocol.
//!
//! Each diagnostic carries a stable `code`, the offending argument, and the
//! protocol's current `schema_version`. The taxonomy starts with a single
//! entry point — `unknown_command` — that every adapter (CLI, TUI, MCP)
//! emits verbatim so callers can switch on `code` without parsing free-form
//! text.

use serde::Serialize;

/// The full set of diagnostic codes emitted by the versioned command
/// protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// The caller invoked a command id that is not registered in the
    /// static command registry. Emitted by the CLI dispatcher when the
    /// arg after `--machine` is not a known subcommand, and when `--machine`
    /// is supplied without a value.
    UnknownCommand,
    PersistenceFailure,
    IntegrityFailure,
    WorkerFailure,
    /// OCCT cannot apply the requested operation to the selected geometry.
    UnsupportedGeometry,
    /// The worker produced a BREP that fails `BRepCheck_Analyzer`. The
    /// host surfaces this to the caller without committing the revision.
    BrepInvalid,
    ThemePaletteInvalid,
    ArtifactPromotionFailure,
    ArtifactHashMismatch,
    ArtifactRevisionMismatch,
    ArtifactRequestMismatch,
    ArtifactCacheKeyMismatch,
    InvalidRequest,
    ReferenceAmbiguous,
    ReferenceLost,
    ReferenceIncompatible,
}

/// One structured diagnostic entry. The base JSON shape is fixed:
/// `{ "code": "<DiagnosticCode>", "arg": "<offending argument>", "schema_version": "threeterm.protocol/1" }`.
/// Palette startup diagnostics additionally carry their source, reason, and
/// recovery hint.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub arg: String,
    pub schema_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

impl Diagnostic {
    fn base(code: DiagnosticCode, arg: &str) -> Self {
        Self {
            code,
            arg: arg.to_string(),
            schema_version: crate::schema_version(),
            source: None,
            detail: None,
            recovery: None,
        }
    }

    pub fn unknown_command(arg: &str) -> Self {
        Self::base(DiagnosticCode::UnknownCommand, arg)
    }

    pub fn persistence_failure(detail: &str) -> Self {
        Self::base(DiagnosticCode::PersistenceFailure, detail)
    }

    pub fn integrity_failure(detail: &str) -> Self {
        Self::base(DiagnosticCode::IntegrityFailure, detail)
    }

    pub fn worker_failure(detail: &str) -> Self {
        Self::base(DiagnosticCode::WorkerFailure, detail)
    }

    pub fn unsupported_geometry(detail: &str) -> Self {
        Self::base(DiagnosticCode::UnsupportedGeometry, detail)
    }

    pub fn brep_invalid(detail: &str) -> Self {
        Self::base(DiagnosticCode::BrepInvalid, detail)
    }

    pub fn theme_palette_invalid(value: &str, source: &str, detail: &str, recovery: &str) -> Self {
        let mut diagnostic = Self::base(DiagnosticCode::ThemePaletteInvalid, value);
        diagnostic.source = Some(source.to_string());
        diagnostic.detail = Some(detail.to_string());
        diagnostic.recovery = Some(recovery.to_string());
        diagnostic
    }

    pub fn artifact_promotion_failure(detail: &str) -> Self {
        Self::base(DiagnosticCode::ArtifactPromotionFailure, detail)
    }

    pub fn artifact_hash_mismatch(expected: &str, actual: &str) -> Self {
        Self::base(
            DiagnosticCode::ArtifactHashMismatch,
            &format!("expected={expected};actual={actual}"),
        )
    }

    pub fn artifact_revision_mismatch(detail: &str) -> Self {
        Self::base(DiagnosticCode::ArtifactRevisionMismatch, detail)
    }

    pub fn artifact_request_mismatch(detail: &str) -> Self {
        Self::base(DiagnosticCode::ArtifactRequestMismatch, detail)
    }

    pub fn artifact_cache_key_mismatch(detail: &str) -> Self {
        Self::base(DiagnosticCode::ArtifactCacheKeyMismatch, detail)
    }

    pub fn invalid_request(detail: &str) -> Self {
        Self::base(DiagnosticCode::InvalidRequest, detail)
    }

    pub fn reference_ambiguous(detail: &str) -> Self {
        Self::base(DiagnosticCode::ReferenceAmbiguous, detail)
    }

    pub fn reference_lost(detail: &str) -> Self {
        Self::base(DiagnosticCode::ReferenceLost, detail)
    }

    pub fn reference_incompatible(detail: &str) -> Self {
        Self::base(DiagnosticCode::ReferenceIncompatible, detail)
    }
}

//! Structured diagnostic taxonomy for the versioned command protocol.
//!
//! Each diagnostic carries a stable `code`, the offending argument, and the
//! protocol's current `schema_version`. The taxonomy starts with a single
//! entry point — `unknown_command` — that every adapter (CLI, TUI, MCP)
//! emits verbatim so callers can switch on `code` without parsing free-form
//! text.

use serde::Serialize;

/// The full set of diagnostic codes emitted by the versioned command
/// protocol. This slice (#233) ships exactly one entry; later slices add
/// codes here as new failure modes are introduced.
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
    ArtifactPromotionFailure,
    ArtifactHashMismatch,
    ArtifactRevisionMismatch,
    ArtifactRequestMismatch,
    ArtifactCacheKeyMismatch,
}

/// One structured diagnostic entry. The JSON shape is fixed:
/// `{ "code": "<DiagnosticCode>", "arg": "<offending argument>", "schema_version": "threeterm.protocol/1" }`.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub arg: String,
    pub schema_version: &'static str,
}

impl Diagnostic {
    pub fn unknown_command(arg: &str) -> Self {
        Self {
            code: DiagnosticCode::UnknownCommand,
            arg: arg.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn persistence_failure(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::PersistenceFailure,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn integrity_failure(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::IntegrityFailure,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn worker_failure(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::WorkerFailure,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn unsupported_geometry(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::UnsupportedGeometry,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn brep_invalid(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::BrepInvalid,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn artifact_promotion_failure(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::ArtifactPromotionFailure,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn artifact_hash_mismatch(expected: &str, actual: &str) -> Self {
        Self {
            code: DiagnosticCode::ArtifactHashMismatch,
            arg: format!("expected={expected};actual={actual}"),
            schema_version: crate::schema_version(),
        }
    }

    pub fn artifact_revision_mismatch(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::ArtifactRevisionMismatch,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn artifact_request_mismatch(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::ArtifactRequestMismatch,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn artifact_cache_key_mismatch(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::ArtifactCacheKeyMismatch,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }
}

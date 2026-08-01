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
    /// The persistence layer rejected the operation: the manifest seal
    /// mismatch, the bundle cannot be staged, the transaction replay is
    /// inconsistent, or the on-disk state would be corrupted by publishing.
    PersistenceFailure,
    /// The request body failed schema validation or domain request
    /// preflight. Emitted when a command's JSON request is malformed or
    /// carries structurally invalid data (closed issue #23: every
    /// reattachment diagnostic carries a stable code callers can switch on).
    InvalidRequest,
    /// The reattachment policy (`threeterm.reference.semantic`) returned
    /// `Ambiguous`: more than one candidate satisfied the reference
    /// predicates and the host cannot choose silently (closed issue #23).
    /// Emitted when a definition, instance, or semantic reference resolves
    /// to multiple matches.
    ReferenceAmbiguous,
    /// The reattachment policy returned `Lost`: zero candidates satisfied
    /// the reference predicates. The requested id or descriptor does not
    /// resolve anywhere in the canonical graph (closed issue #23).
    ReferenceLost,
    /// The reattachment policy returned `Incompatible`: the reference's
    /// schema version or expected feature kind does not match any
    /// candidate. The canonical graph was preserved (closed issue #23).
    ReferenceIncompatible,
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

    pub fn invalid_request(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::InvalidRequest,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn reference_ambiguous(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::ReferenceAmbiguous,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn reference_lost(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::ReferenceLost,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    pub fn reference_incompatible(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::ReferenceIncompatible,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }
}

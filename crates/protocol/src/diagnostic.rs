//! Structured diagnostic taxonomy for the versioned command protocol.
//!
//! Each diagnostic carries a stable `code`, the offending argument, and the
//! protocol's current `schema_version`. The taxonomy starts with a single
//! entry point — `unknown_command` — that every adapter (CLI, TUI, MCP)
//! emits verbatim so callers can switch on `code` without parsing free-form
//! text. Slice (#235) adds `integrity_failure` for the save / load
//! round-trip.

use serde::Serialize;

/// The full set of diagnostic codes emitted by the versioned command
/// protocol. This slice (#233) ships `unknown_command`; slice (#235)
/// adds `integrity_failure`. Later slices add codes here as new failure
/// modes are introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// The caller invoked a command id that is not registered in the
    /// static command registry. Emitted by the CLI dispatcher when the
    /// arg after `--machine` is not a known subcommand, and when `--machine`
    /// is supplied without a value.
    UnknownCommand,
    /// The save / load round-trip detected a persistence-layer integrity
    /// violation: missing manifest, missing log, broken chain link,
    /// manifest-declared digest mismatch with the recomputed chain, or
    /// an unsupported schema generation.
    IntegrityFailure,
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
    /// Emit a structured diagnostic for a command id that is not registered
    /// in the static command registry.
    pub fn unknown_command(arg: &str) -> Self {
        Self {
            code: DiagnosticCode::UnknownCommand,
            arg: arg.to_string(),
            schema_version: crate::schema_version(),
        }
    }

    /// Emit a structured diagnostic for a persistence-layer integrity
    /// failure. `detail` is a stable lowercase string the CLI uses to
    /// switch on the failure mode (e.g. `"log_digest_mismatch"`).
    pub fn integrity_failure(detail: &str) -> Self {
        Self {
            code: DiagnosticCode::IntegrityFailure,
            arg: detail.to_string(),
            schema_version: crate::schema_version(),
        }
    }
}

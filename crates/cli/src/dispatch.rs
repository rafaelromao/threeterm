use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use serde_json::Value;
use threeterm_domain::ProjectGeneration;
use threeterm_host::{Host, HostError, SnapshotView};
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::schema::iter;
pub use threeterm_protocol::schema::{
    BRACKET_RESPONSE_SCHEMA_VERSION, LOAD_RESPONSE_SCHEMA_VERSION, SAVE_RESPONSE_SCHEMA_VERSION,
};

pub const EXIT_OK: i32 = 0;
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;
pub const EXIT_INTEGRITY_FAILURE: i32 = 2;
pub const EXIT_PERSISTENCE_FAILURE: i32 = 3;

#[derive(Debug, Clone, PartialEq)]
enum DispatchPlan {
    List,
    NewProject {
        path: String,
    },
    Save {
        bundle: String,
        feature_id: String,
        kind: String,
    },
    Load {
        bundle: String,
    },
    Bracket {
        bundle: String,
        bracket_id: String,
        length: f64,
        width: f64,
        height: f64,
        thickness: f64,
    },
    Unknown {
        arg: String,
    },
}

fn plan(args: &[OsString]) -> DispatchPlan {
    if args.first().is_some_and(|value| value == "new-project") {
        return match args {
            [_, path] => DispatchPlan::NewProject {
                path: path.to_string_lossy().into_owned(),
            },
            [_, other, ..] => DispatchPlan::Unknown {
                arg: other.to_string_lossy().into_owned(),
            },
            _ => DispatchPlan::Unknown {
                arg: "new-project".to_string(),
            },
        };
    }
    if args.first().is_none_or(|value| value != "--machine") {
        return DispatchPlan::Unknown {
            arg: args
                .first()
                .map_or_else(String::new, |value| value.to_string_lossy().into_owned()),
        };
    }
    let Some(command) = args.get(1).and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "--machine".to_string(),
        };
    };
    match command {
        "list" if args.len() == 2 => DispatchPlan::List,
        "new-project" if args.len() == 3 => DispatchPlan::NewProject {
            path: args[2].to_string_lossy().into_owned(),
        },
        "save" => parse_save(&args[2..]),
        "load" => parse_load(&args[2..]),
        "bracket" => parse_bracket(&args[2..]),
        _ => DispatchPlan::Unknown {
            arg: command.to_string(),
        },
    }
}

fn parse_save(args: &[OsString]) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "save".to_string(),
        };
    };
    if bundle.starts_with("--") {
        return DispatchPlan::Unknown {
            arg: bundle.to_string(),
        };
    }

    let mut feature_id = None;
    let mut kind = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        let Some(value) = args.get(index + 1) else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        match flag.as_ref() {
            "--feature-id" => feature_id = Some(value.to_string_lossy().into_owned()),
            "--kind" => kind = Some(value.to_string_lossy().into_owned()),
            _ => {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            }
        }
        index += 2;
    }

    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(kind) = kind else {
        return DispatchPlan::Unknown {
            arg: "--kind".to_string(),
        };
    };
    DispatchPlan::Save {
        bundle: bundle.to_string(),
        feature_id,
        kind,
    }
}

fn parse_load(args: &[OsString]) -> DispatchPlan {
    match args {
        [bundle] if !bundle.to_string_lossy().starts_with("--") => DispatchPlan::Load {
            bundle: bundle.to_string_lossy().into_owned(),
        },
        [argument, ..] => DispatchPlan::Unknown {
            arg: argument.to_string_lossy().into_owned(),
        },
        [] => DispatchPlan::Unknown {
            arg: "load".to_string(),
        },
    }
}

fn parse_bracket(args: &[OsString]) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "bracket".to_string(),
        };
    };
    if bundle.starts_with("--") {
        return DispatchPlan::Unknown {
            arg: bundle.to_string(),
        };
    }

    let mut bracket_id = None;
    let mut length = None;
    let mut width = None;
    let mut height = None;
    let mut thickness = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        let Some(value) = args.get(index + 1) else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        match flag.as_ref() {
            "--bracket-id" => bracket_id = Some(value.to_string_lossy().into_owned()),
            "--length" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => length = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            "--width" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => width = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            "--height" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => height = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            "--thickness" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => thickness = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            _ => {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            }
        }
        index += 2;
    }

    let Some(bracket_id) = bracket_id else {
        return DispatchPlan::Unknown {
            arg: "--bracket-id".to_string(),
        };
    };
    let Some(length) = length else {
        return DispatchPlan::Unknown {
            arg: "--length".to_string(),
        };
    };
    let Some(width) = width else {
        return DispatchPlan::Unknown {
            arg: "--width".to_string(),
        };
    };
    let Some(height) = height else {
        return DispatchPlan::Unknown {
            arg: "--height".to_string(),
        };
    };
    let Some(thickness) = thickness else {
        return DispatchPlan::Unknown {
            arg: "--thickness".to_string(),
        };
    };
    DispatchPlan::Bracket {
        bundle: bundle.to_string(),
        bracket_id,
        length,
        width,
        height,
        thickness,
    }
}

fn parse_dimension(value: &str) -> Option<f64> {
    value.parse::<f64>().ok()
}

pub fn dispatch<I>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    match plan(&args) {
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::NewProject { path } => emit_new_project(&path, stdout, stderr),
        DispatchPlan::Save {
            bundle,
            feature_id,
            kind,
        } => emit_save(&bundle, &feature_id, &kind, stdout, stderr),
        DispatchPlan::Load { bundle } => emit_load(&bundle, stdout, stderr),
        DispatchPlan::Bracket {
            bundle,
            bracket_id,
            length,
            width,
            height,
            thickness,
        } => emit_bracket(
            &bundle,
            &bracket_id,
            length,
            width,
            height,
            thickness,
            stdout,
            stderr,
        ),
        DispatchPlan::Unknown { arg } => emit_unknown_command(&arg, stderr),
    }
}

/// Pure dispatcher entry point shared between the CLI and the MCP adapter.
///
/// Both transports call this same function so the only difference between
/// CLI and MCP is framing, parsing, and serialization — not the dispatch
/// logic. On success the post-write `SnapshotView` is returned; on failure
/// a structured `DispatchError` carries the same diagnostic detail the
/// CLI would have written to stderr.
pub fn dispatch_bracket(
    bundle: &str,
    bracket_id: &str,
    length: f64,
    width: f64,
    height: f64,
    thickness: f64,
) -> Result<SnapshotView, DispatchError> {
    Host::new()
        .save_bracket(bundle, bracket_id, length, width, height, thickness)
        .map_err(DispatchError::from)
}

/// Structured failure modes emitted by the shared CLI/MCP dispatcher. The
/// CLI renders these as JSON diagnostics on stderr; the MCP server
/// converts them to JSON-RPC error envelopes.
#[derive(Debug)]
pub enum DispatchError {
    Host(HostError),
    Validation(String),
}

impl From<HostError> for DispatchError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

impl DispatchError {
    pub fn diagnostic_detail(&self) -> String {
        match self {
            Self::Host(error) => match error {
                HostError::BundlePathMissing { .. } => "bundle_path_missing".to_string(),
                HostError::BundlePathNotDirectory { .. } => "bundle_path_not_directory".to_string(),
                HostError::Validation { detail } => format!("host_validation: {detail}"),
                HostError::Persistence(error) => error.diagnostic_detail().to_string(),
            },
            Self::Validation(detail) => format!("dispatch_validation: {detail}"),
        }
    }
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(error) => write!(formatter, "{error}"),
            Self::Validation(detail) => write!(formatter, "dispatch.validation: {detail}"),
        }
    }
}

impl std::error::Error for DispatchError {}

fn emit_listing(stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let entries: Vec<&_> = iter().collect();
    let serialized = match serde_json::to_value(&entries) {
        Ok(Value::Array(items)) => Value::Array(items),
        Ok(other) => {
            return emit_internal_error(
                &format!("expected the registry to serialize as an array, got {other:?}"),
                stderr,
            );
        }
        Err(error) => {
            return emit_internal_error(&format!("registry serialization failed: {error}"), stderr);
        }
    };
    write_success(stdout, &serialized, stderr)
}

fn emit_new_project(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    let generation = ProjectGeneration::fresh();
    match threeterm_persistence::write_fresh(Path::new(path), generation) {
        Ok(manifest) => {
            // The Project Generation identity is the canonical log
            // digest; surface the manifest's identity, not the caller's
            // seed value.
            let generation_id = manifest.generation_id.clone();
            write_success(
                stdout,
                &serde_json::json!({
                    "generation_id": generation_id,
                    "manifest": manifest,
                }),
                stderr,
            )
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_save(
    bundle: &str,
    feature_id: &str,
    kind: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match Host::new().save(bundle, feature_id, kind) {
        Ok(view) => write_snapshot(
            &view.feature_graph_hash,
            &view.revision_hash,
            SAVE_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => emit_host_error(&error, stderr),
    }
}

fn emit_load(bundle: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    match Host::new().load(bundle) {
        Ok(view) => write_snapshot(
            &view.feature_graph_hash,
            &view.revision_hash,
            LOAD_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_bracket(
    bundle: &str,
    bracket_id: &str,
    length: f64,
    width: f64,
    height: f64,
    thickness: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match dispatch_bracket(bundle, bracket_id, length, width, height, thickness) {
        Ok(view) => write_snapshot(
            &view.feature_graph_hash,
            &view.revision_hash,
            BRACKET_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => match error {
            DispatchError::Host(host_error) => emit_host_error(&host_error, stderr),
            DispatchError::Validation(detail) => {
                emit_internal_error(&format!("bracket validation: {detail}"), stderr)
            }
        },
    }
}

fn write_snapshot(
    feature_graph_hash: &str,
    revision_hash: &str,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "feature_graph_hash": feature_graph_hash,
            "revision_hash": revision_hash,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_success(stdout: &mut dyn Write, value: &Value, stderr: &mut dyn Write) -> i32 {
    match serde_json::to_writer_pretty(&mut *stdout, value) {
        Ok(()) => {
            let _ = writeln!(stdout);
            EXIT_OK
        }
        Err(error) => emit_internal_error(&format!("response write failed: {error}"), stderr),
    }
}

fn emit_host_error(error: &HostError, stderr: &mut dyn Write) -> i32 {
    let detail = match error {
        HostError::BundlePathMissing { .. } => "bundle_path_missing",
        HostError::BundlePathNotDirectory { .. } => "bundle_path_not_directory",
        HostError::Validation { detail } => detail,
        HostError::Persistence(error) => error.diagnostic_detail(),
    };
    let diagnostic = Diagnostic::integrity_failure(detail);
    write_diagnostic(stderr, &diagnostic);
    EXIT_INTEGRITY_FAILURE
}

fn emit_persistence_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    write_diagnostic(stderr, &Diagnostic::persistence_failure(detail));
    EXIT_PERSISTENCE_FAILURE
}

fn emit_unknown_command(arg: &str, stderr: &mut dyn Write) -> i32 {
    write_diagnostic(stderr, &Diagnostic::unknown_command(arg));
    EXIT_UNKNOWN_COMMAND
}

fn emit_internal_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    write_diagnostic(stderr, &Diagnostic::unknown_command(detail));
    EXIT_UNKNOWN_COMMAND
}

fn write_diagnostic(stderr: &mut dyn Write, diagnostic: &Diagnostic) {
    match serde_json::to_writer_pretty(&mut *stderr, diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn dispatch_machine_list_writes_top_level_json_array_to_stdout() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_OK);
        assert!(stderr.is_empty());
        let parsed: Value = serde_json::from_slice(&stdout).expect("listing is JSON");
        let commands = parsed.as_array().expect("listing is an array");
        assert_eq!(commands.len(), 5);
        let list = commands
            .iter()
            .find(|command| command["id"] == "list")
            .expect("list is registered");
        assert_eq!(list["schema_version"], "threeterm.command.list/1");
    }

    #[test]
    fn dispatch_machine_unknown_writes_diagnostic_to_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        assert!(stdout.is_empty());
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "bogus");
    }

    #[test]
    fn dispatch_machine_without_value_writes_diagnostic_with_arg_machine() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["arg"], "--machine");
    }

    #[test]
    fn dispatch_without_machine_flag_writes_diagnostic_with_first_arg() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--bogus"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["arg"], "--bogus");
    }

    #[test]
    fn dispatch_with_no_args_writes_diagnostic_with_empty_arg() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&[]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["arg"], "");
    }

    #[test]
    fn dispatch_rejects_missing_save_and_load_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "save"], "save"),
            (
                vec![
                    "--machine",
                    "save",
                    "--feature-id",
                    "box-1",
                    "--kind",
                    "box",
                ],
                "--feature-id",
            ),
            (vec!["--machine", "save", "bundle"], "--feature-id"),
            (
                vec!["--machine", "save", "bundle", "--feature-id", "box-1"],
                "--kind",
            ),
            (vec!["--machine", "load"], "load"),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            assert!(stdout.is_empty());
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_does_not_call_exit_or_panic() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let _ = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&[]), &mut stdout, &mut stderr);
    }
}

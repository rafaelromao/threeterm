//! CLI command dispatcher.
//!
//! `dispatch` parses an argv slice, routes the recognized `--machine
//! <subcommand>` grammar, and writes either the JSON listing to stdout or
//! a structured `unknown_command` / `integrity_failure` diagnostic to
//! stderr. The dispatcher owns no global state and never calls
//! `std::process::exit`: the binary wraps it and propagates the returned
//! exit code.
//!
//! Three `--machine` subcommands are recognised:
//!
//! - `--machine list` — emit the registry as a top-level JSON array.
//! - `--machine save <bundle> --feature-id <id> --kind <kind>` — append a
//!   feature to the bundle, atomically re-seal the manifest, print the
//!   snapshot JSON on stdout.
//! - `--machine load <bundle>` — integrity-verify the bundle, print the
//!   snapshot JSON on stdout.
//!
//! `save` and `load` share the same response shape
//! (`{ feature_graph_hash, revision_hash, schema_version }`); the
//! `schema_version` field carries the response's own version pin.

use std::ffi::OsString;
use std::io::Write;

use serde_json::Value;
use threeterm_host::{Host, HostError};
use threeterm_persistence::BundleError;
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::schema::iter;

/// Exit code returned when a `--machine` subcommand is recognized and the
/// JSON listing is emitted to stdout.
pub const EXIT_OK: i32 = 0;

/// Exit code returned when the dispatcher rejects the argv (unknown
/// subcommand, missing value, or no `--machine` flag). Callers can
/// switch on `Diagnostic.code` for the structured detail.
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;

/// Exit code returned when the host surfaces an integrity failure
/// (sealed manifest missing, log missing, log digest mismatch, chain
/// link broken, schema generation unsupported, bundle path missing).
/// Distinct from `EXIT_UNKNOWN_COMMAND` so shell-level callers can
/// short-circuit without parsing the diagnostic envelope.
pub const EXIT_INTEGRITY_FAILURE: i32 = 3;

/// Stable schema pins for the response JSON shape produced by save /
/// load. They must mirror the values registered in
/// `protocol::schema::COMMAND_REGISTRY`.
pub const SAVE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.save.response/1";
pub const LOAD_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.load.response/1";

#[derive(Debug, Clone, PartialEq, Eq)]
enum DispatchPlan {
    List,
    Save {
        bundle: String,
        feature_id: String,
        kind: String,
    },
    Load {
        bundle: String,
    },
    Unknown {
        arg: String,
    },
}

/// Inspect the argv slice and decide which dispatch plan to execute.
///
/// The grammar is:
/// - `["--machine", "list"]` -> `DispatchPlan::List`
/// - `["--machine", "save", <bundle>, "--feature-id", <id>, "--kind", <kind>]` (flags in any order after the positional) -> `DispatchPlan::Save { bundle, feature_id, kind }`
/// - `["--machine", "load", <bundle>]` -> `DispatchPlan::Load { bundle }`
/// - Anything else routes to `DispatchPlan::Unknown { arg: <offending arg> }`.
fn plan(args: &[OsString]) -> DispatchPlan {
    if args.len() < 2 || args[0] != "--machine" {
        let arg = args.first().map_or("", |s| s.to_str().unwrap_or(""));
        return DispatchPlan::Unknown {
            arg: arg.to_string(),
        };
    }

    let subcommand = args[1].to_str().unwrap_or("");

    match subcommand {
        "list" => {
            if args.len() != 2 {
                return DispatchPlan::Unknown {
                    arg: args
                        .get(2)
                        .map(|s| s.to_str().unwrap_or("").to_string())
                        .unwrap_or_default(),
                };
            }
            DispatchPlan::List
        }
        "save" => parse_save(&args[2..]),
        "load" => parse_load(&args[2..]),
        _ => DispatchPlan::Unknown {
            arg: subcommand.to_string(),
        },
    }
}

fn parse_save(rest: &[OsString]) -> DispatchPlan {
    if rest.is_empty() {
        return DispatchPlan::Unknown {
            arg: "save".to_string(),
        };
    }
    let bundle = rest[0].to_str().unwrap_or("").to_string();
    if bundle.starts_with("--") {
        return DispatchPlan::Unknown {
            arg: bundle.clone(),
        };
    }

    let mut feature_id: Option<String> = None;
    let mut kind: Option<String> = None;
    let mut i = 1;
    while i < rest.len() {
        let flag = rest[i].to_str().unwrap_or("");
        let Some(value) = rest
            .get(i + 1)
            .map(|s| s.to_str().unwrap_or("").to_string())
        else {
            return DispatchPlan::Unknown {
                arg: flag.to_string(),
            };
        };
        match flag {
            "--feature-id" => feature_id = Some(value),
            "--kind" => kind = Some(value),
            _ => {
                return DispatchPlan::Unknown {
                    arg: flag.to_string(),
                };
            }
        }
        i += 2;
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
        bundle,
        feature_id,
        kind,
    }
}

fn parse_load(rest: &[OsString]) -> DispatchPlan {
    if rest.is_empty() {
        return DispatchPlan::Unknown {
            arg: "load".to_string(),
        };
    }
    let bundle = rest[0].to_str().unwrap_or("").to_string();
    if bundle.starts_with("--") {
        return DispatchPlan::Unknown {
            arg: bundle.clone(),
        };
    }
    if rest.len() > 1 {
        return DispatchPlan::Unknown {
            arg: rest[1].to_str().unwrap_or("").to_string(),
        };
    }
    DispatchPlan::Load { bundle }
}

/// Dispatch the argv slice. Writes either the JSON listing to `stdout`
/// or a structured diagnostic to `stderr`, and returns the exit code.
pub fn dispatch<I>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let collected: Vec<OsString> = args.into_iter().collect();
    let plan = plan(&collected);

    match plan {
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::Save {
            bundle,
            feature_id,
            kind,
        } => emit_save(&bundle, &feature_id, &kind, stdout, stderr),
        DispatchPlan::Load { bundle } => emit_load(&bundle, stdout, stderr),
        DispatchPlan::Unknown { arg } => emit_unknown_command(&arg, stderr),
    }
}

fn emit_listing(stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let entries: Vec<&_> = iter().collect();

    let serialized = match serde_json::to_value(&entries) {
        Ok(value) => value,
        Err(error) => {
            return emit_internal_error(&format!("registry serialization failed: {error}"), stderr);
        }
    };

    let array: Vec<Value> = match serialized {
        Value::Array(items) => items,
        other => {
            return emit_internal_error(
                &format!("expected the registry to serialize as an array, got {other:?}"),
                stderr,
            );
        }
    };

    match serde_json::to_writer_pretty(&mut *stdout, &Value::Array(array)) {
        Ok(()) => {
            let _ = writeln!(stdout);
            EXIT_OK
        }
        Err(error) => emit_internal_error(&format!("listing write failed: {error}"), stderr),
    }
}

fn emit_save(
    bundle: &str,
    feature_id: &str,
    kind: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let host = Host::new();
    match host.save(bundle, feature_id, kind) {
        Ok(view) => write_snapshot(
            &view.feature_graph_hash_hex,
            &view.revision_hash_hex,
            SAVE_RESPONSE_SCHEMA_VERSION,
            stdout,
        ),
        Err(err) => emit_host_error(&err, stderr),
    }
}

fn emit_load(bundle: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let host = Host::new();
    match host.load(bundle) {
        Ok(view) => write_snapshot(
            &view.feature_graph_hash_hex,
            &view.revision_hash_hex,
            LOAD_RESPONSE_SCHEMA_VERSION,
            stdout,
        ),
        Err(err) => emit_host_error(&err, stderr),
    }
}

fn write_snapshot(
    feature_graph_hash: &str,
    revision_hash: &str,
    schema_version: &str,
    stdout: &mut dyn Write,
) -> i32 {
    let payload = serde_json::json!({
        "feature_graph_hash": feature_graph_hash,
        "revision_hash": revision_hash,
        "schema_version": schema_version,
    });
    let serialize_result = serde_json::to_writer_pretty(&mut *stdout, &payload);
    let _ = writeln!(stdout);
    match serialize_result {
        Ok(()) => EXIT_OK,
        Err(_) => EXIT_INTEGRITY_FAILURE,
    }
}

fn emit_host_error(err: &HostError, stderr: &mut dyn Write) -> i32 {
    let detail = match err {
        HostError::BundlePathMissing { .. } => "bundle_path_missing",
        HostError::Persistence(BundleError::ManifestMissing) => "manifest_missing",
        HostError::Persistence(BundleError::LogMissing) => "log_missing",
        HostError::Persistence(BundleError::LogDigestMismatch) => "log_digest_mismatch",
        HostError::Persistence(BundleError::SchemaGenerationUnsupported { .. }) => {
            "schema_generation_unsupported"
        }
        HostError::Persistence(BundleError::LogFailure(
            threeterm_persistence::log::LogError::LogBrokenLink { .. },
        )) => "log_broken_link",
        HostError::Persistence(BundleError::LogFailure(
            threeterm_persistence::log::LogError::LogMissing,
        )) => "log_missing",
        HostError::Persistence(BundleError::LogFailure(
            threeterm_persistence::log::LogError::Malformed { .. },
        )) => "log_malformed",
        HostError::Persistence(BundleError::LogFailure(
            threeterm_persistence::log::LogError::Io { .. },
        )) => "log_io_failure",
        HostError::Persistence(BundleError::IoFailure { .. }) => "bundle_io_failure",
        HostError::Persistence(BundleError::ProjectGenerationUnavailable { .. }) => {
            "project_generation_unavailable"
        }
    };
    emit_integrity_failure(detail, stderr);
    EXIT_INTEGRITY_FAILURE
}

fn emit_integrity_failure(detail: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::integrity_failure(detail);
    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
    EXIT_INTEGRITY_FAILURE
}

fn emit_unknown_command(arg: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::unknown_command(arg);
    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
    EXIT_UNKNOWN_COMMAND
}

fn emit_internal_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::unknown_command(detail);
    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
    EXIT_UNKNOWN_COMMAND
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn dispatch_machine_list_writes_top_level_json_array_to_stdout() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_OK);
        assert!(stderr.is_empty(), "stderr must be empty on success");

        let stdout_text = std::str::from_utf8(&stdout).expect("stdout is utf-8");
        let parsed: Value =
            serde_json::from_str(stdout_text).expect("dispatch output is parseable JSON");

        let commands = parsed
            .as_array()
            .expect("dispatch output is a top-level JSON array");

        let list_entry = commands
            .iter()
            .find(|c| c["id"] == "list")
            .expect("`list` is registered");
        assert_eq!(list_entry["name"], "list");
        assert_eq!(list_entry["schema_version"], "threeterm.command.list/1");
        assert_eq!(
            list_entry["request_schema_version"],
            "threeterm.command.list.request/1"
        );
        assert_eq!(
            list_entry["response_schema_version"],
            "threeterm.command.list.response/1"
        );
        assert!(list_entry["request_schema"].is_object());
        assert!(list_entry["response_schema"].is_object());
    }

    #[test]
    fn dispatch_machine_unknown_writes_diagnostic_to_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        assert!(stdout.is_empty(), "stdout must be empty on diagnostic");

        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");

        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "bogus");
        assert_eq!(
            parsed["schema_version"],
            Value::from(threeterm_protocol::schema_version())
        );
    }

    #[test]
    fn dispatch_machine_without_value_writes_diagnostic_with_arg_machine() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        assert!(stdout.is_empty(), "stdout must be empty on diagnostic");

        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");

        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--machine");
    }

    #[test]
    fn dispatch_without_machine_flag_writes_diagnostic_with_first_arg() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--bogus"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        assert!(stdout.is_empty(), "stdout must be empty on diagnostic");

        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");

        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--bogus");
    }

    #[test]
    fn dispatch_with_no_args_writes_diagnostic_with_empty_arg() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&[]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);

        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");

        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "");
    }

    #[test]
    fn dispatch_does_not_call_exit_or_panic() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let _ = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&[]), &mut stdout, &mut stderr);
    }

    #[test]
    fn dispatch_save_missing_bundle_reports_unknown_command() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "save",
                "--feature-id",
                "box-1",
                "--kind",
                "box",
            ]),
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");
        assert_eq!(parsed["code"], "unknown_command");
    }

    #[test]
    fn dispatch_save_missing_kind_reports_unknown_command() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "save",
                "/tmp/opencode/threeterm-dispatcher-test-missing-kind",
                "--feature-id",
                "box-1",
            ]),
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--kind");
    }

    #[test]
    fn dispatch_load_missing_bundle_reports_unknown_command() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "load"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "load");
    }
}

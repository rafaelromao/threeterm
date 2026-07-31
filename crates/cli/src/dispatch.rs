//! CLI command dispatcher.
//!
//! `dispatch` parses an argv slice, routes the recognized subcommand
//! grammar (every command is a positional verb: `list`, `new-project`,
//! `identity`, `load`, `apply`), and writes either JSON to stdout or a
//! structured diagnostic to stderr. The dispatcher owns no global state
//! and never calls `std::process::exit`: the binary wraps it and
//! propagates the returned exit code.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use serde_json::Value;
use threeterm_domain::ProjectGeneration;
use threeterm_persistence::{TransactionIntent, append, current_identity, load, write_fresh};
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::schema::iter;

/// Exit code returned when a subcommand is recognized and the JSON
/// listing is emitted to stdout.
pub const EXIT_OK: i32 = 0;

/// Exit code returned when the dispatcher rejects the argv (unknown
/// subcommand, missing value, or no positional verb).
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;
pub const EXIT_PERSISTENCE_FAILURE: i32 = 3;
pub const EXIT_COMMAND_REJECTED: i32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
enum DispatchPlan {
    List,
    NewProject { path: String },
    Identity { path: String },
    Load { path: String },
    Apply { path: String, intent_json: String },
    Unknown { arg: String },
}

/// Inspect the argv slice and decide which dispatch plan to execute.
///
/// The grammar is one positional verb followed by arguments:
/// - `list` -> `List`
/// - `new-project <path>` -> `NewProject`
/// - `identity <path>` -> `Identity`
/// - `load <path>` -> `Load`
/// - `apply <path> <intent-json>` -> `Apply`
/// - otherwise -> `Unknown`
fn plan(args: &[OsString]) -> DispatchPlan {
    let values: Vec<&OsString> = args.iter().collect();
    match values.as_slice() {
        [verb] if os_str(verb) == "list" => DispatchPlan::List,
        [verb, path] if os_str(verb) == "new-project" => DispatchPlan::NewProject {
            path: path.to_string_lossy().into_owned(),
        },
        [verb, path] if os_str(verb) == "identity" => DispatchPlan::Identity {
            path: path.to_string_lossy().into_owned(),
        },
        [verb, path] if os_str(verb) == "load" => DispatchPlan::Load {
            path: path.to_string_lossy().into_owned(),
        },
        [verb, path, intent] if os_str(verb) == "apply" => DispatchPlan::Apply {
            path: path.to_string_lossy().into_owned(),
            intent_json: intent.to_string_lossy().into_owned(),
        },
        [first, ..] => DispatchPlan::Unknown {
            arg: first.to_string_lossy().into_owned(),
        },
        [] => DispatchPlan::Unknown { arg: String::new() },
    }
}

fn os_str(value: &OsString) -> String {
    value.to_string_lossy().into_owned()
}

/// Dispatch the argv slice. Writes either JSON to `stdout` or a
/// structured diagnostic to `stderr`, and returns the exit code.
pub fn dispatch<I>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let collected: Vec<OsString> = args.into_iter().collect();
    let plan = plan(&collected);

    match plan {
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::NewProject { path } => emit_new_project(&path, stdout, stderr),
        DispatchPlan::Identity { path } => emit_identity(&path, stdout, stderr),
        DispatchPlan::Load { path } => emit_load(&path, stdout, stderr),
        DispatchPlan::Apply { path, intent_json } => {
            emit_apply(&path, &intent_json, stdout, stderr)
        }
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

fn emit_new_project(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    let generation = ProjectGeneration::fresh();
    match write_fresh(Path::new(path), generation.clone()) {
        Ok(manifest) => {
            let response = serde_json::json!({
                "generation_id": generation.id,
                "manifest": manifest,
            });
            match serde_json::to_writer_pretty(&mut *stdout, &response) {
                Ok(()) => {
                    let _ = writeln!(stdout);
                    EXIT_OK
                }
                Err(error) => {
                    emit_internal_error(&format!("response write failed: {error}"), stderr)
                }
            }
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_identity(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    match current_identity(Path::new(path)) {
        Ok(identity) => {
            let loaded = load(Path::new(path)).expect("identity was just read");
            let payload = serde_json::json!({
                "identity": identity.0,
                "transaction_count": loaded.manifest.transaction_count,
                "schema_version": threeterm_persistence::schema_version(),
            });
            match serde_json::to_writer_pretty(&mut *stdout, &payload) {
                Ok(()) => {
                    let _ = writeln!(stdout);
                    EXIT_OK
                }
                Err(error) => {
                    emit_internal_error(&format!("identity write failed: {error}"), stderr)
                }
            }
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_load(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    match load(Path::new(path)) {
        Ok(loaded) => {
            let payload = serde_json::json!({
                "manifest": loaded.manifest,
                "generation": loaded.generation,
                "transaction_count": loaded.manifest.transaction_count,
                "schema_version": threeterm_persistence::schema_version(),
            });
            match serde_json::to_writer_pretty(&mut *stdout, &payload) {
                Ok(()) => {
                    let _ = writeln!(stdout);
                    EXIT_OK
                }
                Err(error) => emit_internal_error(&format!("load write failed: {error}"), stderr),
            }
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_apply(
    path: &str,
    intent_json: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    let intent: TransactionIntent = match serde_json::from_str(intent_json) {
        Ok(value) => value,
        Err(error) => {
            return emit_command_rejected(&format!("invalid intent JSON: {error}"), stderr);
        }
    };
    let identity = match current_identity(Path::new(path)) {
        Ok(identity) => identity,
        Err(error) => return emit_persistence_error(&error.to_string(), stderr),
    };
    match append(Path::new(path), &identity, &intent) {
        Ok(result) => {
            let payload = serde_json::json!({
                "identity": result.manifest.transaction_sha256,
                "transaction_count": result.manifest.transaction_count,
                "schema_version": threeterm_persistence::schema_version(),
            });
            match serde_json::to_writer_pretty(&mut *stdout, &payload) {
                Ok(()) => {
                    let _ = writeln!(stdout);
                    EXIT_OK
                }
                Err(error) => emit_internal_error(&format!("apply write failed: {error}"), stderr),
            }
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_persistence_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::persistence_failure(detail);
    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
    EXIT_PERSISTENCE_FAILURE
}

fn emit_command_rejected(detail: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::persistence_failure(detail);
    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
    EXIT_COMMAND_REJECTED
}

fn emit_unknown_command(arg: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::unknown_command(arg);

    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
            EXIT_UNKNOWN_COMMAND
        }
        Err(error) => emit_internal_error(&format!("diagnostic write failed: {error}"), stderr),
    }
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
    fn dispatch_list_writes_top_level_json_array_to_stdout() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["list"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_OK);
        assert!(stderr.is_empty(), "stderr must be empty on success");

        let stdout_text = std::str::from_utf8(&stdout).expect("stdout is utf-8");
        let parsed: Value =
            serde_json::from_str(stdout_text).expect("dispatch output is parseable JSON");

        let commands = parsed
            .as_array()
            .expect("dispatch output is a top-level JSON array");

        assert_eq!(commands.len(), 5, "five registered commands");
        let ids: Vec<&str> = commands
            .iter()
            .map(|c| c["id"].as_str().unwrap_or(""))
            .collect();
        assert!(ids.contains(&"list"));
        assert!(ids.contains(&"new-project"));
        assert!(ids.contains(&"identity"));
        assert!(ids.contains(&"load"));
        assert!(ids.contains(&"apply"));
    }

    #[test]
    fn dispatch_unknown_writes_diagnostic_to_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["bogus"]), &mut stdout, &mut stderr);

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
    fn dispatch_no_args_writes_diagnostic_with_empty_arg() {
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
        let _ = dispatch(args(&["list"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&["bogus"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&[]), &mut stdout, &mut stderr);
    }

    #[test]
    fn plan_recognizes_each_verb() {
        assert_eq!(plan(&args(&["list"])), DispatchPlan::List);
        assert_eq!(
            plan(&args(&["new-project", "/tmp/foo"])),
            DispatchPlan::NewProject {
                path: "/tmp/foo".to_string()
            }
        );
        assert_eq!(
            plan(&args(&["identity", "/tmp/foo"])),
            DispatchPlan::Identity {
                path: "/tmp/foo".to_string()
            }
        );
        assert_eq!(
            plan(&args(&["load", "/tmp/foo"])),
            DispatchPlan::Load {
                path: "/tmp/foo".to_string()
            }
        );
        assert_eq!(
            plan(&args(&["apply", "/tmp/foo", "{\"kind\":\"add-feature\"}"])),
            DispatchPlan::Apply {
                path: "/tmp/foo".to_string(),
                intent_json: "{\"kind\":\"add-feature\"}".to_string(),
            }
        );
    }
}

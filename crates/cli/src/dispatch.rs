//! CLI command dispatcher.
//!
//! `dispatch` parses an argv slice, routes the recognized `--machine
//! <subcommand>` grammar, and writes either the JSON listing to stdout or
//! a structured `unknown_command` diagnostic to stderr. The dispatcher
//! owns no global state and never calls `std::process::exit`: the binary
//! wraps it and propagates the returned exit code.

use std::ffi::OsString;
use std::io::Write;

use std::path::Path;

use serde_json::Value;
use threeterm_domain::ProjectGeneration;
use threeterm_domain::graph::CommandIntent;
use threeterm_host::service::ProjectService;
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::schema::iter;

/// Exit code returned when a `--machine` subcommand is recognized and the
/// JSON listing is emitted to stdout.
pub const EXIT_OK: i32 = 0;

/// Exit code returned when the dispatcher rejects the argv (unknown
/// subcommand, missing value, or no `--machine` flag). The same code is
/// used for every `unknown_command` failure so the caller's switch on
/// `Diagnostic.code` is the single parsing surface.
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;
pub const EXIT_PERSISTENCE_FAILURE: i32 = 3;
pub const EXIT_INVALID_REQUEST: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchPlan<'a> {
    List,
    NewProject { path: &'a str },
    Identity { path: &'a str },
    Load { path: &'a str },
    Apply { path: &'a str, intent: &'a str },
    Unknown { arg: &'a str },
}

/// Inspect the argv slice and decide which dispatch plan to execute.
///
/// The grammar is:
/// - `["--machine", "list"]` -> `DispatchPlan::List`
/// - `["new-project", <path>]` -> `DispatchPlan::NewProject`
/// - `["--machine", "new-project", <path>]` -> `DispatchPlan::NewProject`
/// - `["identity", <path>]` -> `DispatchPlan::Identity`
/// - `["--machine", "identity", <path>]` -> `DispatchPlan::Identity`
/// - `["load", <path>]` -> `DispatchPlan::Load`
/// - `["--machine", "load", <path>]` -> `DispatchPlan::Load`
/// - `["apply", <path>, <intent-json>]` -> `DispatchPlan::Apply`
/// - `["--machine", "apply", <path>, <intent-json>]` -> `DispatchPlan::Apply`
/// - `["--machine", <other>]` -> `DispatchPlan::Unknown { arg: <other> }`
/// - `["--machine"]` -> `DispatchPlan::Unknown { arg: "--machine" }`
/// - `[]` -> `DispatchPlan::Unknown { arg: "" }`
/// - `[<other>, ..]` -> `DispatchPlan::Unknown { arg: <other> }`
fn plan<'a, I>(args: I) -> DispatchPlan<'a>
where
    I: IntoIterator<Item = &'a OsString>,
{
    let values: Vec<&OsString> = args.into_iter().collect();
    let at = |slice: &[&'a OsString], index: usize| -> &'a str {
        slice.get(index).and_then(|s| s.to_str()).unwrap_or("")
    };
    match values.as_slice() {
        [command, path] if *command == "new-project" => DispatchPlan::NewProject {
            path: at(values.as_slice(), 1),
        },
        [command, path] if *command == "identity" => DispatchPlan::Identity {
            path: at(values.as_slice(), 1),
        },
        [command, path] if *command == "load" => DispatchPlan::Load {
            path: at(values.as_slice(), 1),
        },
        [command, path, intent] if *command == "apply" => DispatchPlan::Apply {
            path: at(values.as_slice(), 1),
            intent: at(values.as_slice(), 2),
        },
        [machine, command, path] if *machine == "--machine" && *command == "new-project" => {
            DispatchPlan::NewProject {
                path: at(values.as_slice(), 2),
            }
        }
        [machine, command, path] if *machine == "--machine" && *command == "identity" => {
            DispatchPlan::Identity {
                path: at(values.as_slice(), 2),
            }
        }
        [machine, command, path] if *machine == "--machine" && *command == "load" => {
            DispatchPlan::Load {
                path: at(values.as_slice(), 2),
            }
        }
        [machine, command, path, intent] if *machine == "--machine" && *command == "apply" => {
            DispatchPlan::Apply {
                path: at(values.as_slice(), 2),
                intent: at(values.as_slice(), 3),
            }
        }
        [machine, command] if *machine == "--machine" && *command == "list" => DispatchPlan::List,
        [machine, argument, ..] if *machine == "--machine" => DispatchPlan::Unknown {
            arg: argument.to_str().unwrap_or(""),
        },
        [first, ..] => DispatchPlan::Unknown {
            arg: first.to_str().unwrap_or(""),
        },
        [] => DispatchPlan::Unknown { arg: "" },
    }
}

/// Dispatch the argv slice. Writes either the JSON listing to `stdout` or
/// a structured `unknown_command` diagnostic to `stderr`, and returns the
/// exit code.
pub fn dispatch<I>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let collected: Vec<OsString> = args.into_iter().collect();
    let plan = plan(collected.iter());

    match plan {
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::NewProject { path } => emit_new_project(path, stdout, stderr),
        DispatchPlan::Identity { path } => emit_identity(path, stdout, stderr),
        DispatchPlan::Load { path } => emit_load(path, stdout, stderr),
        DispatchPlan::Apply { path, intent } => emit_apply(path, intent, stdout, stderr),
        DispatchPlan::Unknown { arg } => emit_unknown_command(arg, stderr),
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
    let service = ProjectService::new();
    match service.new_project(Path::new(path)) {
        Ok(generation) => {
            let manifest =
                match threeterm_persistence::bundle::load(Path::new(path)).map(|b| b.manifest) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        return emit_persistence_error(&error.to_string(), stderr);
                    }
                };
            write_manifest_response(stdout, stderr, &generation, &manifest)
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_identity(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    let service = ProjectService::new();
    match service.identity(Path::new(path)) {
        Ok(generation) => match threeterm_persistence::bundle::load(Path::new(path)) {
            Ok(bundle) => {
                let response = serde_json::json!({
                    "generation_id": generation.id,
                    "log_identity": bundle.manifest.log_identity,
                });
                write_json(stdout, stderr, &response)
            }
            Err(error) => emit_persistence_error(&error.to_string(), stderr),
        },
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_load(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    let service = ProjectService::new();
    match service.load(Path::new(path)) {
        Ok(generation) => match threeterm_persistence::bundle::load(Path::new(path)) {
            Ok(bundle) => {
                let response = serde_json::json!({
                    "generation_id": generation.id,
                    "manifest": bundle.manifest,
                    "transactions": bundle.transactions,
                });
                write_json(stdout, stderr, &response)
            }
            Err(error) => emit_persistence_error(&error.to_string(), stderr),
        },
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
    if intent_json.is_empty() {
        return emit_invalid_request("apply requires a JSON intent argument", stderr);
    }
    let intent: CommandIntent = match serde_json::from_str(intent_json) {
        Ok(intent) => intent,
        Err(error) => {
            return emit_invalid_request(
                &format!("intent JSON is not a valid CommandIntent: {error}"),
                stderr,
            );
        }
    };
    let service = ProjectService::new();
    match service.apply(Path::new(path), &intent) {
        Ok(generation) => {
            let manifest =
                match threeterm_persistence::bundle::load(Path::new(path)).map(|b| b.manifest) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        return emit_persistence_error(&error.to_string(), stderr);
                    }
                };
            write_manifest_response(stdout, stderr, &generation, &manifest)
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn write_manifest_response(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    generation: &ProjectGeneration,
    manifest: &threeterm_persistence::bundle::Manifest,
) -> i32 {
    let response = serde_json::json!({
        "generation_id": generation.id,
        "manifest": manifest,
    });
    write_json(stdout, stderr, &response)
}

fn write_json(stdout: &mut dyn Write, stderr: &mut dyn Write, value: &Value) -> i32 {
    match serde_json::to_writer_pretty(&mut *stdout, value) {
        Ok(()) => {
            let _ = writeln!(stdout);
            EXIT_OK
        }
        Err(error) => emit_internal_error(&format!("response write failed: {error}"), stderr),
    }
}

fn emit_invalid_request(detail: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::persistence_failure(detail);
    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
    EXIT_INVALID_REQUEST
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

        assert_eq!(commands.len(), 5, "five registered commands");
        let list = commands
            .iter()
            .find(|command| command["id"] == "list")
            .expect("list command is registered");
        assert_eq!(list["name"], "list");
        assert_eq!(list["schema_version"], "threeterm.command.list/1");
        assert_eq!(
            list["request_schema_version"],
            "threeterm.command.list.request/1"
        );
        assert_eq!(
            list["response_schema_version"],
            "threeterm.command.list.response/1"
        );
        assert!(list["request_schema"].is_object());
        assert!(list["response_schema"].is_object());
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
}

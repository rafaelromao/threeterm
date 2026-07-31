//! CLI command dispatcher.
//!
//! `dispatch` parses an argv slice, routes the recognized `--machine
//! <subcommand>` grammar, and writes either the JSON listing envelope to
//! stdout or a structured `unknown_command` diagnostic to stderr. The
//! dispatcher owns no global state and never calls `std::process::exit`:
//! the binary wraps it and propagates the returned exit code.

use std::ffi::OsString;
use std::io::Write;

use serde_json::json;
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::schema::{COMMAND_REGISTRY, CommandSchema, registry_hash};

/// Exit code returned when a `--machine` subcommand is recognized and the
/// JSON listing is emitted to stdout.
pub const EXIT_OK: i32 = 0;

/// Exit code returned when the dispatcher rejects the argv (unknown
/// subcommand, missing value, or no `--machine` flag). The same code is
/// used for every `unknown_command` failure so the caller's switch on
/// `Diagnostic.code` is the single parsing surface.
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchPlan<'a> {
    List,
    Unknown { arg: &'a str },
}

/// Inspect the argv slice and decide which dispatch plan to execute.
///
/// The grammar is:
/// - `["--machine", "list"]` -> `DispatchPlan::List`
/// - `["--machine", <other>]` -> `DispatchPlan::Unknown { arg: <other> }`
/// - `["--machine"]` -> `DispatchPlan::Unknown { arg: "--machine" }`
/// - `[]` or anything else -> `DispatchPlan::Unknown { arg: "<argv[0]>" or "" }`
fn plan<'a, I>(args: I) -> DispatchPlan<'a>
where
    I: IntoIterator<Item = &'a OsString>,
{
    let mut iter = args.into_iter();

    let Some(first) = iter.next() else {
        return DispatchPlan::Unknown { arg: "" };
    };

    if first != "--machine" {
        return DispatchPlan::Unknown {
            arg: first.to_str().unwrap_or(""),
        };
    }

    match iter.next() {
        Some(value) if value == "list" => DispatchPlan::List,
        Some(value) => DispatchPlan::Unknown {
            arg: value.to_str().unwrap_or(""),
        },
        None => DispatchPlan::Unknown { arg: "--machine" },
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
        DispatchPlan::List => emit_listing(stdout),
        DispatchPlan::Unknown { arg } => emit_unknown_command(arg, stderr),
    }
}

fn emit_listing(stdout: &mut dyn Write) -> i32 {
    let entries: Vec<&CommandSchema> = COMMAND_REGISTRY.iter().collect();
    let envelope = json!({
        "schema_version": threeterm_protocol::schema_version(),
        "registry_hash": registry_hash(),
        "commands": entries,
    });

    match serde_json::to_writer_pretty(&mut *stdout, &envelope) {
        Ok(()) => {
            let _ = writeln!(stdout);
            EXIT_OK
        }
        Err(error) => {
            let _ = writeln!(stdout, "failed to serialize listing: {error}");
            EXIT_UNKNOWN_COMMAND
        }
    }
}

fn emit_unknown_command(arg: &str, stderr: &mut dyn Write) -> i32 {
    let diagnostic = Diagnostic::unknown_command(arg);

    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
            EXIT_UNKNOWN_COMMAND
        }
        Err(error) => {
            let _ = writeln!(stderr, "failed to serialize diagnostic: {error}");
            EXIT_UNKNOWN_COMMAND
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn dispatch_machine_list_writes_json_envelope_to_stdout() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_OK);
        assert!(stderr.is_empty(), "stderr must be empty on success");

        let stdout_text = std::str::from_utf8(&stdout).expect("stdout is utf-8");
        let parsed: serde_json::Value =
            serde_json::from_str(stdout_text).expect("dispatch output is parseable JSON");

        let commands = parsed["commands"]
            .as_array()
            .expect("commands is a JSON array");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["id"], "list");
        assert_eq!(commands[0]["name"], "list");
        assert_eq!(commands[0]["schema_version"], "threeterm.command.list/1");
        assert!(commands[0]["request_schema"].is_object());
        assert!(commands[0]["response_schema"].is_object());
        assert_eq!(
            parsed["schema_version"],
            serde_json::Value::from(threeterm_protocol::schema_version())
        );
        assert_eq!(
            parsed["registry_hash"],
            serde_json::Value::from(registry_hash())
        );
    }

    #[test]
    fn dispatch_machine_unknown_writes_diagnostic_to_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        assert!(stdout.is_empty(), "stdout must be empty on diagnostic");

        let stderr_text = std::str::from_utf8(&stderr).expect("stderr is utf-8");
        let parsed: serde_json::Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");

        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "bogus");
        assert_eq!(
            parsed["schema_version"],
            serde_json::Value::from(threeterm_protocol::schema_version())
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
        let parsed: serde_json::Value =
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
        let parsed: serde_json::Value =
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
        let parsed: serde_json::Value =
            serde_json::from_str(stderr_text).expect("diagnostic output is parseable JSON");

        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "");
    }

    #[test]
    fn dispatch_does_not_mutate_the_registry_hash() {
        let before = registry_hash();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let _ = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);
        let after = registry_hash();

        assert_eq!(
            before, after,
            "dispatch must not mutate the canonical registry state"
        );
    }
}

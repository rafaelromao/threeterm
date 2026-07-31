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
use threeterm_domain::{ProjectGeneration, feature_graph_hash_hex, revision_hex};
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum DispatchPlan {
    List,
    NewProject {
        path: String,
    },
    Save {
        path: String,
        feature_id: String,
        kind: String,
    },
    Load {
        path: String,
    },
    Unknown {
        arg: String,
    },
}

/// Inspect the argv slice and decide which dispatch plan to execute.
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
        "new-project" => {
            if args.len() != 3 {
                let arg = args
                    .get(2)
                    .map(|s| s.to_str().unwrap_or("").to_string())
                    .unwrap_or_default();
                return DispatchPlan::Unknown { arg };
            }
            DispatchPlan::NewProject {
                path: args[2].to_str().unwrap_or("").to_string(),
            }
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
    let path = rest[0].to_str().unwrap_or("").to_string();
    if path.starts_with("--") {
        return DispatchPlan::Unknown { arg: path.clone() };
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
        path,
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
    let path = rest[0].to_str().unwrap_or("").to_string();
    if path.starts_with("--") {
        return DispatchPlan::Unknown { arg: path.clone() };
    }
    if rest.len() > 1 {
        return DispatchPlan::Unknown {
            arg: rest[1].to_str().unwrap_or("").to_string(),
        };
    }
    DispatchPlan::Load { path }
}

/// Dispatch the argv slice. Writes either the JSON listing to `stdout` or
/// a structured `unknown_command` diagnostic to `stderr`, and returns the
/// exit code.
pub fn dispatch<I>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let collected: Vec<OsString> = args.into_iter().collect();
    let plan = plan(&collected);

    match plan {
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::NewProject { path } => emit_new_project(&path, stdout, stderr),
        DispatchPlan::Save {
            path,
            feature_id,
            kind,
        } => emit_save(&path, &feature_id, &kind, stdout, stderr),
        DispatchPlan::Load { path } => emit_load(&path, stdout, stderr),
        DispatchPlan::Unknown { arg } => emit_unknown_command(&arg, stderr),
    }
}

fn emit_snapshot_for_loaded(
    generation: &ProjectGeneration,
    transaction_sha256: &str,
    stdout: &mut dyn Write,
) -> i32 {
    let graph_hash = feature_graph_hash_hex(generation);
    let terminal_log_digest = if transaction_sha256.is_empty() {
        threeterm_domain::EMPTY_LOG_DIGEST_HEX
    } else {
        transaction_sha256
    };
    let revision = revision_hex(&graph_hash, terminal_log_digest);
    let response = serde_json::json!({
        "feature_graph_hash": graph_hash,
        "revision_hash": revision,
    });
    let serialize_result = serde_json::to_writer_pretty(&mut *stdout, &response);
    let _ = writeln!(stdout);
    match serialize_result {
        Ok(()) => EXIT_OK,
        Err(_) => EXIT_PERSISTENCE_FAILURE,
    }
}

fn emit_save(
    path: &str,
    feature_id: &str,
    kind: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let bundle_path = Path::new(path);
    let manifest_present = bundle_path.is_dir() && bundle_path.join("manifest.json").exists();
    let result = if !manifest_present {
        // Either the path is missing entirely, or the path is an empty
        // directory with no manifest yet. Remove it so write_fresh can
        // take ownership of the path without colliding with an existing
        // entry.
        if bundle_path.exists() {
            let _ = std::fs::remove_dir_all(bundle_path);
        }
        threeterm_persistence::write_fresh(bundle_path, ProjectGeneration::fresh())
            .and_then(|_| {
                threeterm_persistence::append_feature_to_features(bundle_path, feature_id, kind)
            })
            .and_then(|manifest| {
                threeterm_persistence::load(bundle_path).map(|loaded| (loaded.generation, manifest))
            })
    } else {
        threeterm_persistence::load(bundle_path).and_then(|loaded| {
            let generation = loaded.generation;
            threeterm_persistence::append_feature_to_features(bundle_path, feature_id, kind)
                .map(|manifest| (generation, manifest))
        })
    };

    match result {
        Ok((generation, manifest)) => {
            let transaction_sha256 = manifest.transaction_sha256.clone();
            emit_snapshot_for_loaded(&generation, &transaction_sha256, stdout)
        }
        Err(err) => {
            let diagnostic = Diagnostic::persistence_failure(&err.to_string());
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
    }
}

fn emit_load(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    match threeterm_persistence::load(Path::new(path)) {
        Ok(loaded) => emit_snapshot_for_loaded(
            &loaded.generation,
            &loaded.manifest.transaction_sha256,
            stdout,
        ),
        Err(err) => {
            let diagnostic = Diagnostic::persistence_failure(&err.to_string());
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
    match threeterm_persistence::write_fresh(Path::new(path), generation.clone()) {
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

        assert_eq!(commands.len(), 2, "two registered commands");
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

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use serde_json::Value;
use threeterm_domain::ProjectGeneration;
use threeterm_host::{Host, HostError};
use threeterm_protocol::diagnostic::{Diagnostic, DiagnosticCode};
use threeterm_protocol::schema::iter;
pub use threeterm_protocol::schema::{
    LOAD_RESPONSE_SCHEMA_VERSION, SAVE_RESPONSE_SCHEMA_VERSION, SOLVE_SKETCH_RESPONSE_SCHEMA_VERSION,
};
use threeterm_slvs_worker::{SketchRequest, SlvsWorker};

pub const EXIT_OK: i32 = 0;
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;
pub const EXIT_INTEGRITY_FAILURE: i32 = 2;
pub const EXIT_PERSISTENCE_FAILURE: i32 = 3;
pub const EXIT_WORKER_FAILURE: i32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    SolveSketch {
        bundle: String,
        sketch_file: String,
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
        "solve-sketch" => parse_solve_sketch(&args[2..]),
        _ => DispatchPlan::Unknown {
            arg: command.to_string(),
        },
    }
}

fn parse_solve_sketch(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "solve-sketch".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut sketch_file: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if flag == "--bundle" {
            let Some(value) = args.get(index + 1) else {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            };
            bundle = Some(value.to_string_lossy().into_owned());
            index += 2;
            continue;
        }
        if flag == "--sketch-file" {
            let Some(value) = args.get(index + 1) else {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            };
            sketch_file = Some(value.to_string_lossy().into_owned());
            index += 2;
            continue;
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(sketch_file) = sketch_file else {
        return DispatchPlan::Unknown {
            arg: "--sketch-file".to_string(),
        };
    };
    DispatchPlan::SolveSketch {
        bundle,
        sketch_file,
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
        DispatchPlan::SolveSketch { bundle, sketch_file } => {
            emit_solve_sketch(&bundle, &sketch_file, stdout, stderr)
        }
        DispatchPlan::Unknown { arg } => emit_unknown_command(&arg, stderr),
    }
}

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
    match threeterm_persistence::write_fresh(Path::new(path), generation.clone()) {
        Ok(manifest) => write_success(
            stdout,
            &serde_json::json!({
                "generation_id": generation.id,
                "manifest": manifest,
            }),
            stderr,
        ),
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

fn emit_solve_sketch(
    bundle: &str,
    sketch_file: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let raw = match std::fs::read_to_string(sketch_file) {
        Ok(raw) => raw,
        Err(error) => {
            let diagnostic =
                Diagnostic::persistence_failure(&format!("sketch file read failed: {error}"));
            write_diagnostic(stderr, &diagnostic);
            return EXIT_PERSISTENCE_FAILURE;
        }
    };
    let request: SketchRequest = match serde_json::from_str(&raw) {
        Ok(request) => request,
        Err(error) => {
            let diagnostic =
                Diagnostic::unknown_command(&format!("sketch envelope invalid: {error}"));
            write_diagnostic(stderr, &diagnostic);
            return EXIT_UNKNOWN_COMMAND;
        }
    };
    let worker = match SlvsWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("worker locate failed: {error}");
            let diagnostic = Diagnostic::integrity_failure(&detail);
            write_diagnostic(stderr, &diagnostic);
            return EXIT_WORKER_FAILURE;
        }
    };
    match Host::new().solve_sketch(bundle, &request, &worker) {
        Ok(view) => {
            let coords = view.solve.coordinates.clone().unwrap_or_default();
            let value = serde_json::json!({
                "status": view.solve.status,
                "dof": view.solve.dof,
                "resolved_entity_ids": view.solve.resolved_entity_ids,
                "failed_constraint_ids": view.solve.failed_constraint_ids,
                "coordinates": coords,
                "feature_graph_hash": view.snapshot.feature_graph_hash,
                "revision_hash": view.snapshot.revision_hash,
                "schema_version": SOLVE_SKETCH_RESPONSE_SCHEMA_VERSION,
            });
            write_success(stdout, &value, stderr)
        }
        Err(error) => {
            let detail = format!("{error}");
            let diagnostic = match &error {
                HostError::WorkerFailure { .. } | HostError::WorkerUnavailable { .. } => {
                    Diagnostic {
                        code: DiagnosticCode::WorkerFailure,
                        arg: detail.clone(),
                        schema_version: crate::schema_version(),
                    }
                }
                HostError::SketchNotFullyConstrained { status, dof } => Diagnostic {
                    code: DiagnosticCode::SketchNotFullyConstrained,
                    arg: format!("status={status} dof={dof}"),
                    schema_version: crate::schema_version(),
                },
                _ => Diagnostic::integrity_failure(&detail),
            };
            write_diagnostic(stderr, &diagnostic);
            match error {
                HostError::Persistence(_) => EXIT_PERSISTENCE_FAILURE,
                HostError::WorkerFailure { .. }
                | HostError::WorkerUnavailable { .. }
                | HostError::SketchNotFullyConstrained { .. } => EXIT_WORKER_FAILURE,
                _ => EXIT_INTEGRITY_FAILURE,
            }
        }
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
        HostError::Persistence(error) => error.diagnostic_detail(),
        HostError::WorkerFailure { detail } => detail,
        HostError::WorkerUnavailable { detail } => detail,
        HostError::SketchNotFullyConstrained { status, dof } => {
            return emit_sketch_not_constrained(status, *dof, stderr);
        }
    };
    let diagnostic = Diagnostic::integrity_failure(detail);
    write_diagnostic(stderr, &diagnostic);
    EXIT_INTEGRITY_FAILURE
}

fn emit_sketch_not_constrained(
    status: &str,
    dof: i64,
    stderr: &mut dyn Write,
) -> i32 {
    let diagnostic = Diagnostic {
        code: DiagnosticCode::SketchNotFullyConstrained,
        arg: format!("status={status} dof={dof}"),
        schema_version: crate::schema_version(),
    };
    write_diagnostic(stderr, &diagnostic);
    EXIT_WORKER_FAILURE
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

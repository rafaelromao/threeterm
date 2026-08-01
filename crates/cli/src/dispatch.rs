use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use threeterm_domain::{
    CommandIntent, ComponentDefinitionId, ComponentInstanceId, DomainError, FeatureDescriptor,
    FeatureId, ProjectGeneration, Transform,
};
use threeterm_host::ProjectService;
use threeterm_protocol::diagnostic::Diagnostic;
use threeterm_protocol::schema::{
    CommandId, DEFINE_COMPONENT_COMMAND_ID, EDIT_PARAMETER_COMMAND_ID, INDEPENDENT_COPY_COMMAND_ID,
    PLACE_INSTANCE_COMMAND_ID, TRANSFORM_INSTANCE_COMMAND_ID, find, iter,
};
use threeterm_protocol::schema_validator::validate;

pub const EXIT_OK: i32 = 0;
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;
pub const EXIT_PERSISTENCE_FAILURE: i32 = 3;
pub const EXIT_INVALID_REQUEST: i32 = 4;
pub const EXIT_DOMAIN_FAILURE: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentCommand {
    Define,
    Place,
    Transform,
    IndependentCopy,
    EditParameter,
}

impl ComponentCommand {
    fn command_id(self) -> CommandId {
        match self {
            Self::Define => DEFINE_COMPONENT_COMMAND_ID,
            Self::Place => PLACE_INSTANCE_COMMAND_ID,
            Self::Transform => TRANSFORM_INSTANCE_COMMAND_ID,
            Self::IndependentCopy => INDEPENDENT_COPY_COMMAND_ID,
            Self::EditParameter => EDIT_PARAMETER_COMMAND_ID,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchPlan<'a> {
    List,
    NewProject {
        path: &'a str,
    },
    Component {
        command: ComponentCommand,
        path: &'a str,
        payload: &'a str,
    },
    Invalid {
        detail: &'a str,
    },
    Unknown {
        arg: &'a str,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefineComponentRequest {
    definition_id: ComponentDefinitionId,
    features: Vec<FeatureDescriptor>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaceInstanceRequest {
    instance_id: ComponentInstanceId,
    definition_id: ComponentDefinitionId,
    transform: Transform,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformInstanceRequest {
    instance_id: ComponentInstanceId,
    transform: Transform,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndependentCopyRequest {
    source_instance_id: ComponentInstanceId,
    copy_suffix: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditParameterRequest {
    definition_id: ComponentDefinitionId,
    feature_id: FeatureId,
    parameter_name: String,
    parameter_value: Value,
}

fn component_command(value: &OsString) -> Option<ComponentCommand> {
    match value.to_str() {
        Some("define-component") => Some(ComponentCommand::Define),
        Some("place-instance") => Some(ComponentCommand::Place),
        Some("transform-instance") => Some(ComponentCommand::Transform),
        Some("independent-copy") => Some(ComponentCommand::IndependentCopy),
        Some("edit-parameter") => Some(ComponentCommand::EditParameter),
        _ => None,
    }
}

fn plan<'a, I>(args: I) -> DispatchPlan<'a>
where
    I: IntoIterator<Item = &'a OsString>,
{
    let values: Vec<&OsString> = args.into_iter().collect();
    match values.as_slice() {
        [command, path] if *command == "new-project" => DispatchPlan::NewProject {
            path: path.to_str().unwrap_or(""),
        },
        [machine, command, path] if *machine == "--machine" && *command == "new-project" => {
            DispatchPlan::NewProject {
                path: path.to_str().unwrap_or(""),
            }
        }
        [machine, command] if *machine == "--machine" && *command == "list" => DispatchPlan::List,
        [machine, command, path, payload] if *machine == "--machine" => {
            match component_command(command) {
                Some(command) => DispatchPlan::Component {
                    command,
                    path: path.to_str().unwrap_or(""),
                    payload: payload.to_str().unwrap_or(""),
                },
                None => DispatchPlan::Unknown {
                    arg: command.to_str().unwrap_or(""),
                },
            }
        }
        [machine, command, ..]
            if *machine == "--machine" && component_command(command).is_some() =>
        {
            DispatchPlan::Invalid {
                detail: "component command requires a bundle path and JSON request",
            }
        }
        [machine, argument, ..] if *machine == "--machine" => DispatchPlan::Unknown {
            arg: argument.to_str().unwrap_or(""),
        },
        [first, ..] => DispatchPlan::Unknown {
            arg: first.to_str().unwrap_or(""),
        },
        [] => DispatchPlan::Unknown { arg: "" },
    }
}

pub fn dispatch<I>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let collected: Vec<OsString> = args.into_iter().collect();
    match plan(collected.iter()) {
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::NewProject { path } => emit_new_project(path, stdout, stderr),
        DispatchPlan::Component {
            command,
            path,
            payload,
        } => emit_component(command, path, payload, stdout, stderr),
        DispatchPlan::Invalid { detail } => emit_diagnostic(
            Diagnostic::invalid_request(detail),
            EXIT_INVALID_REQUEST,
            stderr,
        ),
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
    let generation = ProjectGeneration::fresh();
    match threeterm_persistence::write_fresh(Path::new(path), generation.clone()) {
        Ok(manifest) => emit_json(
            serde_json::json!({
                "generation_id": generation.id,
                "manifest": manifest,
            }),
            stdout,
            stderr,
        ),
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_component(
    command: ComponentCommand,
    path: &str,
    payload: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let intent = match parse_component_intent(command, payload) {
        Ok(intent) => intent,
        Err(detail) => {
            return emit_diagnostic(
                Diagnostic::invalid_request(&detail),
                EXIT_INVALID_REQUEST,
                stderr,
            );
        }
    };
    let loaded = match threeterm_persistence::load(Path::new(path)) {
        Ok(loaded) => loaded,
        Err(error) => return emit_persistence_error(&error.to_string(), stderr),
    };
    let mut service = ProjectService::new(loaded.generation);
    let transaction = match service.execute(intent) {
        Ok(transaction) => transaction,
        Err(error) => return emit_domain_error(&error, stderr),
    };
    let persisted = match threeterm_persistence::append_transaction(Path::new(path), &transaction) {
        Ok(persisted) => persisted,
        Err(error) => return emit_persistence_error(&error.to_string(), stderr),
    };
    emit_json(
        serde_json::json!({
            "generation_id": persisted.generation.id,
            "revision_id": persisted.generation.current_revision().id,
            "reattachment": transaction.reattachment,
            "affected_ids": transaction.affected_ids,
        }),
        stdout,
        stderr,
    )
}

fn parse_component_intent(
    command: ComponentCommand,
    payload: &str,
) -> Result<CommandIntent, String> {
    let value: Value = serde_json::from_str(payload).map_err(|error| error.to_string())?;
    let schema = find(command.command_id()).expect("component command is registered");
    validate(&schema.request_schema, &value)?;
    match command {
        ComponentCommand::Define => {
            let request: DefineComponentRequest =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            Ok(CommandIntent::DefineComponent {
                definition_id: request.definition_id,
                features: request.features,
            })
        }
        ComponentCommand::Place => {
            let request: PlaceInstanceRequest =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            Ok(CommandIntent::PlaceInstance {
                instance_id: request.instance_id,
                definition_id: request.definition_id,
                transform: request.transform,
            })
        }
        ComponentCommand::Transform => {
            let request: TransformInstanceRequest =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            Ok(CommandIntent::TransformInstance {
                instance_id: request.instance_id,
                transform: request.transform,
            })
        }
        ComponentCommand::IndependentCopy => {
            let request: IndependentCopyRequest =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            Ok(CommandIntent::IndependentCopy {
                source_instance_id: request.source_instance_id,
                copy_suffix: request.copy_suffix,
            })
        }
        ComponentCommand::EditParameter => {
            let request: EditParameterRequest =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            Ok(CommandIntent::EditParameter {
                definition_id: request.definition_id,
                feature_id: request.feature_id,
                parameter_name: request.parameter_name,
                parameter_value: request.parameter_value,
            })
        }
    }
}

fn emit_json(value: Value, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    match serde_json::to_writer_pretty(&mut *stdout, &value) {
        Ok(()) => {
            let _ = writeln!(stdout);
            EXIT_OK
        }
        Err(error) => emit_internal_error(&format!("response write failed: {error}"), stderr),
    }
}

fn emit_domain_error(error: &DomainError, stderr: &mut dyn Write) -> i32 {
    let diagnostic = match error {
        DomainError::ReferenceAmbiguous(detail) => Diagnostic::reference_ambiguous(detail),
        DomainError::ReferenceLost(detail) => Diagnostic::reference_lost(detail),
        DomainError::ReferenceIncompatible(detail) => Diagnostic::reference_incompatible(detail),
        other => Diagnostic::invalid_request(&other.to_string()),
    };
    emit_diagnostic(diagnostic, EXIT_DOMAIN_FAILURE, stderr)
}

fn emit_persistence_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    emit_diagnostic(
        Diagnostic::persistence_failure(detail),
        EXIT_PERSISTENCE_FAILURE,
        stderr,
    )
}

fn emit_unknown_command(arg: &str, stderr: &mut dyn Write) -> i32 {
    emit_diagnostic(
        Diagnostic::unknown_command(arg),
        EXIT_UNKNOWN_COMMAND,
        stderr,
    )
}

fn emit_diagnostic(diagnostic: Diagnostic, exit: i32, stderr: &mut dyn Write) -> i32 {
    match serde_json::to_writer_pretty(&mut *stderr, &diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
            exit
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
            exit
        }
    }
}

fn emit_internal_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    emit_diagnostic(
        Diagnostic::unknown_command(detail),
        EXIT_UNKNOWN_COMMAND,
        stderr,
    )
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

        let parsed: Value = serde_json::from_slice(&stdout).expect("dispatch output is JSON");
        let commands = parsed
            .as_array()
            .expect("dispatch output is a top-level JSON array");
        assert_eq!(commands.len(), 7);
        for id in [
            "list",
            "new-project",
            "define-component",
            "place-instance",
            "transform-instance",
            "independent-copy",
            "edit-parameter",
        ] {
            assert!(commands.iter().any(|command| command["id"] == id));
        }
    }

    #[test]
    fn dispatch_machine_unknown_writes_diagnostic_to_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);

        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        assert!(stdout.is_empty(), "stdout must be empty on diagnostic");
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
    fn dispatch_does_not_call_exit_or_panic() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let _ = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&[]), &mut stdout, &mut stderr);
    }
}

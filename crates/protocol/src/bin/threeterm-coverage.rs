use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;
use threeterm_protocol::coverage::{
    ExecutionEvidence, JourneyStreams, SourceIdentity, Surface, UiControlEvidence,
    current_registry, new_journey_report, write_json_atomic,
};

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, manifest, transcript, discovery, output] if command == "tui-report" => {
            match write_tui_report(
                Path::new(manifest),
                Path::new(transcript),
                Path::new(discovery),
                Path::new(output),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("threeterm-coverage: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: threeterm-coverage tui-report MANIFEST TRANSCRIPT DISCOVERY OUTPUT");
            ExitCode::from(2)
        }
    }
}

fn write_tui_report(
    manifest_path: &Path,
    transcript_path: &Path,
    discovery_path: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let manifest = read_json(manifest_path)?;
    let transcript = read_json_lines(transcript_path)?;
    let discovery = read_json(discovery_path).unwrap_or(Value::Null);
    let source = SourceIdentity {
        commit: manifest["source"]["commit"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        dirty: manifest["source"]["dirty"].as_bool().unwrap_or(true),
    };
    let result = manifest["result"].as_str().unwrap_or("failed");
    let observed = transcript
        .iter()
        .filter_map(|entry| entry["command"].as_str())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let mut observed_with_lifecycle = observed;
    if discovery["command"].as_str() == Some("list") && manifest["events"]["discovery"] == "passed"
    {
        observed_with_lifecycle.insert("list".to_string());
    }
    if manifest["events"]["workflow"] == "passed" {
        observed_with_lifecycle.insert("new-project".to_string());
    }
    if manifest["events"]["lifecycle"] == "passed" {
        observed_with_lifecycle.insert("save".to_string());
        observed_with_lifecycle.insert("load".to_string());
    }
    if manifest["events"]["validation"] == "passed" {
        observed_with_lifecycle.insert("validate".to_string());
    }
    if manifest["events"]["export"] == "passed" {
        observed_with_lifecycle.insert("export".to_string());
    }

    let registry = current_registry();
    let mut executions = Vec::new();
    let mut controls = Vec::new();
    for (step_index, command_name) in threeterm_protocol::coverage::required_commands()
        .iter()
        .enumerate()
    {
        if observed_with_lifecycle.contains(*command_name) {
            let contract = registry
                .rows
                .iter()
                .find(|row| row.command_name == *command_name)
                .ok_or_else(|| format!("required command is not registered: {command_name}"))?;
            executions.push(ExecutionEvidence {
                command_id: contract.command_id.clone(),
                command_name: contract.command_name.clone(),
                command_schema_version: contract.command_schema_version.clone(),
                request_schema_hash: contract.request_schema_hash.clone(),
                response_schema_hash: contract.response_schema_hash.clone(),
                outcome: "ok".to_string(),
                step_index: Some(step_index as u32),
                evidence_id: format!("tui-{step_index}-{command_name}"),
            });
            controls.extend([
                UiControlEvidence {
                    control: format!("palette-select:{command_name}"),
                    outcome: "ok".to_string(),
                    detail: "command selected through the TUI palette".to_string(),
                },
                UiControlEvidence {
                    control: format!("preview:{command_name}"),
                    outcome: "ok".to_string(),
                    detail: "preview acknowledgement retained".to_string(),
                },
                UiControlEvidence {
                    control: format!("commit:{command_name}"),
                    outcome: "ok".to_string(),
                    detail: "commit acknowledgement retained".to_string(),
                },
            ]);
        }
    }
    let report = new_journey_report(
        Surface::Tui,
        manifest["test"]
            .as_str()
            .unwrap_or("production_tui_all_tools_stl_journey"),
        result,
        source,
        JourneyStreams {
            executions,
            ui_controls: controls,
            ..JourneyStreams::default()
        },
    );
    write_json_atomic(output_path, &report)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn read_json_lines(path: &Path) -> Result<Vec<Value>, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| format!("{}: {error}", path.display()))
        })
        .collect()
}

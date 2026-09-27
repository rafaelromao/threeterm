use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;
use threeterm_protocol::coverage::{
    CommandContract, ExecutionEvidence, JOURNEY_REPORT_SCHEMA_VERSION, JourneyReport,
    RECIPE_SCHEMA_VERSION, SourceIdentity, Surface, UiControlEvidence, required_commands,
    schema_hash, write_json_atomic,
};

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [
            command,
            manifest,
            transcript,
            discovery,
            coverage_log,
            output,
        ] if command == "tui-report" => {
            match write_tui_report(
                Path::new(manifest),
                Path::new(transcript),
                Path::new(discovery),
                Path::new(coverage_log),
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
            eprintln!(
                "usage: threeterm-coverage tui-report MANIFEST TRANSCRIPT DISCOVERY COVERAGE_LOG OUTPUT"
            );
            ExitCode::from(2)
        }
    }
}

fn write_tui_report(
    manifest_path: &Path,
    transcript_path: &Path,
    discovery_path: &Path,
    coverage_log_path: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let manifest = read_json(manifest_path)?;
    let transcript = read_json_lines(transcript_path)?;
    let discovery = read_json(discovery_path)?;
    let markers = read_coverage_log(coverage_log_path)?;
    if discovery["command"] != "list" {
        return Err(format!(
            "{} is not a retained list discovery record",
            discovery_path.display()
        ));
    }
    let source = source_identity_from_manifest(&manifest)?;
    let result = manifest["result"].as_str().unwrap_or("failed");
    let test = manifest["test"]
        .as_str()
        .ok_or_else(|| "TUI manifest has no retained test name".to_string())?;
    let adapter_exposure = successful_list_exposure(&markers)?;
    if adapter_exposure.is_empty() {
        return Err("retained list discovery record advertises no commands".to_string());
    }
    let registry_hash = markers
        .iter()
        .find_map(|marker| marker["registry_hash"].as_str())
        .ok_or_else(|| "coverage log has no registry hash".to_string())?
        .to_string();
    let executions = markers
        .iter()
        .enumerate()
        .map(|(step_index, marker)| execution_from_marker(marker, step_index))
        .collect::<Result<Vec<_>, _>>()?;
    let controls = ui_controls_from_transcript(&transcript)?;
    let report = JourneyReport {
        schema_version: JOURNEY_REPORT_SCHEMA_VERSION.to_string(),
        surface: Surface::Tui,
        test: test.to_string(),
        recipe_schema_version: RECIPE_SCHEMA_VERSION.to_string(),
        source,
        result: result.to_string(),
        registry_hash,
        registry: adapter_exposure.clone(),
        required_commands: required_commands()
            .iter()
            .map(|command| (*command).to_string())
            .collect(),
        adapter_exposure,
        executions,
        raw_transport_methods: Vec::new(),
        ui_controls: controls,
    };
    write_json_atomic(output_path, &report)
}

fn source_identity_from_manifest(manifest: &Value) -> Result<SourceIdentity, String> {
    let source = manifest
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| "TUI manifest has no source identity object".to_string())?;
    let commit = source
        .get("commit")
        .and_then(Value::as_str)
        .filter(|commit| !commit.is_empty())
        .ok_or_else(|| "TUI manifest source identity has no commit".to_string())?;
    let dirty = source
        .get("dirty")
        .and_then(Value::as_bool)
        .ok_or_else(|| "TUI manifest source identity has no boolean dirty flag".to_string())?;
    Ok(SourceIdentity {
        commit: commit.to_string(),
        dirty,
    })
}

fn successful_list_exposure(markers: &[Value]) -> Result<Vec<CommandContract>, String> {
    markers
        .iter()
        .find(|marker| {
            marker["command_name"] == "list"
                && marker["outcome"] == "ok"
                && marker["adapter_exposure"].is_array()
        })
        .map(|marker| parse_contracts(&marker["adapter_exposure"]))
        .transpose()?
        .ok_or_else(|| "coverage log has no successful list discovery record".to_string())
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
            let value: Value = serde_json::from_str(line)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if !value.is_object() || value["command"].as_str().is_none() {
                return Err(format!(
                    "{}: transcript entry has no command",
                    path.display()
                ));
            }
            Ok(value)
        })
        .collect()
}

fn read_coverage_log(path: &Path) -> Result<Vec<Value>, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: Value = serde_json::from_str(line.trim()).map_err(|error| {
                format!(
                    "{}: malformed tool coverage marker: {error}",
                    path.display()
                )
            })?;
            if !value.is_object() || value["command_name"].as_str().is_none() {
                return Err(format!(
                    "{}: tool coverage record has no command_name",
                    path.display()
                ));
            }
            Ok(value)
        })
        .collect()
}

fn ui_controls_from_transcript(transcript: &[Value]) -> Result<Vec<UiControlEvidence>, String> {
    const UI_CONTROL_FIELDS: &[&str] = &[
        "preview",
        "commit",
        "preview_marker",
        "commit_marker",
        "selection_marker",
        "selection_text",
    ];
    let mut controls = Vec::new();
    for entry in transcript {
        let command = entry["command"]
            .as_str()
            .ok_or_else(|| "TUI transcript entry has no command".to_string())?;
        let acknowledgements = entry
            .get("acknowledgement")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("TUI transcript entry for {command} has no acknowledgements"))?;
        for (control, detail) in acknowledgements {
            if detail.is_null() || !UI_CONTROL_FIELDS.contains(&control.as_str()) {
                continue;
            }
            let outcome = detail
                .get("outcome")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    let serialized = detail.to_string().to_ascii_lowercase();
                    if ["failed", "error", "rejected", "cancelled"]
                        .iter()
                        .any(|marker| serialized.contains(marker))
                    {
                        "failed".to_string()
                    } else {
                        "ok".to_string()
                    }
                });
            controls.push(UiControlEvidence {
                control: format!("{control}:{command}"),
                outcome,
                detail: serde_json::to_string(detail)
                    .map_err(|error| format!("serialize TUI {control} evidence: {error}"))?,
            });
        }
    }
    if controls.is_empty() {
        return Err("TUI transcript retained no non-command control acknowledgements".to_string());
    }
    Ok(controls)
}

fn execution_from_marker(marker: &Value, step_index: usize) -> Result<ExecutionEvidence, String> {
    let field = |name: &str| {
        marker[name]
            .as_str()
            .ok_or_else(|| format!("tool coverage marker is missing string field {name}"))
    };
    let command_name = field("command_name")?;
    Ok(ExecutionEvidence {
        command_id: field("command_id")?.to_string(),
        command_name: command_name.to_string(),
        command_schema_version: field("command_schema_version")?.to_string(),
        request_schema_version: field("request_schema_version")?.to_string(),
        response_schema_version: field("response_schema_version")?.to_string(),
        request_schema_hash: field("request_schema_hash")?.to_string(),
        response_schema_hash: field("response_schema_hash")?.to_string(),
        response_payload_hash: field("response_payload_hash")?.to_string(),
        outcome: field("outcome")?.to_string(),
        step_index: Some(step_index as u32),
        evidence_id: format!("tui-{step_index}-{command_name}"),
    })
}

fn parse_contracts(value: &Value) -> Result<Vec<CommandContract>, String> {
    value
        .as_array()
        .ok_or_else(|| "list tool coverage marker has no adapter exposure array".to_string())?
        .iter()
        .map(|entry| {
            let string = |name: &str| {
                entry[name]
                    .as_str()
                    .ok_or_else(|| format!("list discovery row is missing string field {name}"))
            };
            Ok(CommandContract {
                command_id: string("id")?.to_string(),
                command_name: string("name")?.to_string(),
                command_schema_version: string("schema_version")?.to_string(),
                request_schema_version: string("request_schema_version")?.to_string(),
                response_schema_version: string("response_schema_version")?.to_string(),
                request_schema_hash: schema_hash(&entry["request_schema"]),
                response_schema_hash: schema_hash(&entry["response_schema"]),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{source_identity_from_manifest, successful_list_exposure};
    use serde_json::json;

    #[test]
    fn successful_list_discovery_is_selected_after_a_failed_attempt() {
        let markers = vec![
            json!({
                "command_name": "list",
                "outcome": "failed",
                "adapter_exposure": null
            }),
            json!({
                "command_name": "list",
                "outcome": "ok",
                "adapter_exposure": [{
                    "id": "list",
                    "name": "list",
                    "schema_version": "command/1",
                    "request_schema_version": "request/1",
                    "response_schema_version": "response/1",
                    "request_schema": {},
                    "response_schema": {}
                }]
            }),
        ];

        let exposure = successful_list_exposure(&markers).expect("successful list is retained");
        assert_eq!(exposure[0].command_name, "list");
    }

    #[test]
    fn source_identity_requires_a_boolean_dirty_flag() {
        let manifest = json!({
            "source": {
                "commit": "0123456789abcdef0123456789abcdef01234567",
                "dirty": "false"
            }
        });

        let error = source_identity_from_manifest(&manifest).expect_err("invalid source fails");
        assert!(error.contains("boolean dirty flag"));
    }
}

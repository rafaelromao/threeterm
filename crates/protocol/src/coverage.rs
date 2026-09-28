//! Shared evidence contract and evaluator for the three acceptance journeys.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::sha256_hex;
use crate::schema::{self, CommandSchema};

pub const JOURNEY_REPORT_SCHEMA_VERSION: &str = "threeterm.coverage.journey-report/1";
pub const MATRIX_SCHEMA_VERSION: &str = "threeterm.coverage.matrix/1";
pub const RECIPE_SCHEMA_VERSION: &str = "threeterm.recipe.bracket-complete/1";
pub const MATRIX_FILENAME: &str = "journey-coverage-matrix.json";

const REPORT_FILENAMES: [(Surface, &str); 3] = [
    (Surface::Api, "api-journey-coverage.json"),
    (Surface::Mcp, "mcp-journey-coverage.json"),
    (Surface::Tui, "tui-journey-coverage.json"),
];

const REQUIRED_COMMANDS: &[&str] = &[
    "list",
    "new-project",
    "extrude",
    "boolean-fuse",
    "fillet",
    "chamfer",
    "hole",
    "revolve",
    "mirror",
    "linear-pattern",
    "circular-pattern",
    "shell",
    "draft",
    "loft",
    "save",
    "load",
    "validate",
    "export",
];

// This is the reviewed registry surface, not the all-tool recipe. Updating the
// command registry requires an explicit coverage review before acceptance can pass.
const REVIEWED_REGISTRY_COMMANDS: &[&str] = &[
    "list",
    "new-project",
    "identity",
    "apply",
    "rehearse",
    "save",
    "load",
    "bracket",
    "define-component",
    "create-component-instance",
    "transform-component-instance",
    "make-component-independent",
    "edit-component-parameter",
    "component-state",
    "sketch-solve",
    "bracket-edit",
    "capture-component",
    "historical-edit",
    "create-revision",
    "restore-revision",
    "undo",
    "redo",
    "timeline",
    "replay-verify",
    "extrude",
    "fit-dimension",
    "boolean-fuse",
    "boolean-cut",
    "boolean-common",
    "boolean-pattern",
    "fillet",
    "chamfer",
    "reattach-edge",
    "hole",
    "revolve",
    "mirror",
    "linear-pattern",
    "circular-pattern",
    "shell",
    "draft",
    "loft",
    "export",
    "validate",
];

const JOURNEY_TEST_NAMES: [(Surface, &str); 3] = [
    (Surface::Api, "e2e_stl_api_all_tools_l_bracket"),
    (Surface::Mcp, "e2e_stl_mcp_all_tools_l_bracket"),
    (Surface::Tui, "production_tui_all_tools_stl_journey"),
];

#[derive(Debug, Clone, Copy, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Api,
    Mcp,
    Tui,
}

impl Surface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Mcp => "mcp",
            Self::Tui => "tui",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub commit: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandContract {
    pub command_id: String,
    pub command_name: String,
    pub command_schema_version: String,
    pub request_schema_version: String,
    pub response_schema_version: String,
    pub request_schema_hash: String,
    pub response_schema_hash: String,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEvidence {
    pub command_id: String,
    pub command_name: String,
    pub command_schema_version: String,
    pub request_schema_version: String,
    pub response_schema_version: String,
    pub request_schema_hash: String,
    pub response_schema_hash: String,
    pub response_payload_hash: String,
    pub outcome: String,
    pub step_index: Option<u32>,
    pub evidence_id: String,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawTransportMethod {
    pub direction: String,
    pub method: String,
    pub id: Option<String>,
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UiControlEvidence {
    pub control: String,
    pub outcome: String,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyReport {
    pub schema_version: String,
    pub surface: Surface,
    pub test: String,
    pub recipe_schema_version: String,
    pub source: SourceIdentity,
    pub result: String,
    pub registry_hash: String,
    pub registry: Vec<CommandContract>,
    pub required_commands: Vec<String>,
    pub adapter_exposure: Vec<CommandContract>,
    pub executions: Vec<ExecutionEvidence>,
    pub raw_transport_methods: Vec<RawTransportMethod>,
    pub ui_controls: Vec<UiControlEvidence>,
}

#[derive(Debug, Clone, Default)]
pub struct JourneyStreams {
    pub executions: Vec<ExecutionEvidence>,
    pub raw_transport_methods: Vec<RawTransportMethod>,
    pub ui_controls: Vec<UiControlEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RegistrySnapshot {
    pub hash: String,
    pub rows: Vec<CommandContract>,
}

#[derive(Debug, Clone)]
pub struct ReportInput {
    pub surface: Surface,
    pub report: Option<JourneyReport>,
    pub error: Option<String>,
}

impl ReportInput {
    pub fn complete(report: JourneyReport) -> Self {
        Self {
            surface: report.surface,
            report: Some(report),
            error: None,
        }
    }

    pub fn failed(surface: Surface, error: impl Into<String>) -> Self {
        Self {
            surface,
            report: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageCell {
    pub surface: Surface,
    pub command_id: String,
    pub command_name: String,
    pub command_schema_version: String,
    pub request_schema_version: String,
    pub response_schema_version: String,
    pub request_schema_hash: String,
    pub response_schema_hash: String,
    pub status: String,
    pub execution_count: usize,
    pub executions: Vec<ExecutionEvidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageDelta {
    pub surface: Option<Surface>,
    pub kind: String,
    pub command: Option<String>,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SurfaceTransportEvidence {
    pub surface: Surface,
    pub methods: Vec<RawTransportMethod>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SurfaceUiControlEvidence {
    pub surface: Surface,
    pub controls: Vec<UiControlEvidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageMatrix {
    pub schema_version: &'static str,
    pub result: String,
    pub recipe_schema_version: &'static str,
    pub source: Option<SourceIdentity>,
    pub required_commands: Vec<String>,
    pub cells: Vec<CoverageCell>,
    pub deltas: Vec<CoverageDelta>,
    pub raw_transport_methods: Vec<SurfaceTransportEvidence>,
    pub ui_controls: Vec<SurfaceUiControlEvidence>,
}

pub fn required_commands() -> &'static [&'static str] {
    REQUIRED_COMMANDS
}

pub fn reviewed_registry_commands() -> &'static [&'static str] {
    REVIEWED_REGISTRY_COMMANDS
}

fn expected_journey_test(surface: Surface) -> &'static str {
    JOURNEY_TEST_NAMES
        .iter()
        .find_map(|(candidate, test)| (*candidate == surface).then_some(*test))
        .expect("every coverage surface has an expected journey test")
}

pub fn current_registry() -> RegistrySnapshot {
    let rows = schema::iter().map(command_contract).collect();
    RegistrySnapshot {
        hash: schema::registry_hash(),
        rows,
    }
}

pub fn command_contract(command: &CommandSchema) -> CommandContract {
    CommandContract {
        command_id: command.id.0.to_string(),
        command_name: command.name.to_string(),
        command_schema_version: command.schema_version.to_string(),
        request_schema_version: command.request_schema_version.to_string(),
        response_schema_version: command.response_schema_version.to_string(),
        request_schema_hash: schema_hash(&command.request_schema),
        response_schema_hash: schema_hash(&command.response_schema),
    }
}

pub fn new_journey_report(
    surface: Surface,
    test: impl Into<String>,
    result: impl Into<String>,
    source: SourceIdentity,
    streams: JourneyStreams,
) -> JourneyReport {
    let registry = current_registry();
    new_journey_report_with_exposure(surface, test, result, source, registry.rows, streams)
}

pub fn new_journey_report_with_exposure(
    surface: Surface,
    test: impl Into<String>,
    result: impl Into<String>,
    source: SourceIdentity,
    adapter_exposure: Vec<CommandContract>,
    streams: JourneyStreams,
) -> JourneyReport {
    let registry = current_registry();
    JourneyReport {
        schema_version: JOURNEY_REPORT_SCHEMA_VERSION.to_string(),
        surface,
        test: test.into(),
        recipe_schema_version: RECIPE_SCHEMA_VERSION.to_string(),
        source,
        result: result.into(),
        registry_hash: registry.hash,
        registry: registry.rows.clone(),
        required_commands: required_commands()
            .iter()
            .map(|command| (*command).to_string())
            .collect(),
        adapter_exposure,
        executions: streams.executions,
        raw_transport_methods: streams.raw_transport_methods,
        ui_controls: streams.ui_controls,
    }
}

pub fn write_journey_report(
    root: &Path,
    surface: Surface,
    test: impl Into<String>,
    result: impl Into<String>,
    source: SourceIdentity,
    streams: JourneyStreams,
) -> Result<std::path::PathBuf, String> {
    let report = new_journey_report(surface, test, result, source, streams);
    let path = report_path(root, surface);
    write_json_atomic(&path, &report)?;
    Ok(path)
}

pub fn write_journey_report_with_exposure(
    root: &Path,
    surface: Surface,
    test: impl Into<String>,
    result: impl Into<String>,
    source: SourceIdentity,
    adapter_exposure: Vec<CommandContract>,
    streams: JourneyStreams,
) -> Result<std::path::PathBuf, String> {
    let report =
        new_journey_report_with_exposure(surface, test, result, source, adapter_exposure, streams);
    let path = report_path(root, surface);
    write_json_atomic(&path, &report)?;
    Ok(path)
}

pub fn schema_hash(schema: &Value) -> String {
    sha256_hex(&serde_json::to_vec(schema).expect("JSON schema serializes"))
}

pub fn payload_hash(payload: &Value) -> String {
    sha256_hex(&serde_json::to_vec(payload).expect("JSON payload serializes"))
}

pub fn response_outcome(response: &Value) -> &'static str {
    if response.get("error").is_some() {
        return "failed";
    }
    match response
        .get("outcome")
        .or_else(|| response.get("status"))
        .and_then(Value::as_str)
    {
        Some("failed" | "error" | "invalid_request" | "rejected" | "cancelled") => "failed",
        _ => "ok",
    }
}

pub fn capture_source_identity(repo_root: &Path) -> Result<SourceIdentity, String> {
    let commit = env::var("THREETERM_SOURCE_COMMIT").ok();
    let dirty = env::var("THREETERM_SOURCE_DIRTY").ok();
    match (commit, dirty) {
        (Some(commit), Some(dirty)) => {
            let dirty = parse_dirty(&dirty)?;
            validate_commit(&commit)?;
            Ok(SourceIdentity { commit, dirty })
        }
        (Some(_), None) | (None, Some(_)) => Err(
            "THREETERM_SOURCE_COMMIT and THREETERM_SOURCE_DIRTY must be supplied together"
                .to_string(),
        ),
        (None, None) => {
            let commit = git_output(repo_root, &["rev-parse", "HEAD"])?;
            validate_commit(&commit)?;
            let status = git_output(
                repo_root,
                &["status", "--porcelain", "--untracked-files=all"],
            )?;
            Ok(SourceIdentity {
                commit,
                dirty: !status.is_empty(),
            })
        }
    }
}

pub fn evaluate(inputs: [ReportInput; 3]) -> CoverageMatrix {
    evaluate_with_registry(inputs, &current_registry())
}

pub fn evaluate_with_registry(
    inputs: [ReportInput; 3],
    current: &RegistrySnapshot,
) -> CoverageMatrix {
    let required = REQUIRED_COMMANDS
        .iter()
        .map(|command| (*command).to_string())
        .collect::<Vec<_>>();
    let mut deltas = Vec::new();
    let mut source = None;
    let mut recipe: Option<String> = None;
    let mut cells = Vec::new();
    let mut raw_transport_methods = Vec::new();
    let mut ui_controls = Vec::new();

    for surface in [Surface::Api, Surface::Mcp, Surface::Tui] {
        for row in &current.rows {
            if !REVIEWED_REGISTRY_COMMANDS.contains(&row.command_name.as_str()) {
                push_delta(
                    &mut deltas,
                    surface,
                    "unreviewed-registry-command",
                    Some(&row.command_name),
                    "checked-in reviewed command inventory",
                    &compact(row),
                );
            }
        }
        for command_name in REVIEWED_REGISTRY_COMMANDS {
            if !current
                .rows
                .iter()
                .any(|row| row.command_name == *command_name)
            {
                push_delta(
                    &mut deltas,
                    surface,
                    "reviewed-command-unregistered",
                    Some(command_name),
                    "registered command",
                    "missing",
                );
            }
        }
    }

    for input in &inputs {
        let Some(report) = input.report.as_ref() else {
            push_delta(
                &mut deltas,
                input.surface,
                "missing-report",
                None,
                "a retained journey report",
                input.error.as_deref().unwrap_or("report is absent"),
            );
            add_failed_cells(&mut cells, input.surface, current, &required);
            continue;
        };

        if report.schema_version != JOURNEY_REPORT_SCHEMA_VERSION {
            push_delta(
                &mut deltas,
                input.surface,
                "report-schema-drift",
                None,
                JOURNEY_REPORT_SCHEMA_VERSION,
                &report.schema_version,
            );
        }
        if report.surface != input.surface {
            push_delta(
                &mut deltas,
                input.surface,
                "surface-mismatch",
                None,
                input.surface.as_str(),
                report.surface.as_str(),
            );
        }
        let expected_test = expected_journey_test(input.surface);
        if report.test != expected_test {
            push_delta(
                &mut deltas,
                input.surface,
                "journey-test-mismatch",
                None,
                expected_test,
                &report.test,
            );
        }
        if report.result != "passed" {
            push_delta(
                &mut deltas,
                input.surface,
                "journey-failed",
                None,
                "passed",
                &report.result,
            );
        }
        if report.recipe_schema_version != RECIPE_SCHEMA_VERSION {
            push_delta(
                &mut deltas,
                input.surface,
                "recipe-version-drift",
                None,
                RECIPE_SCHEMA_VERSION,
                &report.recipe_schema_version,
            );
        }
        if let Some(expected) = recipe.as_ref() {
            if expected != &report.recipe_schema_version {
                push_delta(
                    &mut deltas,
                    input.surface,
                    "recipe-version-mismatch",
                    None,
                    expected,
                    &report.recipe_schema_version,
                );
            }
        } else {
            recipe = Some(report.recipe_schema_version.clone());
        }
        if !valid_commit(&report.source.commit) {
            push_delta(
                &mut deltas,
                input.surface,
                "source-invalid",
                None,
                "lowercase hexadecimal commit",
                &report.source.commit,
            );
        }
        if let Some(expected) = source.as_ref() {
            if expected != &report.source {
                push_delta(
                    &mut deltas,
                    input.surface,
                    "source-mismatch",
                    None,
                    &format_source(expected),
                    &format_source(&report.source),
                );
            }
        } else {
            source = Some(report.source.clone());
        }
        if report.registry_hash != current.hash {
            push_delta(
                &mut deltas,
                input.surface,
                "registry-hash-drift",
                None,
                &current.hash,
                &report.registry_hash,
            );
        }
        compare_contracts(
            &mut deltas,
            input.surface,
            "registry-inventory-drift",
            &current.rows,
            &report.registry,
        );
        if report.required_commands != required {
            push_delta(
                &mut deltas,
                input.surface,
                "required-inventory-drift",
                None,
                &format_list(&required),
                &format_list(&report.required_commands),
            );
        }
        compare_exposure(
            &mut deltas,
            input.surface,
            current,
            &report.adapter_exposure,
        );
        if input.surface == Surface::Mcp && report.raw_transport_methods.is_empty() {
            push_delta(
                &mut deltas,
                input.surface,
                "missing-transport-evidence",
                None,
                "at least one retained MCP transport method",
                "none",
            );
        }
        if input.surface == Surface::Tui && report.ui_controls.is_empty() {
            push_delta(
                &mut deltas,
                input.surface,
                "missing-ui-control-evidence",
                None,
                "at least one retained TUI control acknowledgement",
                "none",
            );
        }
        for control in &report.ui_controls {
            if control.outcome != "ok" {
                push_delta(
                    &mut deltas,
                    input.surface,
                    "failed-ui-control",
                    Some(&control.control),
                    "ok",
                    &control.outcome,
                );
            }
        }
        append_streams(
            &mut raw_transport_methods,
            &mut ui_controls,
            input.surface,
            report,
        );
        add_cells(
            &mut cells,
            &mut deltas,
            input.surface,
            current,
            &required,
            &report.executions,
        );
    }

    cells.sort_by_key(|cell| (cell.surface, cell.command_name.clone()));
    deltas.sort_by_key(delta_key);
    CoverageMatrix {
        schema_version: MATRIX_SCHEMA_VERSION,
        result: if deltas.is_empty() {
            "passed".to_string()
        } else {
            "failed".to_string()
        },
        recipe_schema_version: RECIPE_SCHEMA_VERSION,
        source,
        required_commands: required,
        cells,
        deltas,
        raw_transport_methods,
        ui_controls,
    }
}

pub fn evaluate_files(root: &Path) -> CoverageMatrix {
    let inputs = REPORT_FILENAMES.map(|(surface, filename)| {
        let path = root.join(filename);
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<JourneyReport>(&bytes) {
                Ok(report) => ReportInput {
                    surface,
                    report: Some(report),
                    error: None,
                },
                Err(error) => ReportInput::failed(surface, format!("{}: {error}", path.display())),
            },
            Err(error) => ReportInput::failed(surface, format!("{}: {error}", path.display())),
        }
    });
    evaluate(inputs)
}

pub fn all_surfaces_tool_coverage_matrix(root: &Path) -> Result<CoverageMatrix, String> {
    let matrix = evaluate_files(root);
    write_json_atomic(&root.join(MATRIX_FILENAME), &matrix)?;
    if matrix.result == "passed" {
        Ok(matrix)
    } else {
        Err(format_delta_failure(&matrix))
    }
}

pub fn report_root() -> std::path::PathBuf {
    env::var_os("THREETERM_COVERAGE_EVIDENCE_ROOT")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            env::var_os("CARGO_TARGET_DIR")
                .map(|path| std::path::PathBuf::from(path).join("journey-coverage"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("target/journey-coverage"))
}

pub fn report_path(root: &Path, surface: Surface) -> std::path::PathBuf {
    REPORT_FILENAMES
        .iter()
        .find(|(candidate, _)| *candidate == surface)
        .map(|(_, filename)| root.join(filename))
        .expect("all coverage surfaces have a report filename")
}

pub fn remove_journey_report(root: &Path, surface: Surface) -> Result<(), String> {
    let path = report_path(root, surface);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| format!("{}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("{}: {error}", path.display()))
}

fn add_failed_cells(
    cells: &mut Vec<CoverageCell>,
    surface: Surface,
    current: &RegistrySnapshot,
    required: &[String],
) {
    for command_name in required {
        if let Some(contract) = current
            .rows
            .iter()
            .find(|row| row.command_name == *command_name)
        {
            cells.push(CoverageCell {
                surface,
                command_id: contract.command_id.clone(),
                command_name: command_name.clone(),
                command_schema_version: contract.command_schema_version.clone(),
                request_schema_version: contract.request_schema_version.clone(),
                response_schema_version: contract.response_schema_version.clone(),
                request_schema_hash: contract.request_schema_hash.clone(),
                response_schema_hash: contract.response_schema_hash.clone(),
                status: "failed".to_string(),
                execution_count: 0,
                executions: Vec::new(),
            });
        }
    }
}

fn add_cells(
    cells: &mut Vec<CoverageCell>,
    deltas: &mut Vec<CoverageDelta>,
    surface: Surface,
    current: &RegistrySnapshot,
    required: &[String],
    executions: &[ExecutionEvidence],
) {
    let mut seen_execution_ids = BTreeSet::new();
    for execution in executions {
        if !matches!(execution.outcome.as_str(), "ok" | "failed") {
            push_delta(
                deltas,
                surface,
                "invalid-execution-outcome",
                Some(&execution.command_name),
                "ok or failed",
                &execution.outcome,
            );
        }
        if !current
            .rows
            .iter()
            .any(|contract| contract.command_id == execution.command_id)
        {
            push_delta(
                deltas,
                surface,
                "unknown-execution-command",
                Some(&execution.command_name),
                "registered command",
                &execution.command_id,
            );
        }
        if !seen_execution_ids.insert(execution.evidence_id.as_str()) {
            push_delta(
                deltas,
                surface,
                "duplicate-execution-evidence",
                Some(&execution.command_name),
                "unique evidence id",
                &execution.evidence_id,
            );
        }
    }
    for command_name in required {
        let Some(contract) = current
            .rows
            .iter()
            .find(|row| row.command_name == *command_name)
        else {
            push_delta(
                deltas,
                surface,
                "required-command-unregistered",
                Some(command_name),
                "registered command",
                "missing",
            );
            continue;
        };
        let matching = executions
            .iter()
            .filter(|execution| execution.command_id == contract.command_id)
            .cloned()
            .collect::<Vec<_>>();
        let successful = matching
            .iter()
            .filter(|execution| execution.outcome == "ok")
            .filter(|execution| execution_matches(execution, contract))
            .cloned()
            .collect::<Vec<_>>();
        for execution in &matching {
            if execution.outcome != "ok" && successful.is_empty() {
                push_delta(
                    deltas,
                    surface,
                    "failed-execution",
                    Some(command_name),
                    "ok",
                    &execution.outcome,
                );
            } else if !execution_matches(execution, contract) {
                push_delta(
                    deltas,
                    surface,
                    if execution.command_schema_version == contract.command_schema_version {
                        "execution-schema-drift"
                    } else {
                        "execution-version-drift"
                    },
                    Some(command_name),
                    &contract.command_schema_version,
                    &execution.command_schema_version,
                );
            }
        }
        if successful.is_empty() {
            push_delta(
                deltas,
                surface,
                "missing-execution",
                Some(command_name),
                "successful execution",
                if matching.is_empty() {
                    "none"
                } else {
                    "no matching successful execution"
                },
            );
        }
        cells.push(CoverageCell {
            surface,
            command_id: contract.command_id.clone(),
            command_name: command_name.clone(),
            command_schema_version: contract.command_schema_version.clone(),
            request_schema_version: contract.request_schema_version.clone(),
            response_schema_version: contract.response_schema_version.clone(),
            request_schema_hash: contract.request_schema_hash.clone(),
            response_schema_hash: contract.response_schema_hash.clone(),
            status: if successful.is_empty() {
                "failed".to_string()
            } else {
                "passed".to_string()
            },
            execution_count: successful.len(),
            executions: successful,
        });
    }
}

fn compare_exposure(
    deltas: &mut Vec<CoverageDelta>,
    surface: Surface,
    current: &RegistrySnapshot,
    exposure: &[CommandContract],
) {
    let mut by_id = BTreeMap::<&str, &CommandContract>::new();
    for row in exposure {
        if by_id.insert(&row.command_id, row).is_some() {
            push_delta(
                deltas,
                surface,
                "duplicate-adapter-exposure",
                Some(&row.command_name),
                "one exposure row",
                "duplicate exposure rows",
            );
        }
    }
    let current_ids = current
        .rows
        .iter()
        .map(|row| row.command_id.as_str())
        .collect::<BTreeSet<_>>();
    for row in exposure {
        if !current_ids.contains(row.command_id.as_str()) {
            push_delta(
                deltas,
                surface,
                "unsupported-advertised-tool",
                Some(&row.command_name),
                "registered command",
                &row.command_id,
            );
        }
    }
    for expected in &current.rows {
        let Some(actual) = by_id.get(expected.command_id.as_str()) else {
            push_delta(
                deltas,
                surface,
                "missing-adapter-exposure",
                Some(&expected.command_name),
                &expected.command_schema_version,
                "not advertised",
            );
            continue;
        };
        if *actual != expected {
            push_delta(
                deltas,
                surface,
                "adapter-schema-drift",
                Some(&expected.command_name),
                &compact(expected),
                &compact(actual),
            );
        }
    }
}

fn compare_contracts(
    deltas: &mut Vec<CoverageDelta>,
    surface: Surface,
    kind: &str,
    expected: &[CommandContract],
    actual: &[CommandContract],
) {
    let mut seen = BTreeSet::new();
    for row in actual {
        if !seen.insert(row.command_id.as_str()) {
            push_delta(
                deltas,
                surface,
                "duplicate-registry-row",
                Some(&row.command_name),
                "one registry row",
                &row.command_id,
            );
        }
    }
    let expected_by_id = expected
        .iter()
        .map(|row| (row.command_id.as_str(), row))
        .collect::<BTreeMap<_, _>>();
    let actual_by_id = actual
        .iter()
        .map(|row| (row.command_id.as_str(), row))
        .collect::<BTreeMap<_, _>>();
    for row in actual {
        if !expected_by_id.contains_key(row.command_id.as_str()) {
            push_delta(
                deltas,
                surface,
                kind,
                Some(&row.command_name),
                "no extra registry row",
                &compact(row),
            );
        }
    }
    for row in expected {
        match actual_by_id.get(row.command_id.as_str()) {
            None => push_delta(
                deltas,
                surface,
                kind,
                Some(&row.command_name),
                &compact(row),
                "missing",
            ),
            Some(actual) if *actual != row => push_delta(
                deltas,
                surface,
                "schema-drift",
                Some(&row.command_name),
                &compact(row),
                &compact(actual),
            ),
            Some(_) => {}
        }
    }
}

fn execution_matches(execution: &ExecutionEvidence, contract: &CommandContract) -> bool {
    execution.command_name == contract.command_name
        && execution.command_schema_version == contract.command_schema_version
        && execution.request_schema_version == contract.request_schema_version
        && execution.response_schema_version == contract.response_schema_version
        && execution.request_schema_hash == contract.request_schema_hash
        && execution.response_schema_hash == contract.response_schema_hash
        && execution.response_payload_hash.len() == 64
        && execution
            .response_payload_hash
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

fn format_delta_failure(matrix: &CoverageMatrix) -> String {
    matrix
        .deltas
        .iter()
        .map(|delta| {
            format!(
                "{}:{}:{} expected={} actual={}",
                delta.surface.map(Surface::as_str).unwrap_or("global"),
                delta.kind,
                delta.command.as_deref().unwrap_or("-"),
                delta.expected,
                delta.actual
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn append_streams(
    transports: &mut Vec<SurfaceTransportEvidence>,
    controls: &mut Vec<SurfaceUiControlEvidence>,
    surface: Surface,
    report: &JourneyReport,
) {
    transports.push(SurfaceTransportEvidence {
        surface,
        methods: report.raw_transport_methods.clone(),
    });
    controls.push(SurfaceUiControlEvidence {
        surface,
        controls: report.ui_controls.clone(),
    });
}

fn push_delta(
    deltas: &mut Vec<CoverageDelta>,
    surface: Surface,
    kind: &str,
    command: Option<&str>,
    expected: &str,
    actual: &str,
) {
    deltas.push(CoverageDelta {
        surface: Some(surface),
        kind: kind.to_string(),
        command: command.map(str::to_string),
        expected: expected.to_string(),
        actual: actual.to_string(),
    });
}

fn delta_key(delta: &CoverageDelta) -> (String, String, String, String, String) {
    (
        delta
            .surface
            .map(Surface::as_str)
            .unwrap_or("global")
            .to_string(),
        delta.kind.clone(),
        delta.command.clone().unwrap_or_default(),
        delta.expected.clone(),
        delta.actual.clone(),
    )
}

fn compact<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("coverage value serializes")
}

fn format_list(values: &[String]) -> String {
    compact(values)
}

fn format_source(source: &SourceIdentity) -> String {
    compact(source)
}

fn git_output(repo_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(|error| format!("git {:?} failed to start: {error}", args))?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} failed with {}: {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_dirty(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!(
            "THREETERM_SOURCE_DIRTY must be true or false, got {value:?}"
        )),
    }
}

fn validate_commit(commit: &str) -> Result<(), String> {
    if valid_commit(commit) {
        Ok(())
    } else {
        Err(format!(
            "source commit must be 40-64 lowercase hexadecimal characters, got {commit:?}"
        ))
    }
}

fn valid_commit(commit: &str) -> bool {
    (40..=64).contains(&commit.len())
        && commit
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_hash_is_stable_for_compact_json() {
        assert_eq!(
            schema_hash(&json!({"b": 2, "a": 1})),
            schema_hash(&json!({"a": 1, "b": 2}))
        );
    }

    #[test]
    fn source_commit_rejects_uppercase_and_short_values() {
        assert!(!valid_commit("ABCDEF"));
        assert!(!valid_commit("0123456789abcdef0123456789abcdef0123456"));
        assert!(valid_commit("0123456789abcdef0123456789abcdef01234567"));
    }
}

use std::fs;
use std::path::PathBuf;
use threeterm_protocol::coverage::{
    ExecutionEvidence, JourneyReport, RawTransportMethod, ReportInput, SourceIdentity, Surface,
    UiControlEvidence, all_surfaces_tool_coverage_matrix as aggregate, evaluate, report_path,
    required_commands, write_json_atomic,
};

const EXPECTED_REQUIRED_COMMANDS: &[&str] = &[
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

fn complete_report(surface: Surface) -> JourneyReport {
    let registry = threeterm_protocol::coverage::current_registry();
    let executions = EXPECTED_REQUIRED_COMMANDS
        .iter()
        .enumerate()
        .map(|(step_index, command_name)| {
            let contract = registry
                .rows
                .iter()
                .find(|row| row.command_name == *command_name)
                .expect("required command is registered");
            ExecutionEvidence {
                command_id: contract.command_id.clone(),
                command_name: contract.command_name.clone(),
                command_schema_version: contract.command_schema_version.clone(),
                request_schema_version: contract.request_schema_version.clone(),
                response_schema_version: contract.response_schema_version.clone(),
                request_schema_hash: contract.request_schema_hash.clone(),
                response_schema_hash: contract.response_schema_hash.clone(),
                response_payload_hash: "0".repeat(64),
                outcome: "ok".to_string(),
                step_index: Some(step_index as u32),
                evidence_id: format!("{surface:?}-{command_name}"),
            }
        })
        .collect();

    JourneyReport {
        schema_version: "threeterm.coverage.journey-report/1".to_string(),
        surface,
        test: match surface {
            Surface::Api => "e2e_stl_api_all_tools_l_bracket",
            Surface::Mcp => "e2e_stl_mcp_all_tools_l_bracket",
            Surface::Tui => "production_tui_all_tools_stl_journey",
        }
        .to_string(),
        recipe_schema_version: "threeterm.recipe.bracket-complete/1".to_string(),
        source: SourceIdentity {
            commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            dirty: false,
        },
        result: "passed".to_string(),
        registry_hash: registry.hash,
        registry: registry.rows.clone(),
        required_commands: EXPECTED_REQUIRED_COMMANDS
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        adapter_exposure: registry.rows,
        executions,
        raw_transport_methods: if surface == Surface::Mcp {
            vec![RawTransportMethod {
                direction: "request".to_string(),
                method: "tools/list".to_string(),
                id: Some("fixture-list".to_string()),
                correlation_id: Some("fixture-list".to_string()),
            }]
        } else {
            Vec::new()
        },
        ui_controls: if surface == Surface::Tui {
            vec![UiControlEvidence {
                control: "preview:fixture".to_string(),
                outcome: "ok".to_string(),
                detail: "fixture acknowledgement".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

#[test]
fn complete_reports_produce_a_required_command_matrix() {
    let matrix = evaluate([
        ReportInput::complete(complete_report(Surface::Api)),
        ReportInput::complete(complete_report(Surface::Mcp)),
        ReportInput::complete(complete_report(Surface::Tui)),
    ]);

    assert_eq!(matrix.result, "passed");
    assert!(matrix.deltas.is_empty());
    assert_eq!(required_commands(), EXPECTED_REQUIRED_COMMANDS);
    assert_eq!(matrix.required_commands, expected_commands());
    assert_eq!(matrix.cells.len(), EXPECTED_REQUIRED_COMMANDS.len() * 3);
    assert!(matrix.cells.iter().all(|cell| cell.status == "passed"));
}

#[test]
fn evaluator_reports_missing_execution_and_keeps_non_domain_evidence_separate() {
    let mut report = complete_report(Surface::Api);
    report
        .executions
        .retain(|execution| execution.command_name != "export");
    report.raw_transport_methods.push(RawTransportMethod {
        direction: "request".to_string(),
        method: "tools/call".to_string(),
        id: Some("transport-only".to_string()),
        correlation_id: None,
    });
    report.ui_controls.push(UiControlEvidence {
        control: "palette-select:export".to_string(),
        outcome: "ok".to_string(),
        detail: "UI control is not domain execution".to_string(),
    });
    let matrix = evaluate([
        ReportInput::complete(report),
        ReportInput::complete(complete_report(Surface::Mcp)),
        ReportInput::complete(complete_report(Surface::Tui)),
    ]);

    assert_eq!(matrix.result, "failed");
    assert!(matrix.deltas.iter().any(|delta| {
        delta.surface == Some(Surface::Api)
            && delta.kind == "missing-execution"
            && delta.command.as_deref() == Some("export")
    }));
    assert_eq!(matrix.raw_transport_methods[0].methods.len(), 1);
    assert_eq!(matrix.ui_controls[0].controls.len(), 1);
    assert!(matrix.cells.iter().all(|cell| {
        cell.command_name != "export" || cell.surface != Surface::Api || cell.status == "failed"
    }));
}

#[test]
fn evaluator_allows_a_failed_attempt_when_a_later_attempt_succeeds() {
    let mut report = complete_report(Surface::Api);
    let mut failed = report
        .executions
        .iter()
        .find(|execution| execution.command_name == "extrude")
        .expect("complete report includes extrude")
        .clone();
    failed.outcome = "failed".to_string();
    failed.evidence_id = "api-failed-extrude".to_string();
    report.executions.push(failed);

    let matrix = evaluate([
        ReportInput::complete(report),
        ReportInput::complete(complete_report(Surface::Mcp)),
        ReportInput::complete(complete_report(Surface::Tui)),
    ]);

    assert_eq!(matrix.result, "passed");
    assert!(!matrix.deltas.iter().any(|delta| {
        delta.surface == Some(Surface::Api)
            && delta.kind == "failed-execution"
            && delta.command.as_deref() == Some("extrude")
    }));
    let extrude = matrix
        .cells
        .iter()
        .find(|cell| cell.surface == Surface::Api && cell.command_name == "extrude")
        .expect("extrude cell exists");
    assert_eq!(extrude.execution_count, 1);
    assert_eq!(extrude.status, "passed");
}

#[test]
fn evaluator_requires_mcp_transport_and_tui_control_evidence() {
    let mut mcp = complete_report(Surface::Mcp);
    mcp.raw_transport_methods.clear();
    let mut tui = complete_report(Surface::Tui);
    tui.ui_controls.clear();

    let matrix = evaluate([
        ReportInput::complete(complete_report(Surface::Api)),
        ReportInput::complete(mcp),
        ReportInput::complete(tui),
    ]);

    assert!(matrix.deltas.iter().any(|delta| {
        delta.kind == "missing-transport-evidence" && delta.surface == Some(Surface::Mcp)
    }));
    assert!(matrix.deltas.iter().any(|delta| {
        delta.kind == "missing-ui-control-evidence" && delta.surface == Some(Surface::Tui)
    }));
}

#[test]
fn evaluator_rejects_failed_tui_control_evidence() {
    let mut tui = complete_report(Surface::Tui);
    tui.ui_controls[0].outcome = "failed".to_string();

    let matrix = evaluate([
        ReportInput::complete(complete_report(Surface::Api)),
        ReportInput::complete(complete_report(Surface::Mcp)),
        ReportInput::complete(tui),
    ]);

    assert_eq!(matrix.result, "failed");
    assert!(
        matrix.deltas.iter().any(|delta| {
            delta.kind == "failed-ui-control" && delta.surface == Some(Surface::Tui)
        })
    );
}

#[test]
fn evaluator_identifies_advertised_inventory_and_schema_drift() {
    let mut report = complete_report(Surface::Mcp);
    report.adapter_exposure.pop();
    report.adapter_exposure[0].request_schema_hash = "drifted".to_string();
    report
        .adapter_exposure
        .push(report.adapter_exposure[0].clone());
    report
        .adapter_exposure
        .push(threeterm_protocol::coverage::CommandContract {
            command_id: "unsupported-command".to_string(),
            command_name: "unsupported".to_string(),
            command_schema_version: "unsupported/1".to_string(),
            request_schema_version: "unsupported.request/1".to_string(),
            response_schema_version: "unsupported.response/1".to_string(),
            request_schema_hash: "request".to_string(),
            response_schema_hash: "response".to_string(),
        });
    let matrix = evaluate([
        ReportInput::complete(complete_report(Surface::Api)),
        ReportInput::complete(report),
        ReportInput::complete(complete_report(Surface::Tui)),
    ]);

    assert_eq!(matrix.result, "failed");
    assert!(matrix.deltas.iter().any(|delta| {
        delta.kind == "missing-adapter-exposure" || delta.kind == "adapter-schema-drift"
    }));
    assert!(
        matrix
            .deltas
            .iter()
            .any(|delta| delta.kind == "unsupported-advertised-tool")
    );
    assert!(
        matrix
            .deltas
            .iter()
            .any(|delta| delta.kind == "duplicate-adapter-exposure")
    );
}

#[test]
fn evaluator_identifies_registry_execution_and_required_inventory_drift() {
    let mut api = complete_report(Surface::Api);
    api.registry_hash = "drifted-registry-hash".to_string();
    api.registry[0].command_schema_version = "drifted-command-version".to_string();
    api.required_commands.pop();

    let mut mcp = complete_report(Surface::Mcp);
    mcp.executions[0].command_schema_version = "drifted-execution-version".to_string();
    mcp.executions[1].request_schema_hash = "drifted-request-schema".to_string();

    let mut tui = complete_report(Surface::Tui);
    tui.adapter_exposure[0].response_schema_version = "drifted-response-version".to_string();

    let matrix = evaluate([
        ReportInput::complete(api),
        ReportInput::complete(mcp),
        ReportInput::complete(tui),
    ]);

    assert_eq!(matrix.result, "failed");
    assert_delta(&matrix, Surface::Api, "registry-hash-drift");
    assert_delta(&matrix, Surface::Api, "schema-drift");
    assert_delta(&matrix, Surface::Api, "required-inventory-drift");
    assert_delta(&matrix, Surface::Mcp, "execution-version-drift");
    assert_delta(&matrix, Surface::Mcp, "execution-schema-drift");
    assert_delta(&matrix, Surface::Tui, "adapter-schema-drift");
}

#[test]
fn evaluator_identifies_a_wrong_journey_test_for_a_surface() {
    let mut report = complete_report(Surface::Api);
    report.test = "different-journey".to_string();

    let matrix = evaluate([
        ReportInput::complete(report),
        ReportInput::complete(complete_report(Surface::Mcp)),
        ReportInput::complete(complete_report(Surface::Tui)),
    ]);

    assert!(matrix.deltas.iter().any(|delta| {
        delta.kind == "journey-test-mismatch"
            && delta.surface == Some(Surface::Api)
            && delta.expected == "e2e_stl_api_all_tools_l_bracket"
            && delta.actual == "different-journey"
    }));
}

#[test]
fn filesystem_aggregate_writes_matrix_even_when_reports_are_missing() {
    let root = temporary_root("missing");
    fs::create_dir_all(&root).expect("coverage root creates");
    let error = aggregate(&root).expect_err("missing reports fail closed");
    assert!(error.contains("missing-report"));
    let matrix: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("journey-coverage-matrix.json")).expect("matrix is retained"),
    )
    .expect("matrix is JSON");
    assert_eq!(matrix["result"], "failed");
    fs::remove_dir_all(root).expect("temporary coverage root cleans");
}

#[test]
fn filesystem_aggregate_consumes_all_three_retained_reports() {
    let root = temporary_root("complete");
    fs::create_dir_all(&root).expect("coverage root creates");
    for surface in [Surface::Api, Surface::Mcp, Surface::Tui] {
        let report = complete_report(surface);
        write_json_atomic(&report_path(&root, surface), &report).expect("journey report writes");
    }

    let matrix = aggregate(&root).expect("complete reports pass the aggregate");
    assert_eq!(matrix.result, "passed");
    assert_eq!(matrix.cells.len(), EXPECTED_REQUIRED_COMMANDS.len() * 3);
    assert!(root.join("journey-coverage-matrix.json").is_file());
    fs::remove_dir_all(root).expect("temporary coverage root cleans");
}

#[test]
fn filesystem_aggregate_fails_closed_for_invalid_retained_reports() {
    assert_filesystem_failure("malformed", "missing-report", |root| {
        fs::write(report_path(root, Surface::Api), b"{").expect("malformed report writes");
    });
    assert_filesystem_failure("incomplete", "missing-execution", |root| {
        let mut report = complete_report(Surface::Api);
        report.executions.clear();
        write_json_atomic(&report_path(root, Surface::Api), &report)
            .expect("incomplete report writes");
    });
    assert_filesystem_failure("failed", "journey-failed", |root| {
        let mut report = complete_report(Surface::Api);
        report.result = "failed".to_string();
        write_json_atomic(&report_path(root, Surface::Api), &report).expect("failed report writes");
    });
    assert_filesystem_failure("recipe", "recipe-version-drift", |root| {
        let mut report = complete_report(Surface::Api);
        report.recipe_schema_version = "threeterm.recipe.other/1".to_string();
        write_json_atomic(&report_path(root, Surface::Api), &report)
            .expect("recipe-drifted report writes");
    });
    assert_filesystem_failure("source", "source-mismatch", |root| {
        let mut report = complete_report(Surface::Api);
        report.source.commit = "fedcba9876543210fedcba9876543210fedcba98".to_string();
        write_json_atomic(&report_path(root, Surface::Api), &report)
            .expect("source-mismatched report writes");
    });
}

#[test]
fn filesystem_aggregate_rejects_reports_swapped_between_surface_paths() {
    let root = temporary_root("swapped");
    fs::create_dir_all(&root).expect("coverage root creates");
    let api = complete_report(Surface::Api);
    let mcp = complete_report(Surface::Mcp);
    write_json_atomic(&report_path(&root, Surface::Api), &mcp).expect("swapped API report writes");
    write_json_atomic(&report_path(&root, Surface::Mcp), &api).expect("swapped MCP report writes");
    write_json_atomic(
        &report_path(&root, Surface::Tui),
        &complete_report(Surface::Tui),
    )
    .expect("TUI report writes");

    let matrix = aggregate(&root).expect_err("swapped reports fail the aggregate");
    assert!(matrix.contains("surface-mismatch"));
    fs::remove_dir_all(root).expect("temporary coverage root cleans");
}

#[test]
#[ignore = "requires the three retained production journey reports"]
fn all_surfaces_tool_coverage_matrix() {
    let root = threeterm_protocol::coverage::report_root();
    let matrix = aggregate(&root)
        .unwrap_or_else(|error| panic!("all-surface tool coverage failed: {error}"));
    assert_eq!(matrix.result, "passed");
}

fn temporary_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("threeterm-coverage-{name}-{}", std::process::id()))
}

fn expected_commands() -> Vec<String> {
    EXPECTED_REQUIRED_COMMANDS
        .iter()
        .map(|command| (*command).to_string())
        .collect()
}

fn assert_delta(
    matrix: &threeterm_protocol::coverage::CoverageMatrix,
    surface: Surface,
    kind: &str,
) {
    assert!(
        matrix
            .deltas
            .iter()
            .any(|delta| delta.surface == Some(surface) && delta.kind == kind),
        "missing {surface:?} {kind} delta: {:?}",
        matrix.deltas
    );
}

fn assert_filesystem_failure(
    name: &str,
    expected_delta: &str,
    mutate: impl FnOnce(&std::path::Path),
) {
    let root = temporary_root(name);
    fs::create_dir_all(&root).expect("coverage root creates");
    for surface in [Surface::Api, Surface::Mcp, Surface::Tui] {
        let report = complete_report(surface);
        write_json_atomic(&report_path(&root, surface), &report).expect("journey report writes");
    }
    mutate(&root);

    let error = aggregate(&root).expect_err("invalid reports fail the aggregate");
    assert!(error.contains(expected_delta), "{error}");
    let matrix: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("journey-coverage-matrix.json")).expect("matrix is retained"),
    )
    .expect("matrix is JSON");
    assert_eq!(matrix["result"], "failed");
    fs::remove_dir_all(root).expect("temporary coverage root cleans");
}

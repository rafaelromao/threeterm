//! Structured validation for one selected current solid through the
//! public versioned domain-command API. A known-good solid validates
//! after reopen with its verdict bound to the selected feature plus
//! revision; fatal, stale, missing, and unselected geometry is refused
//! with structured diagnostics before any export output exists.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::{
    Host, HostError, domain_command_diagnostic, domain_command_failure_value,
    domain_execution_diagnostic,
};
use threeterm_persistence::Bundle;
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::diagnostic::DiagnosticCode;
use threeterm_protocol::schema::{
    EXTRUDE_COMMAND_ID, LOAD_COMMAND_ID, NEW_PROJECT_COMMAND_ID, SAVE_COMMAND_ID,
    TIMELINE_COMMAND_ID, VALIDATE_COMMAND_ID,
};
use threeterm_protocol::schema_validator::validate as validate_schema;

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-validate-solid-{label}-{suffix}"))
}

fn locate_worker() -> Option<threeterm_occt_worker::OcctWorker> {
    threeterm_occt_worker::OcctWorker::locate().ok()
}

fn required_fixture_worker(test_name: &str) -> Option<threeterm_occt_worker::OcctWorker> {
    let worker = locate_worker();
    if worker.is_none() && std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() {
        panic!("{test_name}: OCCT worker is required in this environment");
    }
    worker
}

fn command_response(
    host: &Host,
    command: threeterm_protocol::schema::CommandId,
    request: Value,
) -> Value {
    let schema = threeterm_protocol::schema::find(command)
        .unwrap_or_else(|| panic!("command {} is registered", command.0));
    validate_schema(&schema.request_schema, &request)
        .unwrap_or_else(|error| panic!("request for {} violates its schema: {error}", command.0));
    let response = host
        .execute_domain_command(command, request)
        .unwrap_or_else(|error| panic!("command {} failed: {error:?}", command.0));
    validate_schema(&schema.response_schema, &response)
        .unwrap_or_else(|error| panic!("response for {} violates its schema: {error}", command.0));
    response
}

fn new_project(host: &Host, parent: &std::path::Path) -> std::path::PathBuf {
    let destination = parent.join("project");
    command_response(
        host,
        NEW_PROJECT_COMMAND_ID,
        json!({"destination": destination.to_string_lossy()}),
    );
    destination
}

#[test]
fn validate_unknown_feature_reports_reference_lost_through_the_dispatcher() {
    let parent = root("unknown-feature");
    let host = Host::new();
    let bundle = new_project(&host, &parent);
    let manifest = std::fs::read(bundle.join("manifest.json")).expect("manifest reads");
    let log = std::fs::read(bundle.join("transactions.log")).expect("log reads");

    let error = Host::new()
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "missing-solid",
            }),
        )
        .expect_err("unknown selected feature is refused");
    let ExecutionError::Handler(HostError::Validation { detail }) = error else {
        panic!("unknown feature must report Validation, got {error:?}");
    };
    assert!(
        detail.contains("reference is lost"),
        "refusal names the lost reference: {detail}"
    );
    assert!(
        detail.contains("missing-solid"),
        "refusal names the selected feature: {detail}"
    );
    let diagnostic = domain_execution_diagnostic(&ExecutionError::Handler(HostError::Validation {
        detail: detail.clone(),
    }));
    assert_eq!(diagnostic.code, DiagnosticCode::InvalidRequest);
    assert_eq!(
        std::fs::read(bundle.join("manifest.json")).expect("manifest re-reads"),
        manifest,
        "refusal leaves the manifest untouched"
    );
    assert_eq!(
        std::fs::read(bundle.join("transactions.log")).expect("log re-reads"),
        log,
        "refusal appends no transaction"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn validate_empty_feature_id_is_rejected_by_the_request_contract() {
    let parent = root("empty-feature");
    let host = Host::new();
    let bundle = new_project(&host, &parent);
    let manifest = std::fs::read(bundle.join("manifest.json")).expect("manifest reads");
    let log = std::fs::read(bundle.join("transactions.log")).expect("log reads");

    let error = host
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "",
            }),
        )
        .expect_err("empty selection is never treated as valid");
    assert!(
        matches!(error, ExecutionError::InvalidRequest(_)),
        "empty feature id fails request validation, got {error:?}"
    );
    assert_eq!(
        std::fs::read(bundle.join("manifest.json")).expect("manifest re-reads"),
        manifest,
        "rejection leaves the manifest untouched"
    );
    assert_eq!(
        std::fs::read(bundle.join("transactions.log")).expect("log re-reads"),
        log,
        "rejection appends no transaction"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn validate_missing_committed_brep_reports_persistence_failure_without_replay() {
    let parent = root("missing-brep");
    let bundle = parent.join("project");
    Bundle::create_for_test(&bundle, &"00".repeat(16)).expect("bundle creates");
    let staging = parent.join("staging");
    std::fs::create_dir_all(&staging).expect("staging dir creates");
    let brep_source = staging.join("box-1.brep");
    std::fs::write(&brep_source, b"staged validate bytes").expect("staging BREP writes");
    Host::new()
        .commit_brep_feature(&bundle, "box-1", &brep_source)
        .expect("BREP feature commits");
    let manifest = std::fs::read(bundle.join("manifest.json")).expect("manifest reads");
    let log = std::fs::read(bundle.join("transactions.log")).expect("log reads");

    // The committed BREP disappears after the last load with no second
    // load or replay before validation, so healing cannot mask the loss.
    std::fs::remove_file(bundle.join("brep/box-1.brep")).expect("committed BREP deletes");

    let error = Host::new()
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "box-1",
            }),
        )
        .expect_err("missing current geometry is refused");
    let ExecutionError::Handler(HostError::BrepFileMissing { path }) = error else {
        panic!("missing BREP must report BrepFileMissing, got {error:?}");
    };
    assert!(
        path.to_string_lossy().contains("box-1.brep"),
        "refusal names the missing BREP: {}",
        path.display()
    );
    let diagnostic = domain_command_diagnostic(&HostError::BrepFileMissing { path });
    assert_eq!(diagnostic.code, DiagnosticCode::PersistenceFailure);
    assert_eq!(
        std::fs::read(bundle.join("manifest.json")).expect("manifest re-reads"),
        manifest,
        "refusal leaves the manifest untouched"
    );
    assert_eq!(
        std::fs::read(bundle.join("transactions.log")).expect("log re-reads"),
        log,
        "refusal appends no transaction"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn validate_stale_family_reports_stale_geometry_before_any_output() {
    let parent = root("stale-family");
    let bundle = parent.join("project");
    let output = parent.join("output");
    let host = Host::new();
    host.save_bracket(&bundle, "l-bracket", 60.0, 30.0, 40.0, 3.0)
        .expect("history initializes");
    host.historical_edit(&bundle, "l-bracket-base", "length", 0.0)
        .expect("failing historical edit is committed");
    let manifest = std::fs::read(bundle.join("manifest.json")).expect("manifest reads");
    let log = std::fs::read(bundle.join("transactions.log")).expect("log reads");

    let error = Host::new()
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "l-bracket",
            }),
        )
        .expect_err("stale family is refused before any output");
    let ExecutionError::Handler(HostError::StaleLastValidGeometry {
        feature_id,
        active_revision,
        stale_features,
    }) = error
    else {
        panic!("stale family must report StaleLastValidGeometry, got {error:?}");
    };
    assert_eq!(feature_id, "l-bracket");
    assert!(
        active_revision.starts_with("history-revision-"),
        "refusal binds the active revision: {active_revision}"
    );
    assert_eq!(
        stale_features
            .iter()
            .map(|feature| feature.feature_id.as_str())
            .collect::<Vec<_>>(),
        ["l-bracket-base", "l-bracket-bend", "l-bracket-finish"]
    );
    let failure = domain_command_failure_value(&HostError::StaleLastValidGeometry {
        feature_id: feature_id.clone(),
        active_revision: active_revision.clone(),
        stale_features: stale_features.clone(),
    });
    assert_eq!(failure["code"], "stale_last_valid_geometry");
    assert_eq!(failure["feature_id"], "l-bracket");
    assert!(!output.exists(), "refusal produces no export output");
    assert_eq!(
        std::fs::read(bundle.join("manifest.json")).expect("manifest re-reads"),
        manifest
    );
    assert_eq!(
        std::fs::read(bundle.join("transactions.log")).expect("log re-reads"),
        log
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn validate_known_good_solid_reports_bound_success_after_reopen() {
    if required_fixture_worker("validate_known_good_solid_reports_bound_success_after_reopen")
        .is_none()
    {
        eprintln!("validate: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let parent = root("known-good");
    let host = Host::new();
    let bundle = new_project(&host, &parent);
    // One L-bracket arm from the qualified complete recipe: real kernel
    // geometry committed through the public dispatcher.
    let mut extrude = json!({
        "feature_id": "arm-x",
        "profile": [[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 20.0]],
        "height": 8.0,
        "mode": "additive",
    });
    extrude["bundle_path"] = bundle.to_string_lossy().into_owned().into();
    let extruded = command_response(&host, EXTRUDE_COMMAND_ID, extrude);
    assert_eq!(extruded["status"], "ok");
    command_response(
        &host,
        SAVE_COMMAND_ID,
        json!({
            "bundle_path": bundle.to_string_lossy(),
            "feature_id": "validate-snapshot",
            "kind": "checkpoint",
            "expected_revision": extruded["revision_hash"],
        }),
    );

    // Reopen in a fresh host so validation reads durable state only.
    let reopened = Host::new();
    let loaded = command_response(
        &reopened,
        LOAD_COMMAND_ID,
        json!({"bundle_path": bundle.to_string_lossy()}),
    );
    let timeline = command_response(
        &reopened,
        TIMELINE_COMMAND_ID,
        json!({"bundle_path": bundle.to_string_lossy()}),
    );
    let response = command_response(
        &reopened,
        VALIDATE_COMMAND_ID,
        json!({
            "bundle_path": bundle.to_string_lossy(),
            "feature_id": "arm-x",
        }),
    );
    assert_eq!(response["status"], "ok");
    assert_eq!(response["valid"], true);
    assert_eq!(response["feature_id"], "arm-x");
    assert_eq!(response["revision_id"], timeline["active_revision"]);
    assert_eq!(response["feature_graph_hash"], loaded["feature_graph_hash"]);
    assert_eq!(response["revision_hash"], loaded["revision_hash"]);
    assert!(
        !parent.join("output").exists(),
        "validation produces no export artifact"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn validate_rejects_a_kernel_valid_brep_with_wrong_authenticated_provenance() {
    if required_fixture_worker(
        "validate_rejects_a_kernel_valid_brep_with_wrong_authenticated_provenance",
    )
    .is_none()
    {
        eprintln!("validate: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let parent = root("wrong-provenance");
    let host = Host::new();
    let bundle = new_project(&host, &parent);
    for (feature_id, profile, height) in [
        (
            "arm-x",
            vec![[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 20.0]],
            8.0,
        ),
        (
            "other-solid",
            vec![[0.0, 0.0], [35.0, 0.0], [35.0, 12.0], [0.0, 12.0]],
            5.0,
        ),
    ] {
        let mut extrude = json!({
            "feature_id": feature_id,
            "profile": profile,
            "height": height,
            "mode": "additive",
        });
        extrude["bundle_path"] = bundle.to_string_lossy().into_owned().into();
        assert_eq!(
            command_response(&host, EXTRUDE_COMMAND_ID, extrude)["status"],
            "ok"
        );
    }

    let reopened = Host::new();
    command_response(
        &reopened,
        LOAD_COMMAND_ID,
        json!({"bundle_path": bundle.to_string_lossy()}),
    );
    let replacement =
        std::fs::read(bundle.join("brep/other-solid.brep")).expect("replacement BREP reads");
    std::fs::write(bundle.join("brep/arm-x.brep"), replacement).expect("replacement BREP writes");

    let error = reopened
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "arm-x",
            }),
        )
        .expect_err("a valid but wrong BREP is refused");
    let ExecutionError::Handler(HostError::BrepInvalid { detail, .. }) = error else {
        panic!("wrong BREP must report BrepInvalid, got {error:?}");
    };
    assert!(
        detail.contains("authenticated BREP provenance mismatch") && detail.contains("arm-x"),
        "refusal identifies the provenance mismatch: {detail}"
    );
    assert!(
        !parent.join("output").exists(),
        "validation produces no export artifact"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn validate_corrupt_committed_brep_is_refused_as_brep_invalid() {
    if required_fixture_worker("validate_corrupt_committed_brep_is_refused_as_brep_invalid")
        .is_none()
    {
        eprintln!("validate: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let parent = root("corrupt-brep");
    let output = parent.join("output");
    let host = Host::new();
    let bundle = new_project(&host, &parent);
    let mut extrude = json!({
        "feature_id": "arm-x",
        "profile": [[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 20.0]],
        "height": 8.0,
        "mode": "additive",
    });
    extrude["bundle_path"] = bundle.to_string_lossy().into_owned().into();
    let extruded = command_response(&host, EXTRUDE_COMMAND_ID, extrude);
    assert_eq!(extruded["status"], "ok");
    let reopened = Host::new();
    let loaded = command_response(
        &reopened,
        LOAD_COMMAND_ID,
        json!({"bundle_path": bundle.to_string_lossy()}),
    );
    assert_eq!(loaded["status"], "ok");

    // Corrupt the committed bytes after the reopen load with no second
    // load before validation: digit-only distortion keeps the BREP
    // framing parseable while the kernel check must fail the shape.
    let brep_path = bundle.join("brep/arm-x.brep");
    let bytes = std::fs::read(&brep_path).expect("committed BREP reads");
    let midpoint = bytes.len() / 2;
    let window = &bytes[midpoint..(midpoint + bytes.len() / 4).min(bytes.len())];
    assert!(
        window.iter().any(|byte| byte.is_ascii_digit()),
        "committed BREP carries numeric geometry to distort"
    );
    let corrupted: Vec<u8> = bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if index >= midpoint
                && index < (midpoint + bytes.len() / 4).min(bytes.len())
                && byte.is_ascii_digit()
            {
                b'7'
            } else {
                *byte
            }
        })
        .collect();
    assert_ne!(corrupted, bytes);
    std::fs::write(&brep_path, &corrupted).expect("corrupted BREP writes");
    let manifest = std::fs::read(bundle.join("manifest.json")).expect("manifest reads");
    let log = std::fs::read(bundle.join("transactions.log")).expect("log reads");

    let error = Host::new()
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "arm-x",
            }),
        )
        .expect_err("fatal current-geometry defect is refused");
    let ExecutionError::Handler(HostError::BrepInvalid { detail, .. }) = error else {
        panic!("corrupt BREP must report BrepInvalid, got {error:?}");
    };
    assert!(
        detail.contains("BRepCheck_Analyzer"),
        "refusal carries the kernel diagnostic: {detail}"
    );
    let diagnostic = domain_command_diagnostic(&HostError::BrepInvalid {
        request_id: None,
        detail: detail.clone(),
    });
    assert_eq!(diagnostic.code, DiagnosticCode::BrepInvalid);
    assert!(
        !output.exists() && !parent.join("arm-x.stl").exists(),
        "refusal produces no export output"
    );
    assert_eq!(
        std::fs::read(bundle.join("manifest.json")).expect("manifest re-reads"),
        manifest,
        "refusal leaves the manifest untouched"
    );
    assert_eq!(
        std::fs::read(bundle.join("transactions.log")).expect("log re-reads"),
        log,
        "refusal appends no transaction"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// End-to-end vertical slice: one named test validates a known-good
/// reopened solid, then observes structured refusal for corrupt and
/// unavailable geometry without producing export output.
#[test]
fn validate_selected_current_solid_end_to_end() {
    if required_fixture_worker("validate_selected_current_solid_end_to_end").is_none() {
        eprintln!("validate: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let parent = root("end-to-end");
    let output = parent.join("output");
    let host = Host::new();

    // Phase 1: known-good solid validates after save plus fresh reopen.
    let bundle = new_project(&host, &parent);
    let mut extrude = json!({
        "feature_id": "arm-x",
        "profile": [[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 20.0]],
        "height": 8.0,
        "mode": "additive",
    });
    extrude["bundle_path"] = bundle.to_string_lossy().into_owned().into();
    let extruded = command_response(&host, EXTRUDE_COMMAND_ID, extrude);
    assert_eq!(extruded["status"], "ok");
    command_response(
        &host,
        SAVE_COMMAND_ID,
        json!({
            "bundle_path": bundle.to_string_lossy(),
            "feature_id": "validate-snapshot",
            "kind": "checkpoint",
            "expected_revision": extruded["revision_hash"],
        }),
    );
    let reopened = Host::new();
    let loaded = command_response(
        &reopened,
        LOAD_COMMAND_ID,
        json!({"bundle_path": bundle.to_string_lossy()}),
    );
    let success = command_response(
        &reopened,
        VALIDATE_COMMAND_ID,
        json!({
            "bundle_path": bundle.to_string_lossy(),
            "feature_id": "arm-x",
        }),
    );
    assert_eq!(success["status"], "ok");
    assert_eq!(success["valid"], true);
    assert_eq!(success["feature_id"], "arm-x");
    assert_eq!(success["revision_hash"], loaded["revision_hash"]);

    // Phase 2: corrupt committed geometry is refused as fatal.
    let brep_path = bundle.join("brep/arm-x.brep");
    let bytes = std::fs::read(&brep_path).expect("committed BREP reads");
    let midpoint = bytes.len() / 2;
    let corrupted: Vec<u8> = bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if index >= midpoint
                && index < (midpoint + bytes.len() / 4).min(bytes.len())
                && byte.is_ascii_digit()
            {
                b'7'
            } else {
                *byte
            }
        })
        .collect();
    assert_ne!(corrupted, bytes);
    std::fs::write(&brep_path, &corrupted).expect("corrupted BREP writes");
    let error = Host::new()
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "arm-x",
            }),
        )
        .expect_err("corrupt geometry is refused");
    assert!(
        matches!(
            error,
            ExecutionError::Handler(HostError::BrepInvalid { .. })
        ),
        "corrupt geometry reports BrepInvalid, got {error:?}"
    );

    // Phase 3: unavailable geometry is reported explicitly, never valid.
    std::fs::remove_file(&brep_path).expect("corrupted BREP deletes");
    let error = Host::new()
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "arm-x",
            }),
        )
        .expect_err("missing geometry is refused");
    assert!(
        matches!(
            error,
            ExecutionError::Handler(HostError::BrepFileMissing { .. })
        ),
        "missing geometry reports BrepFileMissing, got {error:?}"
    );
    let error = Host::new()
        .execute_domain_command(
            VALIDATE_COMMAND_ID,
            json!({
                "bundle_path": bundle.to_string_lossy(),
                "feature_id": "unbuilt-solid",
            }),
        )
        .expect_err("unselected geometry is refused");
    assert!(
        matches!(error, ExecutionError::Handler(HostError::Validation { .. })),
        "unselected geometry reports Validation, got {error:?}"
    );
    assert!(!output.exists(), "no phase produced export output");

    let _ = std::fs::remove_dir_all(&parent);
}

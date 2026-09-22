//! Production API coverage for exporting one validated current solid to STL.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::{Host, HostError, domain_execution_diagnostic};
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::diagnostic::DiagnosticCode;
use threeterm_protocol::schema::{EXPORT_COMMAND_ID, LOAD_COMMAND_ID, VALIDATE_COMMAND_ID};
use threeterm_protocol::schema_validator::validate as validate_schema;

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-export-validated-{label}-{suffix}"))
}

fn command_response(
    host: &Host,
    command: threeterm_protocol::schema::CommandId,
    request: Value,
) -> Value {
    let schema = threeterm_protocol::schema::find(command).expect("command is registered");
    validate_schema(&schema.request_schema, &request).expect("request validates");
    let response = host
        .execute_domain_command(command, request)
        .expect("command succeeds");
    validate_schema(&schema.response_schema, &response).expect("response validates");
    response
}

fn export_request(bundle: &std::path::Path, output: &std::path::Path) -> Value {
    json!({
        "bundle_path": bundle.to_string_lossy(),
        "feature_id": "l-bracket",
        "formats": ["stl"],
        "output_dir": output.to_string_lossy(),
        "tessellation_deflection": 0.1,
        "override_warnings": false,
        "accept_stale_geometry": false,
    })
}

#[test]
fn export_validated_current_solid_to_stl_end_to_end() {
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() {
                panic!("OCCT worker is required: {error}");
            }
            eprintln!("export_validated_solid: OCCT worker unavailable: {error}");
            return;
        }
    };
    drop(worker);

    let parent = root("vertical-slice");
    let bundle = parent.join("project");
    let output = parent.join("export");
    let invalid_output = parent.join("invalid-export");
    Host::new()
        .save_bracket(&bundle, "l-bracket", 60.0, 30.0, 40.0, 3.0)
        .expect("real L-bracket saves");

    let reopened = Host::new();
    let loaded = command_response(
        &reopened,
        LOAD_COMMAND_ID,
        json!({"bundle_path": bundle.to_string_lossy()}),
    );
    let validated = command_response(
        &reopened,
        VALIDATE_COMMAND_ID,
        json!({
            "bundle_path": bundle.to_string_lossy(),
            "feature_id": "l-bracket",
        }),
    );
    assert_eq!(validated["status"], "ok");
    assert_eq!(validated["valid"], true);
    assert_eq!(validated["feature_id"], "l-bracket");
    assert_eq!(validated["revision_hash"], loaded["revision_hash"]);

    let manifest = fs::read(bundle.join("manifest.json")).expect("manifest reads");
    let log = fs::read(bundle.join("transactions.log")).expect("transaction log reads");
    let stl_path = output.join("l-bracket.stl");
    assert!(
        !stl_path.exists(),
        "requested STL destination starts absent"
    );

    let exported = command_response(
        &reopened,
        EXPORT_COMMAND_ID,
        export_request(&bundle, &output),
    );
    assert_eq!(exported["status"], "ok");
    assert_eq!(exported["feature_id"], validated["feature_id"]);
    assert_eq!(exported["source_revision_id"], validated["revision_hash"]);
    assert_eq!(
        exported["validation"],
        json!({
            "feature_id": validated["feature_id"],
            "revision_id": validated["revision_id"],
            "feature_graph_hash": validated["feature_graph_hash"],
            "revision_hash": validated["revision_hash"],
            "brep_path": validated["brep_path"],
            "brep_sha256": validated["brep_sha256"],
            "valid": true,
        })
    );
    assert_eq!(exported["tessellation"]["units"], "millimetres");
    assert_eq!(exported["tessellation"]["relative"], false);
    assert_eq!(exported["tessellation"]["deflection"], 0.1);
    assert_eq!(exported["tessellation"]["angular_deflection_radians"], 0.5);

    let stl = fs::read(&stl_path).expect("published STL reads");
    assert!(!stl.is_empty(), "published STL is nonempty");
    assert!(
        String::from_utf8_lossy(&stl).contains("facet"),
        "published STL contains ASCII facets"
    );
    let artifact = exported["derived_artifacts"]
        .as_array()
        .expect("derived artifacts are present")
        .first()
        .expect("STL artifact is present");
    assert_eq!(artifact["artifact_kind"], "stl");
    assert_eq!(artifact["feature_id"], "l-bracket");
    assert_eq!(artifact["source_revision_id"], validated["revision_hash"]);
    assert_eq!(artifact["output_path"], stl_path.to_string_lossy().as_ref());
    assert_eq!(artifact["artifact_name"], "l-bracket.stl");
    assert_eq!(artifact["byte_count"], stl.len());
    assert_eq!(artifact["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(exported["artifacts"], json!([stl_path.to_string_lossy()]));
    assert_eq!(fs::read(bundle.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(bundle.join("transactions.log")).unwrap(), log);

    let brep_path = bundle.join("brep/l-bracket.brep");
    let bytes = fs::read(&brep_path).expect("current BREP reads");
    let corrupted: Vec<u8> = bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if index >= bytes.len() / 2 && byte.is_ascii_digit() {
                b'7'
            } else {
                *byte
            }
        })
        .collect();
    assert_ne!(corrupted, bytes);
    fs::write(&brep_path, corrupted).expect("current BREP corruption writes");
    let error = reopened
        .execute_domain_command(EXPORT_COMMAND_ID, export_request(&bundle, &invalid_output))
        .expect_err("invalid current geometry refuses export");
    let diagnostic = domain_execution_diagnostic(&error);
    assert_eq!(diagnostic.code, DiagnosticCode::BrepInvalid);
    assert!(diagnostic.arg.contains("l-bracket"));
    assert!(!invalid_output.exists(), "refusal creates no output");

    assert!(matches!(
        error,
        ExecutionError::Handler(HostError::BrepInvalid { .. })
    ));
    let _ = fs::remove_dir_all(parent);
}

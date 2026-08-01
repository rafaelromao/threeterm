//! Integration tests that exercise the production worker binary through
//! the Rust boundary.
//!
//! When the worker binary is unavailable (no system OCCT in the
//! development environment) the tests soft-skip via
//! `OcctWorker::locate` returning `Err`. The CI archlinux container
//! installs `opencascade` via `pacman` so the binary is built and the
//! tests exercise the production code path end-to-end.

use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_occt_worker::{
    BooleanFuseRequest, ExtrudeRequest, OcctDiagnostic, OcctWorker, Operation, WorkerError,
    schema_version,
};

fn unique_request_id(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{label}-{nanos}-{}", std::process::id())
}

fn rectangle_extrude_request() -> ExtrudeRequest {
    ExtrudeRequest::new(
        unique_request_id("rect"),
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
        3.0,
    )
    .with_feature_id("box-rect")
}

fn triangle_extrude_request(label: &str) -> ExtrudeRequest {
    ExtrudeRequest::new(
        unique_request_id(label),
        vec![(0.0, 0.0), (4.0, 0.0), (2.0, 4.0)],
        2.0,
    )
    .with_feature_id(format!("{label}-1"))
}

fn locate_worker() -> Option<OcctWorker> {
    if let Ok(worker) = OcctWorker::locate() {
        return Some(worker);
    }
    eprintln!(
        "threeterm-occt-worker: no worker binary found; set \
         THREETERM_OCCTBUILD_WORKER or build the crate against a system OCCT install"
    );
    None
}

#[test]
fn extrude_rectangle_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!("threeterm-occt-extrude-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir creates");
    let request = rectangle_extrude_request().with_output_path(&temp, "rect.brep");

    let result = worker.extrude(&request).expect("extrude returns");

    assert_eq!(result.status, "ok", "extrude returned {:?}", result);
    assert_eq!(result.operation, Operation::Extrude);
    assert_eq!(result.feature_id, "box-rect");
    let brep_path = result.brep_path.clone();
    assert!(brep_path.is_file(), "BREP was not written: {brep_path:?}");
    let bytes = std::fs::read(&brep_path).expect("BREP reads");
    assert!(!bytes.is_empty());
    assert_eq!(result.brep_bytes, bytes.len());
    assert_eq!(result.brep_sha256.len(), 64);
    // The OCCT BREP file starts with the `DBRep_DrawableShape` marker
    // emitted by `BRepTools::Write`. Asserting on the prefix proves
    // the file is a real OCCT shape — not an empty payload or a
    // hand-coded fixture.
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "BREP must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn extrude_with_short_profile_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let mut request = ExtrudeRequest::new(
        unique_request_id("short"),
        vec![(0.0, 0.0), (1.0, 0.0)],
        1.0,
    )
    .with_feature_id("short-1");
    request.profile = vec![[0.0, 0.0], [1.0, 0.0]]; // 2 vertices — too short
    let result = worker.extrude(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
            assert_eq!(diag.schema_version, schema_version());
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn extrude_with_zero_height_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let mut request = triangle_extrude_request("zero");
    request.height = 0.0;
    let result = worker.extrude(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn boolean_fuse_of_two_extrudes_emits_a_valid_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!("threeterm-occt-fuse-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = ExtrudeRequest::new(
        unique_request_id("base"),
        vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)],
        2.0,
    )
    .with_output_path(&temp, "base.brep")
    .with_feature_id("box-base");
    let base_result = worker.extrude(&base_request).expect("base extrude");
    assert_eq!(base_result.status, "ok");

    let tool_request = ExtrudeRequest::new(
        unique_request_id("tool"),
        vec![(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)],
        2.0,
    )
    .with_output_path(&temp, "tool.brep")
    .with_feature_id("box-tool");
    let tool_result = worker.extrude(&tool_request).expect("tool extrude");
    assert_eq!(tool_result.status, "ok");

    let fuse_request = BooleanFuseRequest::new(
        unique_request_id("fuse"),
        &base_result.brep_path,
        &tool_result.brep_path,
    )
    .with_output_path(&temp, "fused.brep")
    .with_feature_id("box-fused");
    let fuse_result = worker.boolean_fuse(&fuse_request).expect("boolean_fuse");
    assert_eq!(fuse_result.status, "ok", "fuse returned {:?}", fuse_result);
    assert_eq!(fuse_result.operation, Operation::BooleanFuse);
    assert_eq!(fuse_result.feature_id, "box-fused");
    let fused_path = fuse_result.brep_path.clone();
    assert!(
        fused_path.is_file(),
        "fused BREP was not written: {fused_path:?}"
    );
    let fused_bytes = std::fs::read(&fused_path).expect("fused BREP reads");
    assert!(fused_bytes.len() > base_result.brep_bytes);
    assert_eq!(fuse_result.brep_bytes, fused_bytes.len());
    assert_eq!(fuse_result.brep_sha256.len(), 64);

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn boolean_fuse_with_missing_base_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = BooleanFuseRequest::new(
        unique_request_id("missing"),
        "/no/such/base.brep",
        "/no/such/tool.brep",
    )
    .with_feature_id("missing-1");
    let result = worker.boolean_fuse(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn diagnostic_round_trips_with_schema_version() {
    let diag = OcctDiagnostic::new("brep_invalid", "BRepCheck_Analyzer failed");
    let value = serde_json::to_value(&diag).expect("diagnostic serializes");
    assert_eq!(value["code"], "brep_invalid");
    assert_eq!(value["arg"], "BRepCheck_Analyzer failed");
    assert_eq!(value["schema_version"], schema_version());
}

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
    BooleanFuseRequest, ChamferRequest, ExtrudeRequest, FilletRequest, HoleRequest,
    LinearPatternRequest, MirrorRequest, OcctDiagnostic, OcctWorker, Operation, RevolveRequest,
    WorkerError, schema_version,
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

fn fillet_request(base_path: &std::path::Path, label: &str) -> FilletRequest {
    FilletRequest::new(unique_request_id(label), base_path, 0.5).with_feature_id("box-filleted-1")
}

fn chamfer_request(base_path: &std::path::Path, label: &str) -> ChamferRequest {
    ChamferRequest::new(unique_request_id(label), base_path, 0.25)
        .with_feature_id("box-chamfered-1")
}

#[test]
fn fillet_of_extruded_box_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!("threeterm-occt-fillet-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = triangle_extrude_request("fillet-base")
        .with_output_path(&temp, "fillet-base.brep")
        .with_feature_id("fillet-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");
    assert_eq!(base_result.status, "ok");

    let request = fillet_request(&base_result.brep_path, "fillet-1")
        .with_output_path(&temp, "fillet-out.brep");
    let result = worker.fillet(&request).expect("fillet returns");
    assert_eq!(result.status, "ok", "fillet returned {:?}", result);
    assert_eq!(result.operation, Operation::Fillet);
    assert_eq!(result.feature_id, "box-filleted-1");
    let brep_path = result.brep_path.clone();
    assert!(
        brep_path.is_file(),
        "filleted BREP was not written: {brep_path:?}"
    );
    let bytes = std::fs::read(&brep_path).expect("filleted BREP reads");
    assert!(!bytes.is_empty());
    assert_eq!(result.brep_bytes, bytes.len());
    assert_eq!(result.brep_sha256.len(), 64);
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "filleted BREP must start with DBRep_DrawableShape marker; got {prefix_str:?}"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn chamfer_of_extruded_box_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!("threeterm-occt-chamfer-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = triangle_extrude_request("chamfer-base")
        .with_output_path(&temp, "chamfer-base.brep")
        .with_feature_id("chamfer-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");
    assert_eq!(base_result.status, "ok");

    let request = chamfer_request(&base_result.brep_path, "chamfer-1")
        .with_output_path(&temp, "chamfer-out.brep");
    let result = worker.chamfer(&request).expect("chamfer returns");
    assert_eq!(result.status, "ok", "chamfer returned {:?}", result);
    assert_eq!(result.operation, Operation::Chamfer);
    assert_eq!(result.feature_id, "box-chamfered-1");
    let brep_path = result.brep_path.clone();
    assert!(
        brep_path.is_file(),
        "chamfered BREP was not written: {brep_path:?}"
    );
    let bytes = std::fs::read(&brep_path).expect("chamfered BREP reads");
    assert!(!bytes.is_empty());
    assert_eq!(result.brep_bytes, bytes.len());
    assert_eq!(result.brep_sha256.len(), 64);
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "chamfered BREP must start with DBRep_DrawableShape marker; got {prefix_str:?}"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn fillet_with_missing_base_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = FilletRequest::new(
        unique_request_id("fillet-missing"),
        "/no/such/base.brep",
        0.5,
    )
    .with_feature_id("fillet-missing-1");
    let result = worker.fillet(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn chamfer_with_missing_base_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = ChamferRequest::new(
        unique_request_id("chamfer-missing"),
        "/no/such/base.brep",
        0.25,
    )
    .with_feature_id("chamfer-missing-1");
    let result = worker.chamfer(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn fillet_with_zero_radius_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = FilletRequest::new(unique_request_id("fillet-zero"), "/tmp/base.brep", 0.0)
        .with_feature_id("fillet-zero-1");
    let result = worker.fillet(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn chamfer_with_zero_distance_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = ChamferRequest::new(unique_request_id("chamfer-zero"), "/tmp/base.brep", 0.0)
        .with_feature_id("chamfer-zero-1");
    let result = worker.chamfer(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

fn hole_request(base_path: &std::path::Path, label: &str, feature_id: &str) -> HoleRequest {
    HoleRequest::new(
        unique_request_id(label),
        base_path,
        [1.5, 1.5, 0.0],
        [0.0, 0.0, 1.0],
        1.0,
    )
    .with_feature_id(feature_id.to_string())
}

#[test]
fn hole_of_extruded_box_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!("threeterm-occt-hole-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = rectangle_extrude_request()
        .with_output_path(&temp, "hole-base.brep")
        .with_feature_id("hole-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");
    assert_eq!(base_result.status, "ok");

    let request = hole_request(&base_result.brep_path, "hole-1", "box-holed-1")
        .with_output_path(&temp, "hole-out.brep");
    let result = worker.hole(&request).expect("hole returns");
    assert_eq!(result.status, "ok", "hole returned {:?}", result);
    assert_eq!(result.operation, Operation::Hole);
    assert_eq!(result.feature_id, "box-holed-1");
    let brep_path = result.brep_path.clone();
    assert!(
        brep_path.is_file(),
        "holed BREP was not written: {brep_path:?}"
    );
    let bytes = std::fs::read(&brep_path).expect("holed BREP reads");
    assert!(!bytes.is_empty());
    assert_eq!(result.brep_bytes, bytes.len());
    assert_eq!(result.brep_sha256.len(), 64);
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "holed BREP must start with DBRep_DrawableShape marker; got {prefix_str:?}"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn hole_with_missing_base_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = hole_request(
        std::path::Path::new("/no/such/base.brep"),
        "hole-missing",
        "hole-missing-1",
    );
    let result = worker.hole(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn hole_with_zero_diameter_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = HoleRequest::new(
        unique_request_id("hole-zero"),
        "/tmp/base.brep",
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        0.0,
    )
    .with_feature_id("hole-zero-1");
    let result = worker.hole(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

fn revolve_request(label: &str, feature_id: &str) -> RevolveRequest {
    RevolveRequest::new(
        unique_request_id(label),
        vec![(0.0, 0.5), (1.0, 0.5), (1.0, -0.5), (0.0, -0.5)],
        [0.0, 0.5, 0.0],
        [0.0, 1.0, 0.0],
        std::f64::consts::TAU,
    )
    .with_feature_id(feature_id.to_string())
}

#[test]
fn revolve_rectangle_around_axis_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!("threeterm-occt-revolve-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let request =
        revolve_request("rev-1", "box-revolved-1").with_output_path(&temp, "revolved.brep");
    let result = worker.revolve(&request).expect("revolve returns");
    assert_eq!(result.status, "ok", "revolve returned {:?}", result);
    assert_eq!(result.operation, Operation::Revolve);
    assert_eq!(result.feature_id, "box-revolved-1");
    let brep_path = result.brep_path.clone();
    assert!(
        brep_path.is_file(),
        "revolved BREP was not written: {brep_path:?}"
    );
    let bytes = std::fs::read(&brep_path).expect("revolved BREP reads");
    assert!(!bytes.is_empty());
    assert_eq!(result.brep_bytes, bytes.len());
    assert_eq!(result.brep_sha256.len(), 64);
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "revolved BREP must start with DBRep_DrawableShape marker; got {prefix_str:?}"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn revolve_with_short_profile_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let mut request = RevolveRequest::new(
        unique_request_id("rev-short"),
        vec![(0.0, 0.5), (1.0, 0.5)],
        [0.0, 0.5, 0.0],
        [0.0, 1.0, 0.0],
        std::f64::consts::TAU,
    )
    .with_feature_id("rev-short-1");
    request.profile = vec![[0.0, 0.5], [1.0, 0.5]];
    let result = worker.revolve(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

fn mirror_request(label: &str, feature_id: &str, base_path: &std::path::Path) -> MirrorRequest {
    MirrorRequest::new(
        unique_request_id(label),
        base_path,
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
    )
    .with_feature_id(feature_id.to_string())
}

#[test]
fn mirror_extrude_across_plane_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!("threeterm-occt-mirror-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let extrude_request =
        triangle_extrude_request("mirror-base").with_output_path(&temp, "mirror-base.brep");
    let base = worker.extrude(&extrude_request).expect("base extrude");
    assert_eq!(base.status, "ok", "base extrude returned {:?}", base);
    let base_path = base.brep_path.clone();

    let request = mirror_request("mirror-1", "mirror-out-1", &base_path)
        .with_output_path(&temp, "mirrored.brep");
    let result = worker.mirror(&request).expect("mirror returns");
    assert_eq!(result.status, "ok", "mirror returned {:?}", result);
    assert_eq!(result.operation, Operation::Mirror);
    assert_eq!(result.feature_id, "mirror-out-1");
    let brep_path = result.brep_path.clone();
    assert!(
        brep_path.is_file(),
        "mirrored BREP was not written: {brep_path:?}"
    );
    let bytes = std::fs::read(&brep_path).expect("mirrored BREP reads");
    assert!(!bytes.is_empty());
    assert_eq!(result.brep_bytes, bytes.len());
    assert_eq!(result.brep_sha256.len(), 64);
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "mirrored BREP must start with DBRep_DrawableShape marker; got {prefix_str:?}"
    );

    let base_bytes = std::fs::read(&base_path).expect("base BREP reads");
    assert_ne!(
        bytes, base_bytes,
        "mirrored BREP must differ byte-for-byte from the source BREP"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn mirror_with_zero_plane_normal_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let mut request = mirror_request(
        "mirror-no-normal",
        "mirror-no-normal-1",
        std::path::Path::new("/tmp/base.brep"),
    );
    request.plane_normal = [0.0, 0.0, 0.0];
    let result = worker.mirror(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
            assert_eq!(diag.schema_version, schema_version());
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn mirror_with_non_finite_plane_normal_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let mut request = mirror_request(
        "mirror-nan",
        "mirror-nan-1",
        std::path::Path::new("/tmp/base.brep"),
    );
    request.plane_normal = [f64::NAN, 0.0, 0.0];
    let result = worker.mirror(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn revolve_with_zero_angle_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let mut request = revolve_request("rev-zero", "rev-zero-1");
    request.angle = 0.0;
    let result = worker.revolve(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn revolve_with_zero_axis_direction_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let mut request = revolve_request("rev-no-axis", "rev-no-axis-1");
    request.axis_direction = [0.0, 0.0, 0.0];
    let result = worker.revolve(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

fn linear_pattern_request(
    label: &str,
    feature_id: &str,
    base_path: &std::path::Path,
) -> LinearPatternRequest {
    LinearPatternRequest::new(unique_request_id(label), base_path, [1.0, 0.0, 0.0], 3, 3.0)
        .with_feature_id(feature_id.to_string())
}

#[test]
fn linear_pattern_of_extruded_box_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!(
        "threeterm-occt-linear-pattern-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = rectangle_extrude_request()
        .with_output_path(&temp, "linear-pattern-base.brep")
        .with_feature_id("linear-pattern-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");
    assert_eq!(base_result.status, "ok");

    let request = linear_pattern_request(
        "linear-pattern-1",
        "linear-pattern-out-1",
        &base_result.brep_path,
    )
    .with_output_path(&temp, "linear-pattern-out.brep");
    let result = worker
        .linear_pattern(&request)
        .expect("linear_pattern returns");
    assert_eq!(result.status, "ok", "linear_pattern returned {:?}", result);
    assert_eq!(result.operation, Operation::LinearPattern);
    assert_eq!(result.feature_id, "linear-pattern-out-1");
    let brep_path = result.brep_path.clone();
    assert!(
        brep_path.is_file(),
        "linear-pattern BREP was not written: {brep_path:?}"
    );
    let bytes = std::fs::read(&brep_path).expect("linear-pattern BREP reads");
    assert!(!bytes.is_empty());
    assert_eq!(result.brep_bytes, bytes.len());
    assert_eq!(result.brep_sha256.len(), 64);
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "linear-pattern BREP must start with DBRep_DrawableShape marker; got {prefix_str:?}"
    );

    let base_bytes = std::fs::read(&base_result.brep_path).expect("base BREP reads");
    assert_ne!(
        bytes, base_bytes,
        "linear-pattern BREP must differ byte-for-byte from the source BREP"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn linear_pattern_with_count_one_returns_ok_with_real_brep() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!(
        "threeterm-occt-linear-pattern-single-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = rectangle_extrude_request()
        .with_output_path(&temp, "linear-pattern-single-base.brep")
        .with_feature_id("linear-pattern-single-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");
    assert_eq!(base_result.status, "ok");

    let request = LinearPatternRequest::new(
        unique_request_id("linear-pattern-single"),
        &base_result.brep_path,
        [1.0, 0.0, 0.0],
        1,
        5.0,
    )
    .with_output_path(&temp, "linear-pattern-single-out.brep")
    .with_feature_id("linear-pattern-single-out-1");
    let result = worker
        .linear_pattern(&request)
        .expect("linear_pattern returns");
    assert_eq!(result.status, "ok", "linear_pattern returned {:?}", result);
    assert_eq!(result.operation, Operation::LinearPattern);
    let bytes = std::fs::read(&result.brep_path).expect("BREP reads");
    let base_bytes = std::fs::read(&base_result.brep_path).expect("base BREP reads");
    assert_eq!(
        bytes, base_bytes,
        "count == 1 must produce a BREP identical to the source"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn linear_pattern_with_missing_base_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let request = linear_pattern_request(
        "linear-pattern-missing",
        "linear-pattern-missing-1",
        std::path::Path::new("/no/such/base.brep"),
    );
    let result = worker.linear_pattern(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }
}

#[test]
fn linear_pattern_with_zero_count_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!(
        "threeterm-occt-linear-pattern-zero-count-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = rectangle_extrude_request()
        .with_output_path(&temp, "linear-pattern-zero-count-base.brep")
        .with_feature_id("linear-pattern-zero-count-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");

    let mut request = LinearPatternRequest::new(
        unique_request_id("linear-pattern-zero-count"),
        &base_result.brep_path,
        [1.0, 0.0, 0.0],
        0,
        1.0,
    )
    .with_output_path(&temp, "linear-pattern-zero-count-out.brep")
    .with_feature_id("linear-pattern-zero-count-out-1");
    request.count = 0;
    let result = worker.linear_pattern(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn linear_pattern_with_zero_direction_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!(
        "threeterm-occt-linear-pattern-zero-direction-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = rectangle_extrude_request()
        .with_output_path(&temp, "linear-pattern-zero-direction-base.brep")
        .with_feature_id("linear-pattern-zero-direction-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");

    let mut request = linear_pattern_request(
        "linear-pattern-zero-direction",
        "linear-pattern-zero-direction-1",
        &base_result.brep_path,
    );
    request.direction = [0.0, 0.0, 0.0];
    let result = worker.linear_pattern(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn linear_pattern_with_zero_spacing_returns_request_malformed() {
    let Some(worker) = locate_worker() else {
        return;
    };
    let temp = std::env::temp_dir().join(format!(
        "threeterm-occt-linear-pattern-zero-spacing-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).expect("temp dir creates");

    let base_request = rectangle_extrude_request()
        .with_output_path(&temp, "linear-pattern-zero-spacing-base.brep")
        .with_feature_id("linear-pattern-zero-spacing-base-1");
    let base_result = worker.extrude(&base_request).expect("base extrude");

    let mut request = linear_pattern_request(
        "linear-pattern-zero-spacing",
        "linear-pattern-zero-spacing-1",
        &base_result.brep_path,
    );
    request.spacing = 0.0;
    let result = worker.linear_pattern(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
        }
        other => panic!("expected request_malformed diagnostic, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(temp);
}

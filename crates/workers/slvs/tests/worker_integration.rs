//! Integration tests that exercise the production worker binary through the
//! Rust boundary.

use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_slvs_worker::{
    SCHEMA_VERSION, SketchEntity, SketchRequest, SketchConstraint, SlvsWorker, WorkerError,
};

fn unique_request_id(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{label}-{nanos}-{}", std::process::id())
}

fn rectangle_request() -> SketchRequest {
    SketchRequest::new(unique_request_id("rect"))
        .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0))
        .with_entity(SketchEntity::point_2d("p2", 10.0, 0.0))
        .with_entity(SketchEntity::point_2d("p3", 10.0, 5.0))
        .with_entity(SketchEntity::point_2d("p4", 0.0, 5.0))
        .with_entity(SketchEntity::line_segment_2d("l1", "p1", "p2"))
        .with_entity(SketchEntity::line_segment_2d("l2", "p2", "p3"))
        .with_entity(SketchEntity::line_segment_2d("l3", "p3", "p4"))
        .with_entity(SketchEntity::line_segment_2d("l4", "p4", "p1"))
        .with_constraint(SketchConstraint::horizontal("h1", "l1"))
        .with_constraint(SketchConstraint::vertical("v2", "l2"))
        .with_constraint(SketchConstraint::horizontal("h3", "l3"))
        .with_constraint(SketchConstraint::vertical("v4", "l4"))
        .with_constraint(SketchConstraint::distance("dw", "p1", "p2", 10.0))
        .with_constraint(SketchConstraint::distance("dh", "p1", "p4", 5.0))
}

fn locate_worker() -> Option<SlvsWorker> {
    if let Ok(worker) = SlvsWorker::locate() {
        return Some(worker);
    }
    eprintln!(
        "threeterm-slvs-worker: no worker binary found; set \
         THREETERM_SLVSBUILD_WORKER or build the crate first"
    );
    None
}

#[test]
fn rectangle_solve_returns_ok_with_coordinates() {
    let Some(worker) = locate_worker() else { return };
    let result = worker
        .solve(&rectangle_request())
        .expect("rectangle solve succeeds");
    assert_eq!(result.status, "ok", "solver returned {:?}", result);
    assert!(result.resolved_entity_ids.contains(&"p1".to_string()));
    assert!(result.resolved_entity_ids.contains(&"p2".to_string()));
    assert!(result.resolved_entity_ids.contains(&"p3".to_string()));
    assert!(result.resolved_entity_ids.contains(&"p4".to_string()));
    let coords = result.coordinates.expect("coordinates present");
    assert_eq!(coords.get("p1"), Some(&[0.0, 0.0]));
    assert_eq!(coords.get("p2"), Some(&[10.0, 0.0]));
    assert_eq!(coords.get("p3"), Some(&[10.0, 5.0]));
    assert_eq!(coords.get("p4"), Some(&[0.0, 5.0]));
}

#[test]
fn empty_stdin_returns_request_malformed_diagnostic() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let Some(worker) = locate_worker() else { return };
    let mut child = Command::new(worker.binary_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("worker spawns");
    // Close stdin immediately to deliver an empty stream.
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("worker waits");
    assert!(!output.status.success(), "empty stdin must exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("request_malformed"), "stderr={stderr}");
    assert!(stderr.contains("empty stdin"), "stderr={stderr}");
}

#[test]
fn duplicate_id_returns_request_malformed_diagnostic() {
    let Some(worker) = locate_worker() else { return };
    let mut request = SketchRequest::new(unique_request_id("dup"))
        .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0));
    // Duplicate the same id by re-using the same constraint id pattern.
    request.entities.push(SketchEntity::fixed_point_2d("p1", 1.0, 1.0));
    let result = worker.solve(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
            assert!(diag.arg.contains("duplicate"), "arg={:?}", diag.arg);
        }
        other => panic!("expected duplicate diagnostic, got {other:?}"),
    }
}

#[test]
fn inconsistent_distance_reports_inconsistent_status() {
    let Some(worker) = locate_worker() else { return };
    // Two coincident points with a non-zero distance constraint between them.
    let request = SketchRequest::new(unique_request_id("inconsistent"))
        .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0))
        .with_entity(SketchEntity::fixed_point_2d("p2", 1.0, 0.0))
        .with_constraint(SketchConstraint::distance("d", "p1", "p2", 10.0))
        .with_constraint(SketchConstraint::coincident("c", "p1", "p2"));
    let result = worker.solve(&request).expect("solve returns");
    assert_eq!(result.status, "inconsistent");
    assert!(
        result.failed_constraint_ids.contains(&"d".to_string()),
        "failed_constraint_ids={:?}",
        result.failed_constraint_ids
    );
}

#[test]
fn underconstrained_sketch_reports_positive_dof() {
    let Some(worker) = locate_worker() else { return };
    // Two points with no constraints -> fully underconstrained.
    let request = SketchRequest::new(unique_request_id("under"))
        .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0))
        .with_entity(SketchEntity::point_2d("p2", 1.0, 1.0));
    let result = worker.solve(&request).expect("solve returns");
    assert_eq!(result.status, "ok");
    assert!(
        result.dof >= 2,
        "two free 2D points must have at least 2 dof; got {}",
        result.dof
    );
}

#[test]
fn unknown_constraint_reference_returns_request_malformed() {
    let Some(worker) = locate_worker() else { return };
    let request = SketchRequest::new(unique_request_id("missing-ref"))
        .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0))
        .with_constraint(SketchConstraint::distance("d", "p1", "ghost", 10.0));
    let result = worker.solve(&request);
    match result {
        Err(WorkerError::Diagnostic(diag)) => {
            assert_eq!(diag.code, "request_malformed");
            assert!(diag.arg.contains("unknown"), "arg={:?}", diag.arg);
        }
        other => panic!("expected unknown-entity diagnostic, got {other:?}"),
    }
}
use threeterm_domain::{
    PlanarFaceEvidence, PlanarFaceProvenance, PlanarFaceReference, SketchPlacement,
};
use threeterm_slvs_worker::{SketchConstraint, SketchEntity, SketchSolveRequest, SlvsWorker};

fn rectangle(request_id: &str) -> SketchSolveRequest {
    let points = [
        ("p0", 0.0, 0.0),
        ("p1", 10.0, 0.0),
        ("p2", 10.0, 5.0),
        ("p3", 0.0, 5.0),
    ];
    let mut entities = points
        .into_iter()
        .map(|(id, x, y)| SketchEntity::Point {
            id: id.to_string(),
            x,
            y,
        })
        .collect::<Vec<_>>();
    entities.extend([
        SketchEntity::LineSegment {
            id: "e0".to_string(),
            start: "p0".to_string(),
            end: "p1".to_string(),
        },
        SketchEntity::LineSegment {
            id: "e1".to_string(),
            start: "p1".to_string(),
            end: "p2".to_string(),
        },
        SketchEntity::LineSegment {
            id: "e2".to_string(),
            start: "p2".to_string(),
            end: "p3".to_string(),
        },
        SketchEntity::LineSegment {
            id: "e3".to_string(),
            start: "p3".to_string(),
            end: "p0".to_string(),
        },
    ]);
    let constraints = ["p0", "p1", "p2", "p3"]
        .into_iter()
        .map(|id| SketchConstraint {
            id: format!("fixed-{id}"),
            kind: "fixed".to_string(),
            entities: vec![id.to_string()],
            value: None,
        })
        .collect();
    SketchSolveRequest::new(request_id, "rectangle", entities, constraints)
}

#[test]
fn pinned_libslvs_worker_solves_a_rectangle_with_stable_ids() {
    let worker = match SlvsWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() {
                panic!(
                    "{{\"code\":\"worker_unavailable\",\"worker\":\"libslvs\",\"detail\":\"{error}\"}}"
                );
            }
            eprintln!("libslvs integration skipped: no configured worker binary: {error}");
            return;
        }
    };
    let result = worker
        .solve(&rectangle("real-rectangle"))
        .expect("real libslvs worker solves the rectangle");
    assert_eq!(result.status, "solved");
    assert_eq!(result.dof, 0);
    assert_eq!(
        result.entity_ids,
        ["p0", "p1", "p2", "p3", "e0", "e1", "e2", "e3"]
    );
    assert!(result.related_constraint_ids.is_empty());
    assert_eq!(result.solved_coordinates.as_ref().map(Vec::len), Some(4));
}

#[test]
fn pinned_libslvs_worker_reports_attached_failure_diagnostics_without_coordinates() {
    let worker = match SlvsWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() {
                panic!(
                    "{{\"code\":\"worker_unavailable\",\"worker\":\"libslvs\",\"detail\":\"{error}\"}}"
                );
            }
            eprintln!("libslvs integration skipped: no configured worker binary: {error}");
            return;
        }
    };
    let support = PlanarFaceReference {
        semantic_id: "solid/vertical-face".into(),
        provenance: PlanarFaceProvenance {
            source_feature_id: "solid".into(),
            source_revision_id: "solid-revision".into(),
            source_face_id: "solid/vertical-face".into(),
        },
        role: "sketch-support".into(),
        evidence: PlanarFaceEvidence {
            topology_kind: "planar_face".into(),
            origin: [0.0, 2.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 0.0, -1.0],
            adjacent_feature_ids: Vec::new(),
        },
    };
    let placement = SketchPlacement {
        origin: [0.0, 2.0, 0.0],
        normal: [0.0, 1.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 0.0, -1.0],
    };
    let request = SketchSolveRequest::new(
        "real-attached-underconstrained",
        "attached-sketch",
        vec![SketchEntity::Point {
            id: "p0".into(),
            x: 0.0,
            y: 0.0,
        }],
        Vec::new(),
    )
    .with_source_revision("solid-revision")
    .with_attachment(support.clone(), placement);
    let result = worker
        .solve(&request)
        .expect("real libslvs worker reports underconstraint");
    assert_eq!(result.status, "underconstrained");
    assert!(result.dof > 0);
    assert!(result.related_constraint_ids.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code, "solver_underconstrained");
    assert!(result.solved_coordinates.is_none());
    assert_eq!(result.support, Some(support));
    assert_eq!(result.placement, Some(placement));
}

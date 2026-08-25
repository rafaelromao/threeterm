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
    let Ok(worker) = SlvsWorker::locate() else {
        eprintln!("libslvs integration skipped: no configured worker binary");
        return;
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

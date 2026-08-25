use std::fs;
use std::path::PathBuf;

use threeterm_domain::{
    ProjectGeneration, SketchConstraint, SketchEntity, SketchPayload, SolvedCoordinate,
};
use threeterm_persistence::{Bundle, write_fresh};

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!("threeterm-sketch-replay-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

#[test]
fn solved_sketch_is_canonical_and_replays_after_reload() {
    let path = root();
    let bundle = Bundle::at(&path);
    write_fresh(&path, ProjectGeneration::with_id("sketch")).expect("fresh bundle");
    let payload = SketchPayload {
        feature_id: "rectangle".to_string(),
        entities: vec![
            SketchEntity::Point {
                id: "p0".to_string(),
                x: 0.0,
                y: 0.0,
            },
            SketchEntity::Point {
                id: "p1".to_string(),
                x: 10.0,
                y: 0.0,
            },
            SketchEntity::LineSegment {
                id: "edge".to_string(),
                start: "p0".to_string(),
                end: "p1".to_string(),
            },
        ],
        constraints: vec![SketchConstraint {
            id: "fixed-edge".to_string(),
            kind: "fixed".to_string(),
            entities: vec!["edge".to_string()],
            value: None,
        }],
        status: "solved".to_string(),
        dof: 0,
        entity_ids: vec!["p0".to_string(), "p1".to_string(), "edge".to_string()],
        related_constraint_ids: Vec::new(),
        diagnostics: Vec::new(),
        solved_coordinates: Some(vec![
            SolvedCoordinate {
                entity_id: "p0".to_string(),
                x: 0.0,
                y: 0.0,
            },
            SolvedCoordinate {
                entity_id: "p1".to_string(),
                x: 10.0,
                y: 0.0,
            },
        ]),
    };
    let before = bundle.open().expect("open baseline");
    let committed = bundle
        .append_sketch_if_revision(&payload, before.revision_hash_hex())
        .expect("sketch publishes");
    let reopened = bundle.open().expect("sketch reopens");
    assert_eq!(reopened.graph.sketch("rectangle"), Some(&payload));
    assert_ne!(committed.revision_hash_hex(), before.revision_hash_hex());
    assert_eq!(reopened.revision_hash_hex(), committed.revision_hash_hex());
    let _ = fs::remove_dir_all(path);
}

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use threeterm_domain::ProjectGeneration;
use threeterm_host::Host;
use threeterm_persistence::{Bundle, write_fresh};
use threeterm_slvs_worker::{
    SketchConstraint as WorkerSketchConstraint, SketchEntity as WorkerSketchEntity,
    SketchSolveRequest, SlvsWorker,
};
use threeterm_viewport::{CameraState, ProtocolNeutralViewport, ViewportRequest, ViewportScene};

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!("threeterm-sketch-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

#[test]
fn real_worker_commit_reload_and_viewport_use_one_production_path() {
    let Ok(worker) = SlvsWorker::locate() else {
        eprintln!("libslvs integration skipped: no configured worker binary");
        return;
    };
    let path = root();
    write_fresh(&path, ProjectGeneration::with_id("sketch-e2e")).expect("fresh bundle");
    let request = SketchSolveRequest::new(
        "host-rectangle",
        "rectangle",
        vec![
            WorkerSketchEntity::Point {
                id: "p0".into(),
                x: 0.0,
                y: 0.0,
            },
            WorkerSketchEntity::Point {
                id: "p1".into(),
                x: 10.0,
                y: 0.0,
            },
            WorkerSketchEntity::Point {
                id: "p2".into(),
                x: 10.0,
                y: 5.0,
            },
            WorkerSketchEntity::Point {
                id: "p3".into(),
                x: 0.0,
                y: 5.0,
            },
            WorkerSketchEntity::LineSegment {
                id: "e0".into(),
                start: "p0".into(),
                end: "p1".into(),
            },
            WorkerSketchEntity::LineSegment {
                id: "e1".into(),
                start: "p1".into(),
                end: "p2".into(),
            },
            WorkerSketchEntity::LineSegment {
                id: "e2".into(),
                start: "p2".into(),
                end: "p3".into(),
            },
            WorkerSketchEntity::LineSegment {
                id: "e3".into(),
                start: "p3".into(),
                end: "p0".into(),
            },
        ],
        ["p0", "p1", "p2", "p3"]
            .into_iter()
            .map(|id| WorkerSketchConstraint {
                id: format!("fixed-{id}"),
                kind: "fixed".into(),
                entities: vec![id.into()],
                value: None,
            })
            .collect(),
    );
    let host = Host::new();
    let committed = host
        .commit_sketch_solve_with_worker(&path, &request, &worker)
        .expect("host commits a solved rectangle");
    let loaded = Bundle::at(&path).open().expect("bundle reloads");
    let scene = ViewportScene::from_feature_graph(
        loaded.revision_hash_hex(),
        &loaded.graph,
        Some("rectangle".to_string()),
    );
    assert!(
        scene
            .features
            .iter()
            .any(|feature| feature.kind.starts_with("sketch-segment:"))
    );
    let frame = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new(
            loaded.revision_hash_hex(),
            1,
            160,
            120,
            CameraState::default(),
        ),
    )
    .expect("resolved sketch projects to a viewport frame");
    assert!(
        frame
            .rgb
            .chunks_exact(3)
            .any(|pixel| pixel == [105, 220, 190])
    );
    assert_eq!(committed.snapshot.revision_hash, loaded.revision_hash_hex());
    let _ = fs::remove_dir_all(path);
}

#[test]
fn lost_planar_face_support_is_explicit_and_does_not_mutate_the_bundle() {
    let path = root();
    write_fresh(&path, ProjectGeneration::with_id("sketch-lost-support")).expect("fresh bundle");
    let before = threeterm_persistence::Bundle::at(&path)
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let response = Host::new()
        .execute_domain_command(
            threeterm_protocol::schema::SKETCH_SOLVE_COMMAND_ID,
            json!({
                "bundle_path": path,
                "feature_id": "sketch-1",
                "phase": "preview",
                "entities": [{"kind": "point", "id": "p0", "x": 0.0, "y": 0.0}],
                "constraints": [],
                "support": {
                    "semantic_id": "missing/face",
                    "role": "sketch-support",
                    "provenance": {
                        "source_feature_id": "missing-solid",
                        "source_revision_id": before,
                        "source_face_id": "missing/face"
                    },
                    "evidence": {
                        "topology_kind": "planar_face",
                        "origin": [0.0, 0.0, 0.0],
                        "normal": [0.0, 1.0, 0.0],
                        "x_axis": [1.0, 0.0, 0.0],
                        "y_axis": [0.0, 0.0, -1.0],
                        "adjacent_feature_ids": []
                    }
                },
                "placement": {
                    "origin": [0.0, 0.0, 0.0],
                    "normal": [0.0, 1.0, 0.0],
                    "x_axis": [1.0, 0.0, 0.0],
                    "y_axis": [0.0, 0.0, -1.0]
                }
            }),
        )
        .expect("lost support produces a normalized response");
    assert_eq!(response["status"], "invalid_request");
    assert_eq!(response["reattachment_outcome"], "lost");
    assert_eq!(
        threeterm_persistence::Bundle::at(&path)
            .open()
            .expect("bundle remains readable")
            .revision_hash_hex(),
        before
    );
    let _ = fs::remove_dir_all(path);
}

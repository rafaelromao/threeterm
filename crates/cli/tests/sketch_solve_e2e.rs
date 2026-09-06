use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use threeterm_cli::dispatch::dispatch;
use threeterm_persistence::{Bundle, write_fresh};
use threeterm_slvs_worker::SlvsWorker;
use threeterm_viewport::{CameraState, ProtocolNeutralViewport, ViewportRequest, ViewportScene};

fn root() -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("threeterm-cli-sketch-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

#[test]
fn cli_sketch_solve_commits_and_renders_the_real_worker_result() {
    if let Err(error) = SlvsWorker::locate() {
        if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() {
            panic!("libslvs worker is required: {error}");
        }
        eprintln!("libslvs integration skipped: no configured worker binary");
        return;
    }
    let path = root();
    write_fresh(
        &path,
        threeterm_domain::ProjectGeneration::with_id("cli-sketch"),
    )
    .expect("fresh bundle");
    let request_path = path.with_extension("json");
    fs::write(
        &request_path,
        serde_json::to_vec(&json!({
            "feature_id": "rectangle",
            "entities": [
                {"kind": "point", "id": "p0", "x": 0.0, "y": 0.0},
                {"kind": "point", "id": "p1", "x": 10.0, "y": 0.0},
                {"kind": "point", "id": "p2", "x": 10.0, "y": 5.0},
                {"kind": "point", "id": "p3", "x": 0.0, "y": 5.0},
                {"kind": "line_segment", "id": "e0", "start": "p0", "end": "p1"},
                {"kind": "line_segment", "id": "e1", "start": "p1", "end": "p2"},
                {"kind": "line_segment", "id": "e2", "start": "p2", "end": "p3"},
                {"kind": "line_segment", "id": "e3", "start": "p3", "end": "p0"}
            ],
            "constraints": [
                {"id": "fixed-p0", "kind": "fixed", "entities": ["p0"]},
                {"id": "fixed-p1", "kind": "fixed", "entities": ["p1"]},
                {"id": "fixed-p2", "kind": "fixed", "entities": ["p2"]},
                {"id": "fixed-p3", "kind": "fixed", "entities": ["p3"]}
            ]
        }))
        .expect("request serializes"),
    )
    .expect("request file writes");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = dispatch(
        vec![
            "--machine".into(),
            "sketch-solve".into(),
            "--bundle".into(),
            path.clone().into_os_string(),
            "--request-file".into(),
            request_path.clone().into_os_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let response: Value = serde_json::from_slice(&stdout).expect("CLI response is JSON");
    assert_eq!(response["status"], "solved");
    assert_eq!(response["dof"], 0);
    assert_eq!(response["entity_ids"].as_array().map(Vec::len), Some(8));
    assert!(
        response["solved_coordinates"]
            .as_array()
            .is_some_and(|values| values.len() == 4)
    );

    let loaded = Bundle::at(&path).open().expect("bundle reloads");
    let scene = ViewportScene::from_feature_graph(loaded.revision_hash_hex(), &loaded.graph, None);
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
    .expect("committed rectangle renders");
    assert!(
        frame
            .rgb
            .chunks_exact(3)
            .any(|pixel| pixel == [105, 220, 190])
    );
    let _ = fs::remove_file(request_path);
    let _ = fs::remove_dir_all(path);
}

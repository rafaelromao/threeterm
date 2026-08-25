use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::OcctWorker;
use threeterm_protocol::schema::{FIT_DIMENSION_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;
use threeterm_slvs_worker::SlvsWorker;
use threeterm_viewport::ViewportScene;

fn root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-box-lid-{label}-{nanos}"))
}

fn run(bin: &str, args: &[&str]) -> Output {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm process runs");
    assert!(
        output.status.success(),
        "threeterm {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn sketch_request(path: &Path, feature_id: &str, dimension_id: &str, value: f64) {
    let p0 = format!("{feature_id}-p0");
    let p1 = format!("{feature_id}-p1");
    let e0 = format!("{feature_id}-edge");
    let request = json!({
        "feature_id": feature_id,
        "entities": [
            {"kind": "point", "id": p0, "x": 0.0, "y": 0.0},
            {"kind": "point", "id": p1, "x": value, "y": 0.0},
            {"kind": "line_segment", "id": e0, "start": format!("{feature_id}-p0"), "end": format!("{feature_id}-p1")}
        ],
        "constraints": [
            {"id": format!("{feature_id}-anchor"), "kind": "fixed", "entities": [format!("{feature_id}-p0")]},
            {"id": dimension_id, "kind": "distance", "entities": [format!("{feature_id}-p0"), format!("{feature_id}-p1")], "value": value},
            {"id": format!("{feature_id}-horizontal"), "kind": "horizontal", "entities": [e0]}
        ]
    });
    fs::write(
        path,
        serde_json::to_vec(&request).expect("sketch request serializes"),
    )
    .expect("sketch request writes");
}

fn extrude(bin: &str, bundle: &Path, feature_id: &str, profile: &Path, height: &str) {
    run(
        bin,
        &[
            "--machine",
            "extrude",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            feature_id,
            "--profile-file",
            profile.to_str().unwrap(),
            "--height",
            height,
        ],
    );
}

#[test]
fn box_with_lid_runs_project_sketch_fit_extrude_viewport_export_reload() {
    if OcctWorker::locate().is_err() || SlvsWorker::locate().is_err() {
        eprintln!("box_with_lid_e2e: real OCCT and libslvs workers are required");
        return;
    }
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = root("bundle");
    let output = root("output");
    let box_sketch = root("box-sketch");
    let lid_sketch = root("lid-sketch");
    let box_profile = root("box-profile");
    let lid_profile = root("lid-profile");
    fs::write(&box_profile, "[[0,0],[10,0],[10,8],[0,8]]").expect("box profile writes");
    fs::write(&lid_profile, "[[0.2,0.2],[9.8,0.2],[9.8,7.8],[0.2,7.8]]")
        .expect("lid profile writes");
    sketch_request(&box_sketch, "box-sketch", "box-width", 10.0);
    sketch_request(&lid_sketch, "lid-sketch", "lid-width", 9.6);

    run(bin, &["--machine", "new-project", bundle.to_str().unwrap()]);
    run(
        bin,
        &[
            "--machine",
            "sketch-solve",
            "--bundle",
            bundle.to_str().unwrap(),
            "--request-file",
            box_sketch.to_str().unwrap(),
        ],
    );
    run(
        bin,
        &[
            "--machine",
            "sketch-solve",
            "--bundle",
            bundle.to_str().unwrap(),
            "--request-file",
            lid_sketch.to_str().unwrap(),
        ],
    );
    extrude(bin, &bundle, "box", &box_profile, "4");
    extrude(bin, &bundle, "lid", &lid_profile, "1");

    let loaded: Value =
        serde_json::from_slice(&run(bin, &["--machine", "load", bundle.to_str().unwrap()]).stdout)
            .expect("load response parses");
    let expected_revision = loaded["revision_hash"].as_str().unwrap();
    let fit_output = run(
        bin,
        &[
            "--machine",
            "fit-dimension",
            "--bundle",
            bundle.to_str().unwrap(),
            "--expected-revision",
            expected_revision,
            "--source-feature-id",
            "box-sketch",
            "--target-feature-id",
            "lid-sketch",
            "--source-dimension-id",
            "box-width",
            "--target-dimension-id",
            "lid-width",
            "--dimension",
            "width",
            "--clearance",
            "0.2",
        ],
    );
    let fit: Value = serde_json::from_slice(&fit_output.stdout).expect("fit response parses");
    validate(
        &find(FIT_DIMENSION_COMMAND_ID)
            .expect("fit-dimension is registered")
            .response_schema,
        &fit,
    )
    .expect("fit response validates");
    assert_eq!(fit["fit"]["source_value"], 10.0);
    assert_eq!(fit["fit"]["target_value"], 9.6);

    let fresh_host = Host::new();
    fresh_host.load(&bundle).expect("fresh host reloads");
    let presentation = fresh_host
        .presentation_snapshot()
        .expect("presentation exists");
    let scene = ViewportScene::from_feature_graph(
        presentation.snapshot.revision_hash.clone(),
        &presentation.graph,
        None,
    );
    assert!(scene.features.iter().any(|feature| feature.id == "box"));
    assert!(scene.features.iter().any(|feature| feature.id == "lid"));
    assert_eq!(scene.fit_relationships.len(), 1);

    run(
        bin,
        &[
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "box",
            "--body-ids",
            "box,lid",
            "--formats",
            "stl,3mf",
            "--output-dir",
            output.to_str().unwrap(),
        ],
    );
    run(
        bin,
        &[
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "lid",
            "--formats",
            "stl",
            "--output-dir",
            output.to_str().unwrap(),
        ],
    );
    assert!(output.join("box.stl").is_file());
    assert!(output.join("lid.stl").is_file());
    assert!(output.join("box.3mf").is_file());

    let before_manifest = fs::read(bundle.join("manifest.json")).unwrap();
    let before_log = fs::read(bundle.join("transactions.log")).unwrap();
    let before_box = fs::read(bundle.join("brep/box.brep")).unwrap();
    let invalid = Command::new(bin)
        .args([
            "--machine",
            "fit-dimension",
            "--bundle",
            bundle.to_str().unwrap(),
            "--expected-revision",
            fit["revision_hash"].as_str().unwrap(),
            "--source-feature-id",
            "box-sketch",
            "--target-feature-id",
            "lid-sketch",
            "--source-dimension-id",
            "box-width",
            "--target-dimension-id",
            "lid-width",
            "--dimension",
            "width",
            "--clearance",
            "0.3",
        ])
        .output()
        .expect("invalid fit runs");
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("fit dimension"));
    assert_eq!(
        fs::read(bundle.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(
        fs::read(bundle.join("transactions.log")).unwrap(),
        before_log
    );
    assert_eq!(fs::read(bundle.join("brep/box.brep")).unwrap(), before_box);

    let reloaded: Value =
        serde_json::from_slice(&run(bin, &["--machine", "load", bundle.to_str().unwrap()]).stdout)
            .expect("reload response parses");
    assert_eq!(reloaded["revision_hash"], fit["revision_hash"]);
    let _ = fs::remove_dir_all(&bundle);
    let _ = fs::remove_dir_all(&output);
    let _ = fs::remove_file(box_sketch);
    let _ = fs::remove_file(lid_sketch);
    let _ = fs::remove_file(box_profile);
    let _ = fs::remove_file(lid_profile);
}

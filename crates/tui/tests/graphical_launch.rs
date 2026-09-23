use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_host::{Host, stl_integrity};
use threeterm_occt_worker::{BracketRequest, ExtrudeRequest, OcctWorker, new_request_id};
use threeterm_persistence::{Bundle, CanonicalIntent, LogEntry};
use threeterm_tui::TuiSession;
use threeterm_viewport::{SceneSolid, ViewportScene};

fn solid_for<'a>(scene: &'a ViewportScene, feature_id: &str) -> &'a SceneSolid {
    scene
        .solids
        .iter()
        .find(|solid| solid.feature_id == feature_id)
        .unwrap_or_else(|| panic!("scene has no solid for {feature_id}"))
}

fn positive_volume(solid: &SceneSolid) -> f64 {
    solid
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            let cross = [
                b[1] * c[2] - b[2] * c[1],
                b[2] * c[0] - b[0] * c[2],
                b[0] * c[1] - b[1] * c[0],
            ];
            (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0
        })
        .sum::<f64>()
        .abs()
}

fn assert_authenticated_brep(root: &Path, entry: &LogEntry) {
    let relative_path = entry
        .brep_path
        .as_deref()
        .expect("graphical feature records its BREP path");
    let path = root.join(relative_path);
    let bytes = fs::read(&path).expect("graphical BREP reads");
    assert_eq!(entry.brep_byte_count, Some(bytes.len() as u64));
    let digest = threeterm_occt_worker::sha256_file(&path).expect("graphical BREP hashes");
    assert_eq!(entry.brep_sha256.as_deref(), Some(digest.as_str()));
}

fn section_bounds(solid: &SceneSolid, z: f64) -> Option<[f64; 4]> {
    let mut points = Vec::new();
    for triangle in &solid.triangles {
        let vertices = triangle.vertices;
        for vertex in vertices {
            if (vertex[2] - z).abs() <= 0.05 {
                points.push([vertex[0], vertex[1]]);
            }
        }
        for [a, b] in [
            [vertices[0], vertices[1]],
            [vertices[1], vertices[2]],
            [vertices[2], vertices[0]],
        ] {
            let a_delta = a[2] - z;
            let b_delta = b[2] - z;
            if a_delta * b_delta < 0.0 {
                let t = a_delta / (a_delta - b_delta);
                points.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
            }
        }
    }
    if points.is_empty() {
        return None;
    }
    let mut bounds = [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for [x, y] in points {
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].max(x);
        bounds[2] = bounds[2].min(y);
        bounds[3] = bounds[3].max(y);
    }
    Some(bounds)
}

fn assert_reinforcement_geometry(scene: &ViewportScene) {
    let tapered = solid_for(scene, "tapered-reinforcement");
    let lofted = solid_for(scene, "lofted-gusset");
    assert!(positive_volume(tapered) > 0.05);
    assert!(positive_volume(lofted) > 0.05);
    for solid in [tapered, lofted] {
        let mut edges = BTreeMap::<([i64; 3], [i64; 3]), usize>::new();
        let vertex_key = |vertex: [f64; 3]| {
            [
                (vertex[0] * 1000.0).round() as i64,
                (vertex[1] * 1000.0).round() as i64,
                (vertex[2] * 1000.0).round() as i64,
            ]
        };
        for triangle in &solid.triangles {
            let keys = triangle.vertices.map(vertex_key);
            for [left, right] in [[keys[0], keys[1]], [keys[1], keys[2]], [keys[2], keys[0]]] {
                let edge = if left <= right {
                    (left, right)
                } else {
                    (right, left)
                };
                *edges.entry(edge).or_default() += 1;
            }
        }
        assert!(edges.values().all(|count| *count == 2));
    }
    let tapered_bottom = section_bounds(tapered, 0.0).expect("tapered lower section exists");
    let tapered_top = section_bounds(tapered, 12.0).expect("tapered upper section exists");
    assert!(
        (tapered_bottom[1] - tapered_bottom[0] - (tapered_top[1] - tapered_top[0])).abs() > 0.05
            || (tapered_bottom[3] - tapered_bottom[2] - (tapered_top[3] - tapered_top[2])).abs()
                > 0.05
    );
    let lofted_lower = section_bounds(lofted, 8.0).expect("loft lower section exists");
    let lofted_upper = section_bounds(lofted, 18.0).expect("loft upper section exists");
    for (actual, expected) in [
        (lofted_lower, [8.0, 16.0, 8.0, 16.0]),
        (lofted_upper, [10.0, 14.0, 10.0, 14.0]),
    ] {
        for (value, target) in actual.into_iter().zip(expected) {
            assert!((value - target).abs() <= 0.05);
        }
    }
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let relative = current
            .strip_prefix(root)
            .expect("snapshot path stays below its root")
            .to_path_buf();
        snapshot.insert(relative, None);
        for entry in fs::read_dir(current).expect("snapshot directory reads") {
            let path = entry.expect("snapshot entry reads").path();
            let relative = path
                .strip_prefix(root)
                .expect("snapshot entry stays below its root")
                .to_path_buf();
            if path.is_dir() {
                visit(root, &path, snapshot);
            } else {
                snapshot.insert(relative, Some(fs::read(path).expect("snapshot file reads")));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn assert_prism_geometry(scene: &ViewportScene) {
    assert_eq!(scene.solids.len(), 1);
    let solid = &scene.solids[0];
    assert_eq!(solid.feature_id, "keyboard-extrude");
    assert!(!solid.triangles.is_empty());
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    let mut signed_volume = 0.0;
    let mut edges = BTreeMap::<([i64; 3], [i64; 3]), usize>::new();
    let mut adjacency = BTreeMap::<[i64; 3], BTreeSet<[i64; 3]>>::new();
    let vertex_key = |vertex: [f64; 3]| {
        [
            (vertex[0] * 1000.0).round() as i64,
            (vertex[1] * 1000.0).round() as i64,
            (vertex[2] * 1000.0).round() as i64,
        ]
    };
    for triangle in &solid.triangles {
        let [a, b, c] = triangle.vertices;
        for vertex in [a, b, c] {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex[axis]);
                maximum[axis] = maximum[axis].max(vertex[axis]);
            }
        }
        let cross = [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ];
        signed_volume += (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0;
        let keys = [vertex_key(a), vertex_key(b), vertex_key(c)];
        for [left, right] in [[keys[0], keys[1]], [keys[1], keys[2]], [keys[2], keys[0]]] {
            let edge = if left <= right {
                (left, right)
            } else {
                (right, left)
            };
            *edges.entry(edge).or_default() += 1;
            adjacency.entry(left).or_default().insert(right);
            adjacency.entry(right).or_default().insert(left);
        }
    }
    for (axis, expected) in [(0, 10.0), (1, 5.0), (2, 3.0)] {
        assert!(minimum[axis].abs() < 0.1);
        assert!((maximum[axis] - expected).abs() < 0.1);
    }
    assert!((signed_volume.abs() - 150.0).abs() < 1.0);
    assert!(edges.values().all(|count| *count == 2));
    let first_vertex = *adjacency.keys().next().expect("solid has vertices");
    let mut connected = BTreeSet::from([first_vertex]);
    let mut pending = VecDeque::from([first_vertex]);
    while let Some(vertex) = pending.pop_front() {
        for neighbor in &adjacency[&vertex] {
            if connected.insert(*neighbor) {
                pending.push_back(*neighbor);
            }
        }
    }
    assert_eq!(connected.len(), adjacency.len());
}

#[test]
#[ignore = "requires the qualified graphical Ghostty toolchain"]
fn production_tui_ghostty_session() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-graphical-launch-{}-{suffix}",
        std::process::id()
    ));
    let evidence = root.join("evidence");
    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("graphical launch requires the OCCT worker: {error}"));
    Host::new()
        .create_bracket(
            &root,
            BracketRequest::new("graphical-l-bracket", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("l-bracket"),
            &worker,
        )
        .expect("the graphical launch starts from a real L-bracket Project Generation");

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_ghostty_session")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("graphical runner starts");
    assert!(
        output.status.success(),
        "graphical runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("graphical evidence manifest exists"),
    )
    .expect("graphical evidence manifest is JSON");
    assert_eq!(manifest["schema_version"], "threeterm.graphical-tui/1");
    assert_eq!(manifest["result"], "passed");
    assert_eq!(manifest["test"], "production_tui_ghostty_session");
    assert_eq!(manifest["events"]["probe"], "passed");
    assert_eq!(manifest["events"]["readiness"], "passed");
    assert_eq!(manifest["events"]["orbit"], "passed");
    assert_eq!(manifest["events"]["cleanup"], "passed");
    assert_eq!(manifest["configuration"]["palette"], "catppuccin");
    let startup = &manifest["viewport"]["startup"];
    let orbit = &manifest["viewport"]["orbit"];
    assert_eq!(startup["schema_version"], "threeterm.viewport-evidence/1");
    assert_eq!(startup["frame"]["width"], 800);
    assert_eq!(startup["frame"]["height"], 480);
    assert_eq!(startup["palette"]["name"], "catppuccin");
    assert_eq!(
        startup["palette"]["colors"]["body"],
        serde_json::json!([125, 125, 152])
    );
    assert!(startup["scene"]["solids"].as_array().is_some_and(|solids| {
        solids.iter().any(|solid| {
            solid["feature_id"] == "l-bracket" && solid["triangle_count"].as_u64().unwrap_or(0) > 0
        })
    }));
    assert!(startup["scene"]["body_pixels"].as_u64().unwrap_or(0) > 0);
    assert_ne!(startup["frame"]["image_id"], orbit["frame"]["image_id"]);
    assert_ne!(
        startup["camera"]["yaw_degrees"],
        orbit["camera"]["yaw_degrees"]
    );
    assert_eq!(startup["frame"]["revision"], orbit["frame"]["revision"]);
    assert_eq!(
        manifest["cleanup_evidence"]["final_image_id"],
        manifest["cleanup_evidence"]["final_delete_image_id"]
    );
    assert!(
        manifest["cleanup_evidence"]["deletions"]
            .as_array()
            .is_some_and(|deletions| deletions.len() >= 2)
    );
    assert_eq!(manifest["processes"]["owned"], "stopped");
    assert!(manifest["artifacts"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["kind"] == "startup_screenshot")
            && items
                .iter()
                .any(|item| item["kind"] == "cleanup_screenshot")
            && items.iter().all(|item| item["sha256"].as_str().is_some())
    }));
}

#[test]
#[ignore = "requires the qualified graphical Ghostty and OCCT toolchain"]
fn production_tui_save_reopen_validate_export() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-graphical-lifecycle-{}-{suffix}",
        std::process::id()
    ));
    let evidence = root.join("evidence");
    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("graphical lifecycle requires the OCCT worker: {error}"));
    Host::new()
        .create_bracket(
            &root,
            BracketRequest::new("graphical-lifecycle", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("l-bracket"),
            &worker,
        )
        .expect("the graphical lifecycle starts from a real L-bracket Project Generation");

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_save_reopen_validate_export")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("graphical lifecycle runner starts");
    assert!(
        output.status.success(),
        "graphical lifecycle runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("graphical lifecycle manifest exists"),
    )
    .expect("graphical lifecycle manifest is JSON");
    assert_eq!(
        manifest["schema_version"],
        "threeterm.graphical-tui.save-reopen-validate-export/1"
    );
    assert_eq!(manifest["result"], "passed");
    assert_eq!(manifest["events"]["lifecycle"], "passed");
    assert_eq!(manifest["events"]["validation"], "passed");
    assert_eq!(manifest["events"]["export"], "passed");
    assert_eq!(manifest["events"]["cleanup"], "passed");
    assert_eq!(manifest["processes"]["owned"], "stopped");

    let stl_path = root.join("tui-export/l-bracket.stl");
    let report = stl_integrity::verify_path(&stl_path)
        .expect("graphical TUI export passes the shared independent STL checker");
    assert_eq!(report.format, stl_integrity::StlFormat::Ascii);
    assert!(report.triangle_count > 0);
    assert_eq!(report.shell_count, 1);
    assert!(report.material_volume > 0.0);
    assert!(manifest["artifacts"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == "stl_integrity"
                && item["path"]
                    == evidence
                        .join("stl-integrity.json")
                        .to_string_lossy()
                        .as_ref()
                && item["sha256"].as_str().is_some()
        }) && items.iter().any(|item| {
            item["kind"] == "exported_stl"
                && item["path"] == stl_path.to_string_lossy().as_ref()
                && item["sha256"].as_str().is_some()
        })
    }));

    fs::remove_dir_all(root).expect("graphical lifecycle evidence removes");
}

#[test]
#[ignore = "requires the qualified graphical Ghostty toolchain"]
fn production_tui_mirror_pattern_reinforcing_features() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-graphical-reinforcing-{}-{suffix}",
        std::process::id()
    ));
    let evidence = std::env::temp_dir().join(format!(
        "threeterm-graphical-reinforcing-evidence-{}-{suffix}",
        std::process::id()
    ));
    let worker = OcctWorker::locate().unwrap_or_else(|error| {
        panic!("graphical reinforcing workflow requires the OCCT worker: {error}")
    });
    let host = Host::new();
    host.create_bracket(
        &root,
        BracketRequest::new("graphical-reinforcing", 60.0, 30.0, 40.0, 3.0)
            .with_feature_id("l-bracket"),
        &worker,
    )
    .expect("graphical reinforcing workflow starts from an L-bracket");
    host.extrude(
        &root,
        ExtrudeRequest::new(
            new_request_id(),
            vec![(27.0, 12.0), (30.0, 12.0), (30.0, 15.0), (27.0, 15.0)],
            5.0,
        )
        .with_output_path(root.join("stage"), "reinforce-pad.brep")
        .with_feature_id("reinforce-pad"),
        &worker,
    )
    .expect("graphical reinforcing workflow persists its pad fixture");

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_mirror_pattern_reinforcing_features")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("graphical reinforcing runner starts");
    assert!(
        output.status.success(),
        "graphical reinforcing runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("graphical reinforcing manifest exists"),
    )
    .expect("graphical reinforcing manifest is JSON");
    assert_eq!(
        manifest["schema_version"],
        "threeterm.graphical-tui.mirror-pattern-reinforcing-features/1"
    );
    assert_eq!(manifest["result"], "passed");
    assert_eq!(
        manifest["test"],
        "production_tui_mirror_pattern_reinforcing_features"
    );
    assert_eq!(manifest["events"]["workflow"], "passed");
    assert_eq!(manifest["events"]["orbit"], "passed");
    assert_eq!(manifest["events"]["cleanup"], "passed");
    let expected_features = vec![
        "l-bracket",
        "reinforce-pad",
        "tui-circular-lugs",
        "tui-linear-pads",
        "tui-mirror-pad",
    ];
    let workflow_solids = manifest["viewport"]["workflow"]["scene"]["solids"]
        .as_array()
        .expect("workflow viewport contains solids");
    let mut actual_features = workflow_solids
        .iter()
        .map(|solid| {
            assert!(solid["triangle_count"].as_u64().unwrap_or(0) > 0);
            solid["feature_id"]
                .as_str()
                .expect("workflow solid feature id is a string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    actual_features.sort();
    assert_eq!(
        actual_features,
        expected_features
            .iter()
            .map(|feature_id| (*feature_id).to_owned())
            .collect::<Vec<_>>()
    );
    for kind in [
        "pty_output",
        "pty_input",
        "mirror_screenshot",
        "linear_pattern_screenshot",
        "circular_pattern_screenshot",
        "reinforcing_transcript",
        "orbit_screenshot",
        "cleanup_screenshot",
    ] {
        assert!(
            manifest["artifacts"].as_array().is_some_and(|items| items
                .iter()
                .any(|item| item["kind"] == kind && item["sha256"].as_str().is_some())),
            "graphical reinforcing evidence is missing artifact {kind}"
        );
    }

    let transcript = fs::read_to_string(evidence.join("reinforcing-transcript.jsonl"))
        .expect("reinforcing transcript exists");
    let transcript_entries = transcript
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("reinforcing transcript line is JSON")
        })
        .collect::<Vec<_>>();
    let transcript_commands = transcript_entries
        .iter()
        .map(|entry| {
            entry["command"]
                .as_str()
                .expect("reinforcing transcript command is a string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        transcript_commands,
        vec!["mirror", "mirror", "linear-pattern", "circular-pattern"]
    );
    assert_eq!(
        transcript_entries[0]["cancellation_marker"],
        "[cancellation-glyph] Cancellation: command draft discarded"
    );
    for entry in transcript_entries.iter().skip(1) {
        assert!(entry["request"].is_object());
        assert!(entry["preview_marker"].as_str().is_some());
        assert!(entry["commit_marker"].as_str().is_some());
        assert!(entry["response"]["feature_id"].as_str().is_some());
        assert!(entry["response"]["revision"].as_str().is_some());
        assert!(entry["viewport_evidence"]["acknowledgement"] == "viewport-presented");
        assert!(entry["screenshot"]["sha256"].as_str().is_some());
    }
    let pty_output =
        fs::read_to_string(evidence.join("pty-output.log")).expect("reinforcing PTY output exists");
    for marker in [
        "[cancellation-glyph] Cancellation: command draft discarded",
        "[selection-glyph] Commit: mirror",
        "[selection-glyph] Commit: linear-pattern",
        "[selection-glyph] Commit: circular-pattern",
    ] {
        assert!(
            pty_output.contains(marker),
            "PTY output is missing {marker}"
        );
    }

    let bundle = Bundle::at(&root)
        .open_read_only()
        .expect("graphical reinforcing project opens read-only");
    assert_eq!(bundle.log.len(), 5);
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("transaction log reads");
    for feature_id in &expected_features {
        assert!(
            root.join("brep")
                .join(format!("{feature_id}.brep"))
                .is_file(),
            "retained BREP missing for {feature_id}"
        );
    }
    let identity = host.identity(&root).expect("reinforcing identity reads");
    assert_eq!(identity.transaction_count, 5);
    fs::remove_dir_all(root.join("brep")).expect("derived reinforcing BREP directory removes");
    let replayed_host = Host::new();
    replayed_host
        .load_with_geometry_replay(&root)
        .expect("fresh host replays graphical reinforcing geometry");
    assert_eq!(
        replayed_host
            .identity(&root)
            .expect("replay identity reads"),
        identity
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("replayed manifest reads"),
        before_manifest
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("replayed transaction log reads"),
        before_log
    );
    let replayed_scene = replayed_host
        .presentation_viewport_scene()
        .expect("replayed reinforcing viewport scene reads");
    let mut replayed_features = replayed_scene
        .solids
        .iter()
        .map(|solid| {
            assert!(!solid.triangles.is_empty());
            solid.feature_id.as_str()
        })
        .collect::<Vec<_>>();
    replayed_features.sort_unstable();
    assert_eq!(replayed_features, expected_features);

    fs::remove_dir_all(root).expect("graphical reinforcing project root removes");
    eprintln!(
        "retained graphical reinforcing evidence: {}",
        evidence.display()
    );
}

#[test]
#[ignore = "requires the qualified graphical Ghostty toolchain"]
fn production_tui_keyboard_navigation() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-graphical-navigation-{}-{suffix}",
        std::process::id()
    ));
    let evidence = std::env::temp_dir().join(format!(
        "threeterm-graphical-navigation-evidence-{}-{suffix}",
        std::process::id()
    ));
    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("graphical navigation requires the OCCT worker: {error}"));
    Host::new()
        .create_bracket(
            &root,
            BracketRequest::new("graphical-navigation", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("l-bracket"),
            &worker,
        )
        .expect("the graphical navigation starts from a real L-bracket Project Generation");

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_keyboard_navigation")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("graphical navigation runner starts");
    assert!(
        output.status.success(),
        "graphical navigation runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("graphical navigation manifest exists"),
    )
    .expect("graphical navigation manifest is JSON");
    assert_eq!(
        manifest["schema_version"],
        "threeterm.graphical-tui.keyboard-navigation/1"
    );
    assert_eq!(manifest["result"], "passed");
    assert_eq!(manifest["test"], "production_tui_keyboard_navigation");
    assert_eq!(manifest["events"]["probe"], "passed");
    assert_eq!(manifest["events"]["readiness"], "passed");
    assert_eq!(manifest["events"]["navigation"], "passed");
    assert_eq!(manifest["events"]["cleanup"], "passed");
    assert_eq!(manifest["configuration"]["locale"], "C.UTF-8");
    assert_eq!(manifest["configuration"]["palette"], "catppuccin");
    assert_eq!(manifest["configuration"]["compositor"]["width"], 800);
    assert_eq!(manifest["configuration"]["compositor"]["height"], 600);
    assert_eq!(manifest["configuration"]["terminal"]["columns"], 80);
    assert_eq!(manifest["configuration"]["terminal"]["rows"], 24);

    let selection = &manifest["viewport"]["selection"];
    assert_eq!(selection["selected_feature_id"], "l-bracket");
    assert_eq!(selection["camera"]["yaw_degrees"], 0);
    assert_eq!(selection["camera"]["pitch_degrees"], 25);
    assert_eq!(selection["camera"]["zoom_percent"], 100);
    assert_eq!(selection["camera"]["pan_x"], 0);
    assert_eq!(selection["camera"]["pan_y"], 0);
    assert_eq!(manifest["viewport"]["orbit"]["camera"]["yaw_degrees"], 5);
    assert_eq!(manifest["viewport"]["pan"]["camera"]["pan_y"], -5);
    assert_eq!(manifest["viewport"]["zoom"]["camera"]["zoom_percent"], 105);
    for viewport_name in ["selection", "orbit", "pan", "zoom"] {
        let viewport = &manifest["viewport"][viewport_name];
        assert!(viewport["scene"]["body_pixels"].as_u64().unwrap_or(0) > 0);
        assert!(viewport["frame"]["image_id"].as_u64().unwrap_or(0) > 0);
        assert_eq!(viewport["palette"]["name"], "catppuccin");
    }
    assert_eq!(
        manifest["navigation"]["project_generation_digest_before"],
        manifest["navigation"]["project_generation_digest_after"]
    );
    let startup_revision = manifest["viewport"]["startup"]["frame"]["revision"].clone();
    for viewport_name in ["selection", "orbit", "pan", "zoom"] {
        assert_eq!(
            manifest["viewport"][viewport_name]["frame"]["revision"], startup_revision,
            "{viewport_name} must remain bound to the saved project revision"
        );
    }
    assert_eq!(
        manifest["cleanup_evidence"]["final_image_id"],
        manifest["cleanup_evidence"]["final_delete_image_id"]
    );
    for kind in [
        "pty_output",
        "pty_input",
        "navigation_transcript",
        "startup_viewport_crop",
        "selection_screenshot",
        "orbit_screenshot",
        "pan_screenshot",
        "zoom_screenshot",
        "selection_viewport_crop",
        "orbit_viewport_crop",
        "pan_viewport_crop",
        "zoom_viewport_crop",
        "cleanup_screenshot",
    ] {
        assert!(
            manifest["artifacts"].as_array().is_some_and(|items| items
                .iter()
                .any(|item| item["kind"] == kind && item["sha256"].as_str().is_some())),
            "navigation evidence is missing artifact {kind}"
        );
    }
    let transcript = fs::read_to_string(evidence.join("navigation-transcript.jsonl"))
        .expect("navigation transcript exists");
    let transcript_actions = transcript
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("navigation transcript line is JSON")
        })
        .map(|entry| {
            entry["action"]
                .as_str()
                .expect("navigation transcript action is a string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        transcript_actions,
        vec!["selection", "orbit", "pan", "zoom"]
    );
    let pty_output =
        fs::read_to_string(evidence.join("pty-output.log")).expect("PTY output exists");
    for marker in [
        "selected feature l-bracket",
        "Orbit right",
        "Pan up",
        "Zoom in",
    ] {
        assert!(
            pty_output.contains(marker),
            "PTY output is missing {marker}"
        );
    }

    fs::remove_dir_all(root).expect("graphical navigation project root removes");
    eprintln!(
        "retained graphical navigation evidence: {}",
        evidence.display()
    );
}

#[test]
#[ignore = "requires the qualified graphical Ghostty toolchain"]
fn production_tui_create_project_extrude() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let workspace = std::env::temp_dir().join(format!(
        "threeterm-graphical-fresh-launch-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&workspace).expect("graphical fresh workspace creates");
    let root = workspace.join("launch-placeholder");
    let created = workspace.join("launch-placeholder-created");
    let evidence = workspace.join("evidence");
    assert!(!root.exists());
    assert!(!created.exists());

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_create_project_extrude")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("fresh graphical runner starts");
    assert!(
        output.status.success(),
        "fresh graphical runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("fresh graphical evidence exists"),
    )
    .expect("fresh graphical evidence is JSON");
    assert_eq!(
        manifest["schema_version"],
        "threeterm.graphical-tui.create-project-extrude/1"
    );
    assert_eq!(manifest["result"], "passed");
    assert_eq!(manifest["test"], "production_tui_create_project_extrude");
    assert_eq!(manifest["events"]["probe"], "passed");
    assert_eq!(manifest["events"]["readiness"], "passed");
    assert_eq!(manifest["events"]["workflow"], "passed");
    assert_eq!(manifest["events"]["orbit"], "passed");
    assert_eq!(manifest["events"]["cleanup"], "passed");
    assert_eq!(
        manifest["viewport"]["startup"]["frame"]["revision"],
        "empty-session-source"
    );
    assert!(
        !serde_json::to_string(&manifest)
            .expect("fresh graphical manifest serializes")
            .contains("empty-project")
    );
    assert_eq!(
        manifest["viewport"]["startup"]["scene"]["solids"],
        serde_json::json!([])
    );
    assert_eq!(
        manifest["viewport"]["workflow"]["scene"]["solids"][0]["feature_id"],
        "keyboard-extrude"
    );
    assert!(
        manifest["viewport"]["workflow"]["scene"]["triangle_count"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert_eq!(
        manifest["viewport"]["workflow"]["frame"]["revision"],
        manifest["viewport"]["orbit"]["frame"]["revision"]
    );
    assert_ne!(
        manifest["viewport"]["workflow"]["frame"]["image_id"],
        manifest["viewport"]["orbit"]["frame"]["image_id"]
    );
    assert_ne!(
        manifest["viewport"]["workflow"]["camera"]["yaw_degrees"],
        manifest["viewport"]["orbit"]["camera"]["yaw_degrees"]
    );
    assert_eq!(
        manifest["cleanup_evidence"]["final_image_id"],
        manifest["cleanup_evidence"]["final_delete_image_id"]
    );

    let identity: Value = serde_json::from_slice(
        &fs::read(evidence.join("project-identity.json")).expect("project identity exists"),
    )
    .expect("project identity is JSON");
    assert_eq!(
        identity["project_identity"]["bundle_path"],
        created.to_string_lossy().as_ref()
    );
    assert_eq!(identity["project_identity"]["transaction_count"], 1);
    assert_eq!(identity["intent"]["feature_id"], "keyboard-extrude");
    assert_eq!(
        identity["intent"]["profile"],
        serde_json::json!([[0, 0], [10, 0], [10, 5], [0, 5]])
    );
    assert_eq!(identity["intent"]["height"], 3);
    assert_eq!(identity["intent"]["mode"], "additive");

    assert!(!root.exists());
    let pty_output =
        fs::read_to_string(evidence.join("pty-output.log")).expect("PTY output exists");
    assert!(pty_output.contains("selected feature keyboard-extrude"));
    assert!(!pty_output.contains("empty-project"));
    for kind in [
        "pty_output",
        "pty_input",
        "startup_screenshot",
        "project_created_screenshot",
        "extrusion_committed_screenshot",
        "orbit_screenshot",
        "cleanup_screenshot",
        "project_identity",
        "created_project_manifest",
        "derived_brep",
    ] {
        assert!(
            manifest["artifacts"].as_array().is_some_and(|items| items
                .iter()
                .any(|item| item["kind"] == kind && item["sha256"].as_str().is_some())),
            "fresh graphical evidence is missing artifact {kind}"
        );
    }
    let before_inspection = snapshot_tree(&created);
    let bundle = Bundle::at(&created)
        .open_read_only()
        .expect("fresh graphical project opens read-only");
    assert_eq!(snapshot_tree(&created), before_inspection);
    let verifier = Host::new();
    let scene = verifier
        .read_only_viewport_scene(&created)
        .expect("fresh graphical project loads read-only");
    assert_prism_geometry(&scene);
    let mut selector = TuiSession::from_feature_graph(&bundle.graph, bundle.revision_hash_hex());
    let selected = selector
        .process_terminal_input(b"\x1b[B")
        .expect("committed feature is keyboard-selectable");
    assert_eq!(
        selected.frame.selected_target.as_deref(),
        Some("keyboard-extrude")
    );
    assert_eq!(snapshot_tree(&created), before_inspection);
    assert_eq!(bundle.log.len(), 1);
    let intent = bundle.log.entries()[0]
        .intent
        .as_ref()
        .expect("fresh graphical extrusion retains intent");
    let CanonicalIntent::Extrude(intent) = intent else {
        panic!("fresh graphical workflow retained a non-extrusion intent");
    };
    assert_eq!(intent.affected_semantic_ids, ["keyboard-extrude"]);
    assert_eq!(
        intent.deterministic_inputs.profile,
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]
    );
    assert_eq!(intent.deterministic_inputs.height, 3.0);
    assert_eq!(intent.mode, "additive");
    assert_ne!(bundle.manifest.revision_hash, "empty-project");
    assert!(
        !serde_json::to_string(bundle.log.entries())
            .expect("log entries serialize")
            .contains("empty-project")
    );

    fs::remove_dir_all(workspace).expect("fresh graphical workspace removes");
}

#[test]
#[ignore = "requires the qualified graphical Ghostty toolchain"]
fn production_tui_tapered_lofted_reinforcements() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let workspace = std::env::temp_dir().join(format!(
        "threeterm-graphical-tapered-lofted-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&workspace).expect("graphical reinforcement workspace creates");
    let root = workspace.join("project");
    let evidence = workspace.join("evidence");
    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("graphical reinforcement requires OCCT worker: {error}"));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("graphical reinforcement seed project persists");
    host.extrude(
        &root,
        ExtrudeRequest::new(
            "taper-seed-request",
            vec![(24.0, 0.0), (34.0, 0.0), (34.0, 4.0), (24.0, 4.0)],
            12.0,
        )
        .with_feature_id("taper-seed"),
        &worker,
    )
    .expect("graphical reinforcement seed extrusion commits");
    let before = host
        .identity(&root)
        .expect("graphical reinforcement seed identity reads");
    let seeded_tree = snapshot_tree(&root);

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_tapered_lofted_reinforcements")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("graphical reinforcement runner starts");
    assert!(
        output.status.success(),
        "graphical reinforcement runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("reinforcement manifest exists"),
    )
    .expect("reinforcement manifest is JSON");
    assert_eq!(
        manifest["schema_version"],
        "threeterm.graphical-tui.tapered-lofted-reinforcements/1"
    );
    assert_eq!(manifest["result"], "passed");
    assert_eq!(
        manifest["test"],
        "production_tui_tapered_lofted_reinforcements"
    );
    assert_eq!(manifest["events"]["workflow"], "passed");
    assert_eq!(manifest["events"]["orbit"], "passed");
    assert_ne!(snapshot_tree(&root), seeded_tree);
    for stage in ["tapered", "lofted", "orbit"] {
        let scene = &manifest["viewport"][stage]["scene"];
        assert!(scene["triangle_count"].as_u64().unwrap_or(0) > 0);
        assert!(scene["body_pixels"].as_u64().unwrap_or(0) > 0);
        assert!(scene["edge_pixels"].as_u64().unwrap_or(0) > 0);
    }
    assert!(
        manifest["viewport"]["tapered"]["scene"]["solids"]
            .as_array()
            .is_some_and(|solids| {
                solids
                    .iter()
                    .any(|solid| solid["feature_id"] == "tapered-reinforcement")
            })
    );
    assert!(
        manifest["viewport"]["lofted"]["scene"]["solids"]
            .as_array()
            .is_some_and(|solids| {
                ["tapered-reinforcement", "lofted-gusset"]
                    .iter()
                    .all(|feature_id| {
                        solids
                            .iter()
                            .any(|solid| solid["feature_id"] == *feature_id)
                    })
            })
    );
    for kind in [
        "startup_screenshot",
        "tapered_committed_screenshot",
        "lofted_committed_screenshot",
        "orbit_screenshot",
        "cleanup_screenshot",
        "reinforcement_transcript",
        "tapered_reinforcement_brep",
        "lofted_gusset_brep",
    ] {
        assert!(manifest["artifacts"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["kind"] == kind
                    && item["bytes"].as_u64().unwrap_or(0) > 0
                    && item["sha256"].as_str().is_some_and(|hash| hash.len() == 64)
            })
        }));
    }

    let transcript = fs::read_to_string(evidence.join("reinforcement-transcript.jsonl"))
        .expect("reinforcement transcript exists");
    let stages = transcript
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("reinforcement transcript JSON"))
        .collect::<Vec<_>>();
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0]["command"], "draft");
    assert_eq!(stages[1]["command"], "loft");
    let draft_keys = stages[0]["typed_request"]
        .as_object()
        .expect("draft request is an object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        draft_keys,
        BTreeSet::from([
            "angle".to_string(),
            "base_feature_id".to_string(),
            "feature_id".to_string(),
            "pull_direction".to_string(),
        ])
    );
    let loft_keys = stages[1]["typed_request"]
        .as_object()
        .expect("loft request is an object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        loft_keys,
        BTreeSet::from([
            "feature_id".to_string(),
            "is_solid".to_string(),
            "profiles".to_string(),
            "ruled".to_string(),
        ])
    );
    assert_eq!(stages[0]["typed_request"]["angle"], 0.05235987755982989);
    assert_eq!(
        stages[0]["effective_request"]["bundle_path"],
        root.to_string_lossy().as_ref()
    );
    assert_eq!(
        stages[0]["effective_request"]["expected_revision"],
        before.revision_hash
    );
    assert_ne!(
        stages[0]["effective_request"]["expected_revision"],
        stages[1]["effective_request"]["expected_revision"]
    );
    assert_eq!(
        stages[1]["effective_request"]["expected_revision"],
        stages[0]["viewport_evidence"]["frame"]["revision"]
    );
    for stage in &stages {
        assert_eq!(
            stage["effective_request"]["expected_revision"],
            stage["source_revision"]
        );
        assert!(
            stage["acknowledgement"]["preview_text"]
                .as_str()
                .is_some_and(|text| text.contains("Preview:"))
        );
        assert!(
            stage["acknowledgement"]["commit_text"]
                .as_str()
                .is_some_and(|text| text.contains("Commit:"))
        );
        assert!(
            stage["acknowledgement"]["selection_text"]
                .as_str()
                .is_some_and(|text| text.contains("selected feature"))
        );
    }
    for stage in &stages {
        assert_eq!(stage["acknowledgement"]["preview_count"].as_u64(), Some(1));
        assert_eq!(stage["acknowledgement"]["commit_count"].as_u64(), Some(1));
        assert_eq!(
            stage["viewport_evidence"]["selected_feature_id"],
            stage["feature_id"]
        );
        assert!(
            !stage["viewport_evidence"]["frame"]["revision"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
    }

    let bundle_before_inspection = snapshot_tree(&root);
    let bundle = Bundle::at(&root)
        .open_read_only()
        .expect("reinforcement project opens read-only");
    assert_eq!(
        bundle.manifest.transaction_count,
        before.transaction_count + 2
    );
    assert!(
        bundle
            .log
            .entries()
            .iter()
            .any(|entry| entry.feature_id == "tapered-reinforcement")
    );
    assert!(
        bundle
            .log
            .entries()
            .iter()
            .any(|entry| entry.feature_id == "lofted-gusset")
    );
    let draft_entry = bundle
        .log
        .entries()
        .iter()
        .find(|entry| entry.feature_id == "tapered-reinforcement")
        .expect("graphical draft entry is retained");
    let CanonicalIntent::Draft(draft_intent) = draft_entry.intent.as_ref().expect("draft intent")
    else {
        panic!("graphical draft entry retained a non-draft intent");
    };
    assert_eq!(draft_intent.base_feature_id, "taper-seed");
    assert_eq!(draft_intent.angle, 0.05235987755982989);
    assert_eq!(draft_intent.pull_direction, [0.0, 0.0, 1.0]);
    assert_eq!(
        draft_intent.source_revision,
        stages[0]["source_revision"]
            .as_str()
            .expect("draft source revision")
    );
    assert_authenticated_brep(&root, draft_entry);
    let loft_entry = bundle
        .log
        .entries()
        .iter()
        .find(|entry| entry.feature_id == "lofted-gusset")
        .expect("graphical loft entry is retained");
    let CanonicalIntent::Loft(loft_intent) = loft_entry.intent.as_ref().expect("loft intent")
    else {
        panic!("graphical loft entry retained a non-loft intent");
    };
    assert_eq!(
        loft_intent.profiles,
        vec![
            vec![
                [8.0, 8.0, 8.0],
                [16.0, 8.0, 8.0],
                [16.0, 16.0, 8.0],
                [8.0, 16.0, 8.0]
            ],
            vec![
                [10.0, 10.0, 18.0],
                [14.0, 10.0, 18.0],
                [14.0, 14.0, 18.0],
                [10.0, 14.0, 18.0]
            ],
        ]
    );
    assert!(loft_intent.is_solid);
    assert!(!loft_intent.ruled);
    assert_eq!(
        loft_intent.source_revision,
        stages[1]["source_revision"]
            .as_str()
            .expect("loft source revision")
    );
    assert_authenticated_brep(&root, loft_entry);
    let verifier = Host::new();
    let scene = verifier
        .read_only_viewport_scene(&root)
        .expect("reinforcement scene reads read-only");
    assert_reinforcement_geometry(&scene);
    assert_eq!(snapshot_tree(&root), bundle_before_inspection);

    fs::remove_dir_all(workspace).expect("graphical reinforcement workspace removes");
}

#[test]
#[ignore = "requires the qualified graphical Ghostty toolchain"]
fn production_tui_bracket_foundation() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-graphical-bracket-foundation-{}-{suffix}",
        std::process::id()
    ));
    let evidence = std::env::temp_dir().join(format!(
        "threeterm-graphical-bracket-foundation-evidence-{}-{suffix}",
        std::process::id()
    ));
    Bundle::create(&root).expect("empty bracket project creates");

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_bracket_foundation")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("graphical bracket runner starts");
    assert!(
        output.status.success(),
        "graphical bracket runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("bracket manifest exists"),
    )
    .expect("bracket manifest is JSON");
    assert_eq!(
        manifest["schema_version"],
        "threeterm.graphical-tui.bracket-foundation/1"
    );
    assert_eq!(manifest["result"], "passed");
    assert_eq!(manifest["test"], "production_tui_bracket_foundation");
    assert_eq!(manifest["events"]["workflow"], "passed");
    assert_eq!(manifest["events"]["orbit"], "passed");
    assert_eq!(manifest["events"]["cleanup"], "passed");
    assert_eq!(
        manifest["bracket"]["transcript"],
        evidence
            .join("bracket-transcript.jsonl")
            .to_string_lossy()
            .as_ref()
    );
    assert!(manifest["artifacts"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["kind"] == "bracket_transcript")
            && items
                .iter()
                .filter(|item| item["kind"] == "bracket_step_screenshot")
                .count()
                == 11
            && items
                .iter()
                .any(|item| item["kind"] == "startup_screenshot")
            && items.iter().any(|item| item["kind"] == "orbit_screenshot")
    }));

    let expected_steps = [
        ("arm-x", "extrude"),
        ("arm-z", "extrude"),
        ("pad-a-seed", "extrude"),
        ("pad-a", "fillet"),
        ("pad-b-seed", "extrude"),
        ("pad-b", "chamfer"),
        ("bracket-l", "boolean-fuse"),
        ("bracket-lp1", "boolean-fuse"),
        ("bracket-base", "boolean-fuse"),
        ("bracket-hole-1", "hole"),
        ("bracket-foundation", "hole"),
    ];
    let transcript = fs::read_to_string(evidence.join("bracket-transcript.jsonl"))
        .expect("bracket transcript exists");
    let observed_steps = transcript
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("bracket transcript line is JSON"))
        .map(|entry| {
            (
                entry["feature_id"]
                    .as_str()
                    .expect("bracket transcript feature ID")
                    .to_owned(),
                entry["command"]
                    .as_str()
                    .expect("bracket transcript command")
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed_steps,
        expected_steps
            .iter()
            .map(|(feature_id, command)| ((*feature_id).to_owned(), (*command).to_owned()))
            .collect::<Vec<_>>()
    );
    assert!(transcript.lines().all(|line| {
        let entry: Value = serde_json::from_str(line).expect("transcript remains JSON");
        entry["screenshot"]["path"]
            .as_str()
            .is_some_and(|path| Path::new(path).is_file())
            && entry["acknowledgement"]["preview"]
                .as_str()
                .is_some_and(|marker| marker.contains("Preview:"))
            && entry["acknowledgement"]["commit"]
                .as_str()
                .is_some_and(|marker| marker.contains("Commit:"))
            && entry["commit_revision"] == entry["revision"]
            && entry["commit_revision"] == entry["viewport_evidence"]["frame"]["revision"]
            && entry["keyboard_input"]["pty_log"]
                == evidence.join("pty-input.log").to_string_lossy().as_ref()
            && entry["keyboard_input"]["start_offset"]
                .as_u64()
                .is_some_and(|offset| offset > 0)
            && entry["keyboard_input"]["end_offset"]
                .as_u64()
                .is_some_and(|offset| offset > 0)
            && entry["keyboard_input"]["end_offset"]
                .as_u64()
                .zip(entry["keyboard_input"]["start_offset"].as_u64())
                .is_some_and(|(end, start)| end > start)
            && entry["keyboard_input"]["log_sha256"]
                .as_str()
                .is_some_and(|digest| digest.len() == 64)
            && entry["viewport_evidence"]["frame"]["image_id"]
                .as_u64()
                .is_some_and(|image_id| image_id > 0)
            && entry["viewport_evidence"]["scene"]["triangle_count"]
                .as_u64()
                .is_some_and(|triangles| triangles > 0)
    }));
    let input_ranges = transcript
        .lines()
        .map(|line| {
            let entry: Value = serde_json::from_str(line).expect("transcript line is JSON");
            (
                entry["keyboard_input"]["start_offset"]
                    .as_u64()
                    .expect("keyboard input start offset"),
                entry["keyboard_input"]["end_offset"]
                    .as_u64()
                    .expect("keyboard input end offset"),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        input_ranges
            .windows(2)
            .all(|ranges| ranges[0].1 <= ranges[1].0)
    );

    let bundle = Bundle::at(&root)
        .open()
        .expect("retained bracket project opens");
    assert_eq!(bundle.log.len(), 11);
    assert_eq!(
        bundle
            .log
            .entries()
            .iter()
            .map(|entry| entry.feature_id.as_str())
            .collect::<Vec<_>>(),
        expected_steps
            .iter()
            .map(|(feature_id, _)| *feature_id)
            .collect::<Vec<_>>()
    );

    let revision = bundle.revision_hash_hex().to_string();
    let baseline_entries = bundle.log.entries().to_vec();
    let before_read_only = snapshot_tree(&root);
    let before_scene = Host::new()
        .read_only_viewport_scene(&root)
        .expect("retained bracket scene loads before reopen");
    assert_eq!(snapshot_tree(&root), before_read_only);
    drop(bundle);
    let reopened = Bundle::at(&root)
        .open_read_only()
        .expect("retained bracket project reopens read-only");
    assert_eq!(reopened.revision_hash_hex(), revision);
    assert_eq!(reopened.log.entries(), baseline_entries);
    let reopened_scene = Host::new()
        .read_only_viewport_scene(&root)
        .expect("retained bracket scene loads after reopen");
    assert_eq!(reopened_scene, before_scene);
    assert_eq!(snapshot_tree(&root), before_read_only);

    let worker = OcctWorker::locate().expect("graphical bracket worker exists");
    let pad_a = worker
        .inspect_edges(
            "graphical-bracket-pad-a-measurement",
            root.join("brep/pad-a.brep"),
            "pad-a",
            &revision,
            serde_json::json!({"provenance":{"source_feature_id":"pad-a","source_revision_id":revision,"source_edge_id":"measurement-anchor"}}),
        )
        .expect("graphical fillet landmarks inspect");
    assert!(pad_a.edge_candidates.iter().any(|candidate| {
        candidate.role == "fillet-transition"
            && (candidate.length - std::f64::consts::FRAC_PI_4).abs() < 1e-3
    }));
    let pad_b = worker
        .inspect_edges(
            "graphical-bracket-pad-b-measurement",
            root.join("brep/pad-b.brep"),
            "pad-b",
            &revision,
            serde_json::json!({"provenance":{"source_feature_id":"pad-b","source_revision_id":revision,"source_edge_id":"measurement-anchor"}}),
        )
        .expect("graphical chamfer landmarks inspect");
    let pad_b_seed = worker
        .inspect_edges(
            "graphical-bracket-pad-b-seed-measurement",
            root.join("brep/pad-b-seed.brep"),
            "pad-b-seed",
            &revision,
            serde_json::json!({"provenance":{"source_feature_id":"pad-b-seed","source_revision_id":revision,"source_edge_id":"measurement-anchor"}}),
        )
        .expect("graphical chamfer seed landmarks inspect");
    let pad_b_outer_length: f64 = pad_b
        .edge_candidates
        .iter()
        .filter(|candidate| candidate.role == "outer-perimeter")
        .map(|candidate| candidate.length)
        .sum();
    let pad_b_seed_outer_length: f64 = pad_b_seed
        .edge_candidates
        .iter()
        .filter(|candidate| candidate.role == "outer-perimeter")
        .map(|candidate| candidate.length)
        .sum();
    assert!(pad_b_outer_length > 0.0);
    assert!(pad_b_seed_outer_length > 0.0);
    assert!((pad_b_outer_length - pad_b_seed_outer_length).abs() > 0.01);

    let final_edges = worker
        .inspect_edges(
            "graphical-bracket-final-measurement",
            root.join("brep/bracket-foundation.brep"),
            "bracket-foundation",
            &revision,
            serde_json::json!({"provenance":{"source_feature_id":"bracket-foundation","source_revision_id":revision,"source_edge_id":"measurement-anchor"}}),
        )
        .expect("graphical hole landmarks inspect");
    for midpoint in [
        [52.25, 10.0, 0.0],
        [52.25, 10.0, 8.0],
        [12.25, 50.0, 0.0],
        [12.25, 50.0, 8.0],
    ] {
        assert!(final_edges.edge_candidates.iter().any(|candidate| {
            candidate.role == "fillet-transition"
                && (candidate.length - 14.137166941154069).abs() < 1e-3
                && candidate
                    .midpoint
                    .into_iter()
                    .zip(midpoint)
                    .all(|(actual, expected)| (actual - expected).abs() < 1e-3)
        }));
    }

    assert_eq!(before_scene.solids.len(), 1);
    assert!(
        before_scene.solids.iter().any(|solid| {
            solid.feature_id == "bracket-foundation" && !solid.triangles.is_empty()
        })
    );

    fs::remove_dir_all(root).expect("graphical bracket project removes");
    fs::remove_dir_all(evidence).expect("graphical bracket evidence removes");
}

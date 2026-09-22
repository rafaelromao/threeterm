use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::{BracketRequest, OcctWorker};
use threeterm_persistence::{Bundle, CanonicalIntent};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::schema;
use threeterm_tui::TuiSession;
use threeterm_viewport::ViewportScene;

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

const REINFORCEMENT_RECIPE: &str =
    include_str!("../../host/tests/data/bracket_reinforcement_recipe.v1.json");

fn selected_edge_for_recipe(step: &Value, revision: &str) -> Value {
    let selection = &step["edge_selection"];
    let midpoint: [f64; 3] = serde_json::from_value(selection["midpoint"].clone())
        .expect("recipe edge midpoint is a 3-vector");
    let tangent: [f64; 3] = serde_json::from_value(selection["tangent"].clone())
        .expect("recipe edge tangent is a 3-vector");
    let length = selection["length"]
        .as_f64()
        .expect("recipe edge length is numeric");
    let semantic_input =
        serde_json::to_vec(&(midpoint, tangent, length)).expect("recipe edge evidence serializes");
    json!({
        "semantic_id": format!("edge-{}", sha256_hex(&semantic_input)),
        "provenance": {
            "source_feature_id": step["request"]["base_feature_id"],
            "source_revision_id": revision,
            "source_edge_id": selection["source_edge_id"]
        },
        "role": selection["role"],
        "evidence": {"midpoint": midpoint, "tangent": tangent, "length": length}
    })
}

fn prepare_reinforcement_foundation(host: &Host, root: &Path) {
    let recipe: Value =
        serde_json::from_str(REINFORCEMENT_RECIPE).expect("reinforcement recipe is valid JSON");
    host.execute_domain_command(
        schema::NEW_PROJECT_COMMAND_ID,
        json!({"destination": root.to_string_lossy()}),
    )
    .expect("foundation project creates");
    let mut revision = host
        .identity(root)
        .expect("foundation identity reads")
        .revision_hash;
    for step in recipe["steps"]
        .as_array()
        .expect("recipe steps are an array")
        .iter()
        .take(12)
    {
        let command_name = step["command"]
            .as_str()
            .expect("recipe command is a string");
        let command = schema::find_by_name(command_name)
            .unwrap_or_else(|| panic!("recipe command is registered: {command_name}"));
        let mut request = step["request"].clone();
        request["bundle_path"] = root.to_string_lossy().into_owned().into();
        if matches!(command_name, "extrude" | "fillet" | "chamfer" | "hole") {
            request["expected_revision"] = revision.clone().into();
        }
        if matches!(command_name, "fillet" | "chamfer") {
            request["selected_edge"] = selected_edge_for_recipe(step, &revision);
        }
        let response = host
            .execute_domain_command(command.id, request)
            .unwrap_or_else(|error| {
                panic!("foundation step {} succeeds: {error:?}", step["index"])
            });
        revision = response["revision_hash"]
            .as_str()
            .expect("foundation response has a revision")
            .to_string();
    }
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
fn production_tui_reinforcement() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let workspace = std::env::temp_dir().join(format!(
        "threeterm-graphical-reinforcement-{}-{suffix}",
        std::process::id()
    ));
    let root = workspace.join("project");
    let evidence = workspace.join("evidence");
    fs::create_dir_all(&workspace).expect("graphical reinforcement workspace creates");
    let _worker = OcctWorker::locate().unwrap_or_else(|error| {
        panic!("graphical reinforcement requires the OCCT worker: {error}")
    });
    prepare_reinforcement_foundation(&Host::new(), &root);

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_reinforcement")
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
        "threeterm.graphical-tui.reinforcement/1"
    );
    assert_eq!(manifest["result"], "passed");
    assert_eq!(manifest["test"], "production_tui_reinforcement");
    for event in ["probe", "readiness", "workflow", "orbit", "cleanup"] {
        assert_eq!(
            manifest["events"][event], "passed",
            "event {event} did not pass"
        );
    }
    assert_eq!(
        manifest["viewport"]["startup"]["scene"]["solids"][0]["feature_id"],
        "bracket-foundation"
    );
    assert_eq!(
        manifest["viewport"]["workflow"]["scene"]["solids"][0]["feature_id"],
        "reinforced-foundation"
    );
    assert_eq!(
        manifest["viewport"]["workflow"]["frame"]["revision"],
        manifest["viewport"]["orbit"]["frame"]["revision"]
    );
    assert_ne!(
        manifest["viewport"]["workflow"]["frame"]["image_id"],
        manifest["viewport"]["orbit"]["frame"]["image_id"]
    );
    for kind in [
        "collar_screenshot",
        "opening_screenshot",
        "reinforcement_screenshot",
        "reinforcement_transcript",
        "orbit_screenshot",
        "cleanup_screenshot",
    ] {
        assert!(
            manifest["artifacts"].as_array().is_some_and(|items| items
                .iter()
                .any(|item| item["kind"] == kind && item["sha256"].as_str().is_some())),
            "reinforcement evidence is missing artifact {kind}"
        );
    }
    let transcript = fs::read_to_string(evidence.join("reinforcement-transcript.jsonl"))
        .expect("reinforcement transcript exists");
    let stages = transcript
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("transcript line is JSON"))
        .collect::<Vec<_>>();
    assert_eq!(stages.len(), 7);
    for stage in &stages {
        assert_eq!(stage["response"]["revision"], stage["revision"]);
        assert_eq!(
            stage["acknowledgement"]["preview_marker"],
            format!(
                "[dashed-outline] Preview: {}",
                stage["command"]
                    .as_str()
                    .expect("stage command is a string")
            )
        );
        assert!(
            stage["acknowledgement"]["commit_marker"]
                .as_str()
                .is_some_and(|marker| marker.starts_with("[selection-glyph] Commit: "))
        );
        assert_eq!(
            stage["effective_request"]["bundle_path"],
            root.to_string_lossy().as_ref()
        );
        assert!(
            stage["effective_request"]["expected_revision"]
                .as_str()
                .is_some_and(|revision| !revision.is_empty())
        );
        let expected_viewport_feature = if stage["feature_id"] == "reinforcement-snapshot" {
            "reinforced-foundation"
        } else {
            stage["feature_id"]
                .as_str()
                .expect("stage feature ID is a string")
        };
        assert_eq!(
            stage["viewport_evidence"]["scene"]["solids"][0]["feature_id"],
            expected_viewport_feature
        );
        if stage["screenshot"].is_object() {
            assert_eq!(
                stage["screenshot"]["frame_image_id"],
                stage["viewport_evidence"]["frame"]["image_id"]
            );
        }
    }
    assert_eq!(
        stages[0]["effective_request"]["expected_revision"],
        manifest["viewport"]["startup"]["frame"]["revision"]
    );
    assert!(stages.windows(2).all(|window| {
        window[0]["revision"] != window[1]["revision"]
            && window[1]["effective_request"]["expected_revision"]
                == window[0]["response"]["revision"]
    }));
    assert_eq!(
        stages
            .iter()
            .map(|stage| stage["feature_id"].as_str())
            .collect::<Vec<_>>(),
        [
            Some("revolved-collar"),
            Some("hollow-detail-seed"),
            Some("hollow-detail"),
            Some("hollow-detail-open"),
            Some("foundation-with-collar"),
            Some("reinforced-foundation"),
            Some("reinforcement-snapshot"),
        ]
    );

    let pty_output =
        fs::read_to_string(evidence.join("pty-output.log")).expect("PTY output exists");
    for marker in [
        "Commit: revolve",
        "Commit: extrude",
        "Commit: shell",
        "Commit: hole",
        "Commit: boolean-fuse",
        "Commit: save",
    ] {
        assert!(pty_output.contains(marker), "PTY output omits {marker}");
    }
    let bundle = Bundle::at(&root)
        .open_read_only()
        .expect("reinforcement project opens read-only");
    assert_eq!(bundle.log.len(), 19);
    assert!(root.join("brep/reinforced-foundation.brep").is_file());
    assert_eq!(
        manifest["cleanup_evidence"]["final_image_id"],
        manifest["cleanup_evidence"]["final_delete_image_id"]
    );
    fs::remove_dir_all(workspace).expect("graphical reinforcement workspace removes");
}

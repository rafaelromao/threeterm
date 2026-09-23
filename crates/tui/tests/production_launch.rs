use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::{Host, stl_integrity};
use threeterm_occt_worker::{
    BracketRequest, ExtrudeRequest, OcctWorker, WorkerError, new_request_id,
};
use threeterm_persistence::{Bundle, CanonicalIntent, LogEntry};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::schema;
use threeterm_protocol::schema::{BRACKET_COMMAND_ID, EXTRUDE_COMMAND_ID, LOAD_COMMAND_ID};
use threeterm_tui::{
    InteractiveTerminal, LaunchError, TerminalInput, TuiSession, decode_terminal_input, launch,
    launch_command,
};
use threeterm_viewport::{
    CapabilityProbeIo, CleanupSignal, SceneSolid, TerminalEnvironment, ViewportScene, parse_ack,
};

#[derive(Debug, Default)]
struct ScriptedTerminal {
    writes: Vec<u8>,
    probe_response: Option<Vec<u8>>,
    events: Vec<Vec<u8>>,
    queued_events: Vec<Vec<u8>>,
    read_events: Vec<Vec<u8>>,
    events_read: usize,
    replayed_probe_input: Vec<u8>,
    prepare_fails: bool,
    restore_fails: bool,
    ambiguous_probe: bool,
    cleanup_signal: Option<CleanupSignal>,
    eof_after_events: bool,
    read_fails_after_events: bool,
    panic_after_events: bool,
    skip_probe_replay: bool,
    fail_writes_on_read: Option<usize>,
    write_failures_remaining: usize,
    prepare_calls: usize,
    restore_calls: usize,
    selection_targets: BTreeSet<String>,
    selection_active: bool,
    selection_down_queued: bool,
    selection_waiting_for_frame: bool,
    selection_output_cursor: usize,
    selection_acknowledged_targets: BTreeSet<String>,
    selection_ack_pending: bool,
    selection_quit_queued: bool,
}

const SECTION_TOLERANCE_MM: f64 = 0.05;

fn scene_solid<'a>(scene: &'a ViewportScene, feature_id: &str) -> &'a SceneSolid {
    scene
        .solids
        .iter()
        .find(|solid| solid.feature_id == feature_id)
        .unwrap_or_else(|| panic!("scene has no solid for {feature_id}"))
}

fn solid_volume(solid: &SceneSolid) -> f64 {
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

fn section_bounds(solid: &SceneSolid, z: f64) -> Option<[f64; 4]> {
    let mut points = Vec::new();
    for triangle in &solid.triangles {
        let vertices = triangle.vertices;
        for vertex in vertices {
            if (vertex[2] - z).abs() <= SECTION_TOLERANCE_MM {
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

fn assert_closed_positive_mesh(solid: &SceneSolid) {
    assert!(!solid.triangles.is_empty());
    assert!(solid_volume(solid) > SECTION_TOLERANCE_MM);
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

fn assert_authenticated_brep(root: &Path, entry: &LogEntry) {
    let relative_path = entry
        .brep_path
        .as_deref()
        .expect("committed feature records its BREP path");
    let path = root.join(relative_path);
    let bytes = fs::read(&path).expect("committed BREP reads");
    assert_eq!(entry.brep_byte_count, Some(bytes.len() as u64));
    let digest = threeterm_occt_worker::sha256_file(&path).expect("committed BREP hashes");
    assert_eq!(entry.brep_sha256.as_deref(), Some(digest.as_str()));
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
        "evidence": {
            "midpoint": midpoint,
            "tangent": tangent,
            "length": length
        }
    })
}

fn prepare_reinforcement_foundation(host: &Host, root: &Path) -> String {
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
    revision
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

fn solid_bounds(scene: &ViewportScene, feature_id: &str) -> ([f64; 3], [f64; 3]) {
    let solid = scene
        .solids
        .iter()
        .find(|solid| solid.feature_id == feature_id)
        .unwrap_or_else(|| panic!("viewport scene has no solid {feature_id}"));
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for triangle in &solid.triangles {
        for vertex in triangle.vertices {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex[axis]);
                maximum[axis] = maximum[axis].max(vertex[axis]);
            }
        }
    }
    (minimum, maximum)
}

fn solid_has_triangle_centroid_in_xy_window(
    scene: &ViewportScene,
    feature_id: &str,
    x_range: (f64, f64),
    y_range: (f64, f64),
) -> bool {
    let solid = scene
        .solids
        .iter()
        .find(|solid| solid.feature_id == feature_id)
        .unwrap_or_else(|| panic!("viewport scene has no solid {feature_id}"));
    solid.triangles.iter().any(|triangle| {
        let centroid = [
            triangle
                .vertices
                .iter()
                .map(|vertex| vertex[0])
                .sum::<f64>()
                / 3.0,
            triangle
                .vertices
                .iter()
                .map(|vertex| vertex[1])
                .sum::<f64>()
                / 3.0,
        ];
        centroid[0] > x_range.0
            && centroid[0] < x_range.1
            && centroid[1] > y_range.0
            && centroid[1] < y_range.1
    })
}

fn committed_revision(output: &str, command: &str) -> String {
    let marker = format!("[selection-glyph] Commit: {command} revision=");
    output
        .split(&marker)
        .last()
        .and_then(|suffix| suffix.split_whitespace().next())
        .unwrap_or_else(|| panic!("terminal output has no committed revision for {command}"))
        .to_string()
}

impl Write for ScriptedTerminal {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.write_failures_remaining > 0 {
            self.write_failures_remaining -= 1;
            return Err(io::Error::other("scripted terminal write failure"));
        }
        self.writes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapabilityProbeIo for ScriptedTerminal {
    fn read_probe_response(&mut self, _max_bytes: usize) -> io::Result<Vec<u8>> {
        let mut response = self
            .probe_response
            .take()
            .unwrap_or_else(|| valid_probe_response(probe_nonce_from_writes(&self.writes)));
        if self.ambiguous_probe {
            let nonce = probe_nonce_from_writes(&self.writes);
            response.extend_from_slice(format!("\x1b_Gi={nonce};OK\x1b\\").as_bytes());
        }
        Ok(response)
    }
}

fn probe_nonce_from_writes(writes: &[u8]) -> u64 {
    let Some(start) = writes.windows(2).position(|window| window == b"i=") else {
        return 1;
    };
    let digits = writes[start + 2..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .copied()
        .collect::<Vec<_>>();
    std::str::from_utf8(&digits)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1)
}

fn latest_kitty_image_id(bytes: &[u8], offset: usize) -> Option<u64> {
    let suffix = &bytes[offset..];
    let mut found = None;
    for (index, window) in suffix.windows(2).enumerate() {
        if window != b"i=" {
            continue;
        }
        let digits = suffix[index + 2..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .copied()
            .collect::<Vec<_>>();
        if suffix.get(index + 2 + digits.len()..index + 6 + digits.len())
            == Some(b",o=z".as_slice())
        {
            found = std::str::from_utf8(&digits)
                .ok()
                .and_then(|value| value.parse().ok());
        }
    }
    found
}

fn viewport_revision_between(output: &str, start_marker: &str, end_marker: &str) -> String {
    let start = output
        .find(start_marker)
        .expect("viewport revision start marker exists");
    let end = output
        .find(end_marker)
        .expect("viewport revision end marker exists");
    assert!(start < end, "viewport revision markers are ordered");
    let marker = "[viewport-status] Viewport presented ";
    let mut cursor = start + start_marker.len();
    let mut revision = None;
    while let Some(relative) = output[cursor..end].find(marker) {
        let json_start = cursor + relative + marker.len();
        let mut stream = serde_json::Deserializer::from_str(output[json_start..end].trim_start())
            .into_iter::<Value>();
        if let Some(Ok(value)) = stream.next() {
            revision = value["frame"]["revision"].as_str().map(str::to_string);
        }
        cursor = json_start;
    }
    revision.expect("viewport revision exists between command acknowledgements")
}

impl InteractiveTerminal for ScriptedTerminal {
    fn replay_probe_input(&mut self, bytes: &[u8]) {
        self.replayed_probe_input.extend_from_slice(bytes);
        if !self.skip_probe_replay && !bytes.is_empty() {
            self.queued_events.push(bytes.to_vec());
        }
    }

    fn read_event(&mut self) -> io::Result<Vec<u8>> {
        self.queue_selection_event();
        self.events_read += 1;
        let event = self
            .queued_events
            .pop()
            .or_else(|| self.events.pop())
            .unwrap_or_default();
        if event == b"\x1b[B" && self.selection_down_queued {
            self.selection_down_queued = false;
            self.selection_waiting_for_frame = true;
        }
        if self.selection_ack_pending && event.starts_with(b"\x1b_G") {
            self.selection_ack_pending = false;
        }
        self.read_events.push(event.clone());
        if self.fail_writes_on_read == Some(self.events_read) {
            self.write_failures_remaining = 1;
        }
        if event.is_empty() {
            if self.panic_after_events {
                panic!("scripted terminal panic");
            }
            if self.read_fails_after_events {
                return Err(io::Error::other("scripted terminal read failure"));
            }
            if self.eof_after_events {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "scripted terminal input closed",
                ));
            }
        }
        Ok(event)
    }

    fn viewport_size(&self) -> (u32, u32) {
        (64, 48)
    }

    fn cleanup_signal(&self) -> Option<CleanupSignal> {
        self.cleanup_signal
    }

    fn prepare(&mut self) -> io::Result<()> {
        self.prepare_calls += 1;
        if self.prepare_fails {
            Err(io::Error::other("injected setup failure"))
        } else {
            Ok(())
        }
    }

    fn restore(&mut self) -> io::Result<()> {
        self.restore_calls += 1;
        if self.restore_fails {
            Err(io::Error::other("injected restore failure"))
        } else {
            Ok(())
        }
    }
}

impl ScriptedTerminal {
    fn queue_selection_event(&mut self) {
        if !self.selection_active || self.selection_quit_queued {
            return;
        }
        let output = String::from_utf8_lossy(&self.writes);
        if self.selection_down_queued {
            return;
        }
        if self.selection_waiting_for_frame {
            let Some(image_id) = latest_kitty_image_id(&self.writes, self.selection_output_cursor)
            else {
                return;
            };
            self.queued_events
                .push(format!("\x1b_Gi={image_id};OK\x1b\\").into_bytes());
            self.selection_waiting_for_frame = false;
            self.selection_ack_pending = true;
            return;
        }
        let selected_target = self
            .selection_targets
            .iter()
            .find(|target| output.contains(&format!("selected feature {target}")))
            .cloned();
        if let Some(target) = selected_target {
            self.selection_targets.remove(&target);
            self.selection_acknowledged_targets.insert(target);
        }
        if self.selection_targets.is_empty() {
            self.queued_events.push(b"q".to_vec());
            self.selection_quit_queued = true;
            self.selection_active = false;
            return;
        }
        if !output.contains("[selection-glyph] Commit: loft") {
            return;
        }
        self.selection_output_cursor = self.writes.len();
        self.queued_events.push(b"\x1b[B".to_vec());
        self.selection_down_queued = true;
    }
}

fn official_environment() -> TerminalEnvironment {
    TerminalEnvironment {
        term: Some("xterm-ghostty".to_string()),
        term_program: Some("ghostty".to_string()),
        in_tmux: false,
        over_ssh: false,
        foreground_tty: true,
        utf8: true,
        width: 80,
        height: 24,
    }
}

fn valid_probe_response(nonce: u64) -> Vec<u8> {
    format!(
        "x\x1b_Gi={nonce};OK\x1b\\\x1b_Gi={};OK\x1b\\\x1b[?u\x1b[97;1:1u\x1b[97;1:2u\x1b[<0;1;1M\x1b[<32;2;1M\x1b[<0;2;1m\x1b[<0;101;101M\x1b[<32;102;101M\x1b[<0;102;101m\x1b[I\x1b[8;24;80t",
        nonce + 1
    )
    .into_bytes()
}

fn optional_occt_worker(test_name: &str) -> Option<OcctWorker> {
    match OcctWorker::locate() {
        Ok(worker) => Some(worker),
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("{test_name} requires OCCT worker: {error:?}");
        }
        Err(error) if matches!(&error, WorkerError::Spawn { detail, .. } if detail.contains("not found")) =>
        {
            eprintln!("{test_name}: OCCT worker unavailable: {error:?}");
            None
        }
        Err(error) => panic!("{test_name}: OCCT worker failed to initialize: {error:?}"),
    }
}

fn selected_edge(
    base_feature_id: &str,
    revision: &str,
    source_edge_id: &str,
    midpoint: [f64; 3],
    length: f64,
) -> Value {
    let tangent = [1.0, 0.0, 0.0];
    let semantic_input =
        serde_json::to_vec(&(midpoint, tangent, length)).expect("edge evidence serializes");
    json!({
        "semantic_id": format!("edge-{}", sha256_hex(&semantic_input)),
        "provenance": {
            "source_feature_id": base_feature_id,
            "source_revision_id": revision,
            "source_edge_id": source_edge_id
        },
        "role": "outer-perimeter",
        "evidence": {
            "midpoint": midpoint,
            "tangent": tangent,
            "length": length
        }
    })
}

fn run_palette_command(host: &Host, root: &Path, command: &str, request: Value) -> Value {
    let request = serde_json::to_vec(&request).expect("TUI request serializes");
    let mut events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    events.extend(command.bytes().map(|byte| vec![byte]));
    events.push(b"\r".to_vec());
    events.extend(request.into_iter().map(|byte| vec![byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };
    launch(host, root, &mut terminal, official_environment())
        .unwrap_or_else(|error| panic!("TUI {command} workflow succeeds: {error:?}"))
        .last_response
        .unwrap_or_else(|| panic!("TUI {command} workflow produced no response"))
}

#[test]
fn sgr_pick_coordinates_are_zero_based_and_reject_zero() {
    assert_eq!(
        decode_terminal_input(b"\x1b[<0;33;25M"),
        Some(TerminalInput::Pick { x: 32, y: 24 })
    );
    assert_eq!(
        decode_terminal_input(b"\x1b[<0;64;48M"),
        Some(TerminalInput::Pick { x: 63, y: 47 })
    );
    assert_eq!(decode_terminal_input(b"\x1b[<0;0;1M"), None);
}

fn unattached_environment() -> TerminalEnvironment {
    TerminalEnvironment {
        term: Some("xterm-256color".to_string()),
        term_program: None,
        in_tmux: true,
        over_ssh: false,
        foreground_tty: false,
        utf8: false,
        width: 0,
        height: 0,
    }
}

#[test]
fn production_launch_refuses_unattached_terminal_before_event_loop() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical snapshot exists");
    let mut terminal = ScriptedTerminal::default();

    let error = launch(&host, &root, &mut terminal, unattached_environment())
        .expect_err("unattached terminal cannot start Interactive Modeling");
    assert!(matches!(error, LaunchError::Capability(_)));
    let envelope: Value = serde_json::from_str(&error.to_json()).expect("diagnostic is JSON");
    assert_eq!(envelope["schema_version"], "threeterm.tui.launch/1");
    assert_eq!(envelope["code"], "capability_denied");
    assert_eq!(envelope["route"], "headless_automation");
    assert_eq!(
        envelope["viewport_diagnostic"]["source_revision"],
        "capability-probe"
    );
    assert!(
        terminal.writes.is_empty(),
        "refusal emits no terminal wire bytes"
    );
    assert_eq!(
        terminal.events_read, 0,
        "refusal never enters the event loop"
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_enters_direct_ghostty_loop_after_initial_ack() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-positive-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        probe_response: None,
        events: vec![
            b"q".to_vec(),
            b"\x1b_Gi=3;OK\x1b\\".to_vec(),
            b"\x1b[<0;33;25M".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };

    let result = launch(&host, &root, &mut terminal, official_environment())
        .expect("positive probe starts the production TUI");
    assert!(result.event_loop_entered);
    assert!(
        terminal.replayed_probe_input.starts_with(b"x"),
        "the leading user input survives probe acknowledgement filtering"
    );
    assert!(
        terminal
            .writes
            .windows(b"a=T,t=d".len())
            .any(|window| { window == b"a=T,t=d" })
    );
    assert!(
        terminal
            .writes
            .windows(b"c=80,r=24".len())
            .any(|window| window == b"c=80,r=24"),
        "production frame uses the detected terminal cell placement"
    );
    let output = String::from_utf8_lossy(&terminal.writes);
    let evidence_line = output
        .lines()
        .find(|line| line.contains("[viewport-status] Viewport presented "))
        .expect("initial acknowledged frame emits viewport evidence");
    let evidence: Value = serde_json::from_str(
        evidence_line
            .split_once("presented ")
            .expect("viewport evidence marker has a JSON payload")
            .1,
    )
    .expect("viewport evidence is JSON");
    assert_eq!(evidence["schema_version"], "threeterm.viewport-evidence/1");
    assert_eq!(evidence["acknowledgement"], "viewport-presented");
    assert_eq!(evidence["frame"]["frame_token"], 1);
    assert_eq!(evidence["frame"]["image_id"], 1);
    assert_eq!(evidence["frame"]["width"], 64);
    assert_eq!(evidence["frame"]["height"], 48);
    assert_eq!(evidence["palette"]["name"], "catppuccin");
    assert_eq!(evidence["camera"]["yaw_degrees"], 0);
    assert_eq!(evidence["camera"]["pitch_degrees"], 20);
    assert!(
        output.contains("[ready-status] Interactive Modeling ready"),
        "readiness is visible only after the positive probe and initial frame"
    );
    assert!(
        output.contains("\"state\":\"valid\"")
            && output.contains("\"direct_ghostty\":true")
            && output.contains("\"kitty_acknowledgements\":true")
            && output.contains("\"kitty_keyboard\":true")
            && output.contains("\"sgr_mouse_cell\":true")
            && output.contains("\"sgr_mouse_pixel\":true")
            && output.contains("\"focus_reporting\":true")
            && output.contains("\"alternate_screen\":true")
            && output.contains("\"resize_events\":true"),
        "readiness retains structured capability evidence"
    );
    assert!(
        !terminal
            .writes
            .windows(b"xterm".len())
            .any(|window| window == b"xterm"),
        "production viewport does not emit text fallback"
    );
    assert_eq!(terminal.events_read, 6);
    assert!(
        String::from_utf8_lossy(&terminal.writes).contains("Pick: semantic candidate validated")
    );

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_binds_orbit_evidence_to_the_new_acknowledged_frame() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-evidence-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b_Gi=3;OK\x1b\\".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b[C".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("orbit completes the production event loop");

    let output = String::from_utf8_lossy(&terminal.writes);
    let evidence = output
        .lines()
        .filter(|line| line.contains("[viewport-status] Viewport presented "))
        .map(|line| {
            serde_json::from_str::<Value>(
                line.split_once("Viewport presented ")
                    .expect("viewport marker contains JSON")
                    .1,
            )
            .expect("viewport marker is JSON")
        })
        .collect::<Vec<_>>();
    assert!(
        evidence.len() >= 2,
        "initial and orbit evidence are emitted"
    );
    let startup = &evidence[0];
    let orbit = evidence.last().expect("orbit evidence exists");
    assert_eq!(startup["frame"]["image_id"], 1);
    assert_eq!(orbit["frame"]["image_id"], 3);
    assert_eq!(startup["frame"]["revision"], orbit["frame"]["revision"]);
    assert_eq!(startup["scene"]["solids"], orbit["scene"]["solids"]);
    assert_eq!(
        startup["scene"]["triangle_count"],
        orbit["scene"]["triangle_count"]
    );
    assert_eq!(orbit["camera"]["yaw_degrees"], 5);
    assert_eq!(orbit["camera"]["pitch_degrees"], 20);

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_routes_keyboard_navigation_to_acknowledged_viewport_frames() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-navigation-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let before_tree = snapshot_tree(&root);
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b_Gi=5;OK\x1b\\".to_vec(),
            b"+".to_vec(),
            b"\x1b_Gi=4;OK\x1b\\".to_vec(),
            b"w".to_vec(),
            b"\x1b_Gi=3;OK\x1b\\".to_vec(),
            b"\x1b[C".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b[B".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        skip_probe_replay: true,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("keyboard navigation completes the production event loop");

    let output = String::from_utf8_lossy(&terminal.writes);
    for marker in [
        "selected feature feature-a",
        "Orbit right",
        "Pan up",
        "Zoom in",
    ] {
        assert!(
            output.contains(marker),
            "production output is missing {marker}"
        );
    }
    let evidence = output
        .lines()
        .filter(|line| line.contains("[viewport-status] Viewport presented "))
        .map(|line| {
            serde_json::from_str::<Value>(
                line.split_once("Viewport presented ")
                    .expect("viewport marker contains JSON")
                    .1,
            )
            .expect("viewport marker is JSON")
        })
        .collect::<Vec<_>>();
    assert!(
        evidence.len() >= 5,
        "each navigation frame emits viewport evidence"
    );
    let selection = evidence
        .iter()
        .find(|evidence| evidence["selected_feature_id"] == "feature-a")
        .expect("selection evidence binds the selected feature");
    assert_eq!(selection["camera"]["pitch_degrees"], 25);
    let final_frame = evidence.last().expect("zoom evidence exists");
    assert_eq!(final_frame["camera"]["yaw_degrees"], 5);
    assert_eq!(final_frame["camera"]["pitch_degrees"], 25);
    assert_eq!(final_frame["camera"]["pan_y"], -5);
    assert_eq!(final_frame["camera"]["zoom_percent"], 105);
    assert_eq!(final_frame["selected_feature_id"], "feature-a");
    assert_eq!(host.current(), Some(before));
    assert_eq!(snapshot_tree(&root), before_tree);

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_executes_registered_noninteractive_command() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-command-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b_Gi=3;OK\x1b\\".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };

    let result = launch_command(
        &host,
        &root,
        &mut terminal,
        official_environment(),
        LOAD_COMMAND_ID,
        json!({"bundle_path": root.to_string_lossy()}),
    )
    .expect("registered command runs inside the production TUI");
    assert!(result.event_loop_entered);
    let response = result.last_response.expect("command response");
    assert_eq!(
        response["schema_version"],
        "threeterm.command.load.response/2"
    );
    assert!(response["feature_graph_hash"].is_string());
    assert_eq!(terminal.events_read, 5);

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_saves_closes_reopens_and_loads_through_keyboard_controls() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-lifecycle-{suffix}-{}",
        std::process::id()
    ));
    Host::new()
        .save(&root, "l-bracket", "box")
        .expect("finished L-bracket project persists");

    let append_text = |events: &mut Vec<Vec<u8>>, text: &[u8]| {
        events.extend(text.iter().map(|byte| vec![*byte]));
    };
    let append_lifecycle_command =
        |events: &mut Vec<Vec<u8>>, command: &str, request: &str, commit_id: u8| {
            events.push(b"\x10".to_vec());
            append_text(events, command.as_bytes());
            events.push(b"\r".to_vec());
            append_text(events, request.as_bytes());
            events.extend([
                b"\x16".to_vec(),
                b"\x1b[13;5u".to_vec(),
                format!("\x1b_Gi={commit_id};OK\x1b\\").into_bytes(),
            ]);
        };

    let mut first_events = vec![
        b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
    ];
    append_lifecycle_command(
        &mut first_events,
        "save",
        r#"{"feature_id":"lifecycle-save-marker","kind":"box"}"#,
        3,
    );
    first_events.push(b"q".to_vec());
    first_events.reverse();
    let mut first_terminal = ScriptedTerminal {
        events: first_events,
        ..Default::default()
    };
    let first_host = Host::new();
    let first = launch(
        &first_host,
        &root,
        &mut first_terminal,
        official_environment(),
    )
    .expect("keyboard save and clean close succeed");
    let saved = Bundle::at(&root).open().expect("saved project reopens");
    assert!(saved.graph.contains_feature("l-bracket"));
    assert!(saved.graph.contains_feature("lifecycle-save-marker"));
    assert!(
        first_terminal
            .writes
            .windows(b"Save completed".len())
            .any(|window| window == b"Save completed")
    );
    assert!(
        first_terminal
            .writes
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert!(first.event_loop_entered);

    let mut second_events = vec![
        b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
    ];
    append_lifecycle_command(&mut second_events, "load", r#"{}"#, 3);
    second_events.push(b"q".to_vec());
    second_events.reverse();
    let mut second_terminal = ScriptedTerminal {
        events: second_events,
        ..Default::default()
    };
    let second_host = Host::new();
    let second = launch(
        &second_host,
        &root,
        &mut second_terminal,
        official_environment(),
    )
    .expect("fresh host relaunch and keyboard load succeed");
    let loaded = second_host
        .identity(&root)
        .expect("reopened project identity reads");
    assert_eq!(loaded.feature_graph_hash, saved.feature_graph_hash_hex());
    assert_eq!(loaded.revision_hash, saved.revision_hash_hex());
    assert!(
        second_terminal
            .writes
            .windows(b"Load completed".len())
            .any(|window| window == b"Load completed")
    );
    assert!(second.event_loop_entered);

    fs::remove_dir_all(root).expect("lifecycle project removes");
}

#[test]
#[ignore = "requires the native OCCT worker"]
fn production_launch_saves_reopens_loads_validates_and_exports_through_keyboard_controls() {
    let worker = OcctWorker::locate().expect("keyboard lifecycle requires the OCCT worker");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-export-lifecycle-{suffix}-{}",
        std::process::id()
    ));
    let output = root.join("export");
    Host::new()
        .create_bracket(
            &root,
            BracketRequest::new("keyboard-lifecycle-bracket", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("l-bracket"),
            &worker,
        )
        .expect("finished L-bracket project persists");
    let l_bracket_brep = root.join("brep/l-bracket.brep");
    let initial_l_bracket_brep = fs::read(&l_bracket_brep).expect("initial L-bracket BREP reads");

    let append_text = |events: &mut Vec<Vec<u8>>, text: &[u8]| {
        events.extend(text.iter().map(|byte| vec![*byte]));
    };
    let append_lifecycle_command =
        |events: &mut Vec<Vec<u8>>, command: &str, request: &str, commit_id: u8| {
            events.push(b"\x10".to_vec());
            append_text(events, command.as_bytes());
            events.push(b"\r".to_vec());
            append_text(events, request.as_bytes());
            events.extend([
                b"\x16".to_vec(),
                b"\x1b[13;5u".to_vec(),
                format!("\x1b_Gi={commit_id};OK\x1b\\").into_bytes(),
            ]);
        };

    let mut first_events = vec![
        b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
    ];
    append_lifecycle_command(
        &mut first_events,
        "save",
        &format!(
            r#"{{"feature_id":"lifecycle-save-marker","kind":"box","bundle_path":"{}"}}"#,
            root.to_string_lossy()
        ),
        3,
    );
    first_events.push(b"q".to_vec());
    first_events.reverse();
    let mut first_terminal = ScriptedTerminal {
        events: first_events,
        ..Default::default()
    };
    launch(
        &Host::new(),
        &root,
        &mut first_terminal,
        official_environment(),
    )
    .expect("keyboard save and clean close succeed");

    let saved = Bundle::at(&root).open().expect("saved project reopens");
    assert!(saved.graph.contains_feature("l-bracket"));
    assert!(saved.graph.contains_feature("lifecycle-save-marker"));
    assert_eq!(
        fs::read(&l_bracket_brep).expect("saved L-bracket BREP reads"),
        initial_l_bracket_brep,
        "keyboard save preserves the finished L-bracket BREP"
    );
    assert!(String::from_utf8_lossy(&first_terminal.writes).contains("Save completed"));

    let mut second_events = vec![
        b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
    ];
    append_lifecycle_command(&mut second_events, "load", r#"{}"#, 3);
    append_lifecycle_command(
        &mut second_events,
        "validate",
        r#"{"feature_id":"l-bracket"}"#,
        4,
    );
    append_lifecycle_command(
        &mut second_events,
        "export",
        &format!(
            r#"{{"feature_id":"l-bracket","formats":["stl"],"output_dir":"{}","tessellation_deflection":0.1,"override_warnings":false,"accept_stale_geometry":false}}"#,
            output.to_string_lossy()
        ),
        5,
    );
    second_events.push(b"q".to_vec());
    second_events.reverse();
    let mut second_terminal = ScriptedTerminal {
        events: second_events,
        ..Default::default()
    };
    let second_host = Host::new();
    let second = launch(
        &second_host,
        &root,
        &mut second_terminal,
        official_environment(),
    )
    .expect("fresh host relaunch and keyboard lifecycle succeed");

    let loaded = second_host
        .identity(&root)
        .expect("reopened identity reads");
    assert_eq!(loaded.feature_graph_hash, saved.feature_graph_hash_hex());
    assert_eq!(loaded.revision_hash, saved.revision_hash_hex());
    assert_eq!(
        fs::read(&l_bracket_brep).expect("reloaded L-bracket BREP reads"),
        initial_l_bracket_brep,
        "keyboard relaunch and load preserve the finished L-bracket BREP"
    );
    let output_text = String::from_utf8_lossy(&second_terminal.writes);
    assert!(output_text.contains("Load completed"));
    assert!(output_text.contains("[validation-status] Validation passed"));
    assert!(output_text.contains("[export-status] Export completed"));
    assert_eq!(second.last_response.as_ref().unwrap()["status"], "ok");

    let stl_path = output.join("l-bracket.stl");
    assert!(stl_path.is_file(), "TUI export creates the requested STL");
    let report = stl_integrity::verify_path(&stl_path).expect("TUI STL passes independent check");
    assert_eq!(report.format, stl_integrity::StlFormat::Ascii);
    assert!(report.triangle_count > 0);
    assert_eq!(report.shell_count, 1);
    assert!(report.material_volume > 0.0);

    fs::remove_dir_all(root).expect("export lifecycle project removes");
}

#[test]
fn interactive_capability_gate() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-interactive-capability-gate-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };

    let result = launch(&host, &root, &mut terminal, official_environment())
        .expect("positive attachment-scoped probe admits startup");
    assert!(result.event_loop_entered);
    assert!(terminal.events_read >= 2);
    assert!(
        terminal
            .writes
            .windows(b"a=T,t=d".len())
            .any(|window| window == b"a=T,t=d")
    );
    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_acknowledges_focus_recovery_and_resize() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-transient-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b_Gi=3;OK\x1b\\".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b[8;30;100t".to_vec(),
            b"\x1b[I".to_vec(),
            b"\x1b[O".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("transient production events complete the event loop");

    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("[focus-recovery-banner]"));
    assert!(output.contains("[ready-status]"));
    assert!(output.contains("[resize-recovery-glyph]"));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_acknowledges_pointer_gesture() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-pointer-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"q".to_vec(),
            b"\x1b[<0;34;26m".to_vec(),
            b"\x1b[<32;34;26M".to_vec(),
            b"\x1b_Gi=3;OK\x1b\\".to_vec(),
            b"\x1b[<0;33;25M".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("pointer gesture completes the production event loop");

    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("[selection-glyph] Pick: semantic candidate validated"));
    assert!(output.contains("[motion-trail] drag active"));
    assert!(output.contains("[ready-status] drag finished"));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_terminal_reset() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-reset-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        events: vec![
            b"\x1bc".to_vec(),
            b"\x1b_Gi=2;OK\x1b\\".to_vec(),
            b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        ],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("terminal reset completes cleanup");

    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("[error-glyph] restoring after runtime failure"));
    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_distinguishes_sigint_cleanup() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-sigint-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        cleanup_signal: Some(CleanupSignal::Sigint),
        events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("SIGINT completes cleanup without keyboard emulation");

    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1004l".len())
            .any(|window| window == b"?1004l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_distinguishes_sigterm_cleanup() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-sigterm-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        cleanup_signal: Some(CleanupSignal::Sigterm),
        events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("SIGTERM completes cleanup without keyboard emulation");

    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_terminal_eof() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-eof-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        eof_after_events: true,
        events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    let error = launch(&host, &root, &mut terminal, official_environment())
        .expect_err("EOF is reported after the production loop cleans up");
    assert!(matches!(error, LaunchError::Runtime(_)));
    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert!(
        terminal
            .writes
            .windows(b"?1049l".len())
            .any(|window| window == b"?1049l")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_terminal_write_failure() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-write-failure-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let before = host.current().expect("canonical state exists");
    let mut terminal = ScriptedTerminal {
        fail_writes_on_read: Some(2),
        events: vec![b"q".to_vec(), b"\x1b_Gi=1;OK\x1b\\".to_vec()],
        ..Default::default()
    };
    let error = launch(&host, &root, &mut terminal, official_environment())
        .expect_err("terminal write failure is reported after cleanup");
    assert!(matches!(error, LaunchError::Viewport(_)));
    assert!(
        terminal
            .writes
            .windows(b"a=d,d=I".len())
            .any(|window| window == b"a=d,d=I")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_cleans_up_after_panic_and_read_failure() {
    for (suffix, panic_after_events, read_fails_after_events) in
        [("panic", true, false), ("read-failure", false, true)]
    {
        let root = std::env::temp_dir().join(format!(
            "threeterm-production-launch-{suffix}-{}",
            std::process::id()
        ));
        let host = Host::new();
        host.save(&root, "feature-a", "box")
            .expect("project is persisted");
        let before = host.current().expect("canonical state exists");
        let mut terminal = ScriptedTerminal {
            events: vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()],
            panic_after_events,
            read_fails_after_events,
            ..Default::default()
        };
        let error = launch(&host, &root, &mut terminal, official_environment())
            .expect_err("abnormal terminal path is reported");
        assert!(matches!(error, LaunchError::Runtime(_)));
        assert!(
            terminal
                .writes
                .windows(b"a=d,d=I".len())
                .any(|window| window == b"a=d,d=I")
        );
        assert_eq!(host.current(), Some(before));
        std::fs::remove_dir_all(root).expect("project is removed");
    }
}

#[test]
fn production_binary_refuses_without_terminal_capability_evidence() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("/tmp/threeterm-production-launch-missing")
        .stdin(std::process::Stdio::null())
        .env_remove("TERM")
        .env_remove("TERM_PROGRAM")
        .env_remove("TMUX")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .expect("production TUI binary runs");
    assert_eq!(output.status.code(), Some(10));
    assert!(output.stdout.is_empty(), "refusal has no stdout fallback");
    let diagnostic: Value =
        serde_json::from_slice(&output.stderr).expect("binary refusal is one JSON object");
    assert_eq!(diagnostic["code"], "capability_denied");
    assert_eq!(diagnostic["route"], "headless_automation");
}

#[test]
fn production_launch_rejects_ambiguous_probe_acknowledgement() {
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-ambiguous-{}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let mut terminal = ScriptedTerminal {
        ambiguous_probe: true,
        ..Default::default()
    };

    let error = launch(&host, &root, &mut terminal, official_environment())
        .expect_err("duplicate probe acknowledgement is ambiguous");
    assert!(matches!(error, LaunchError::Capability(_)));
    let diagnostic: Value = serde_json::from_str(&error.to_json()).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "capability_malformed");
    assert_eq!(terminal.events_read, 0);

    std::fs::remove_dir_all(root).expect("project is removed");
}

#[test]
fn production_launch_retains_restore_failure_with_original_diagnostic() {
    let mut terminal = ScriptedTerminal {
        restore_fails: true,
        ..Default::default()
    };

    let error = launch(
        &Host::new(),
        "/tmp/threeterm-production-launch-restore-failure",
        &mut terminal,
        unattached_environment(),
    )
    .expect_err("capability refusal with failed restore is still an error");
    assert!(matches!(error, LaunchError::Cleanup { .. }));
    let diagnostic: Value = serde_json::from_str(&error.to_json()).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "capability_denied");
    assert_eq!(
        diagnostic["cleanup_error"],
        "terminal restore failed: injected restore failure"
    );
}

#[test]
fn production_launch_restores_after_terminal_setup_failure() {
    let mut terminal = ScriptedTerminal {
        prepare_fails: true,
        ..Default::default()
    };

    let error = launch(
        &Host::new(),
        "/tmp/threeterm-production-launch-setup-failure",
        &mut terminal,
        official_environment(),
    )
    .expect_err("terminal setup failure refuses launch");
    assert!(matches!(error, LaunchError::Runtime(_)));
    assert_eq!(terminal.events_read, 0);
}

#[test]
fn shared_extrude_execution_accepts_deterministic_tui_input() {
    if OcctWorker::locate().is_err() {
        eprintln!("interactive command slice: OCCT worker unavailable");
        return;
    }
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-command-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("project is persisted");
    let request =
        br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}"#;
    let mut script = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    script.extend(b"extrude".iter().map(|byte| vec![*byte]));
    script.push(b"\r".to_vec());
    script.extend(request.iter().map(|byte| vec![*byte]));
    script.push(b"\x16".to_vec());
    script.push(b"\x1b_Gi=2;OK\x1b\\".to_vec());
    script.push(b"\x1b[13;5u".to_vec());
    script.push(b"\x1b_Gi=3;OK\x1b\\".to_vec());
    script.push(b"q".to_vec());
    script.reverse();
    let mut terminal = ScriptedTerminal {
        events: script,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("production palette flow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, 2);
    assert!(root.join("brep/keyboard-extrude.brep").is_file());
    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("Command Palette"));
    assert!(output.contains("[outline]"));
    assert!(output.contains("[dashed-outline]"));
    assert!(output.contains("[selection-glyph]"));

    let headless_root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-headless-{}-{suffix}",
        std::process::id()
    ));
    let headless_host = Host::new();
    headless_host
        .save(&headless_root, "feature-a", "box")
        .expect("headless project is persisted");
    let headless_revision = headless_host
        .identity(&headless_root)
        .expect("headless identity reads")
        .revision_hash;
    headless_host
        .execute_domain_command(
            EXTRUDE_COMMAND_ID,
            json!({
                "bundle_path": headless_root.to_string_lossy(),
                "expected_revision": headless_revision,
                "feature_id": "keyboard-extrude",
                "profile": [[0, 0], [10, 0], [10, 5], [0, 5]],
                "height": 3,
                "mode": "additive",
            }),
        )
        .expect("headless extrude succeeds");
    assert_eq!(
        fs::read(root.join("brep/keyboard-extrude.brep")).expect("interactive BREP reads"),
        fs::read(headless_root.join("brep/keyboard-extrude.brep")).expect("headless BREP reads")
    );

    std::fs::remove_dir_all(root).expect("project is removed");
    std::fs::remove_dir_all(headless_root).expect("headless project is removed");
}

#[test]
fn production_launch_completes_keyboard_first_modeling_workflow_end_to_end() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("keyboard-first modeling requires OCCT worker: {error}")
        }
        Err(error) => {
            eprintln!("keyboard-first modeling: OCCT worker unavailable: {error}");
            return;
        }
    };
    drop(worker);

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-keyboard-first-modeling-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("keyboard-first seed project persists");
    let before = host.identity(&root).expect("seed identity reads");
    let manifest_before = fs::read(root.join("manifest.json")).expect("seed manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("seed log reads");
    let request = br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}"#;

    let mut events = vec![
        b"\x1b_Gi=1;OK\x1b\\".to_vec(),
        b"\x1b[B".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"\x1b_Gi=3;OK\x1b\\".to_vec(),
        b"\x10".to_vec(),
    ];
    events.extend(b"extrude".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(request.iter().map(|byte| vec![*byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b_Gi=4;OK\x1b\\".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=5;OK\x1b\\".to_vec(),
        b"\x10".to_vec(),
    ]);
    events.extend(b"extrude".iter().map(|byte| vec![*byte]));
    events.extend([
        b"\r".to_vec(),
        b"\x1b".to_vec(),
        b"\x1b[C".to_vec(),
        b"\x1b_Gi=6;OK\x1b\\".to_vec(),
        b"w".to_vec(),
        b"\x1b_Gi=7;OK\x1b\\".to_vec(),
        b"+".to_vec(),
        b"\x1b_Gi=8;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("keyboard-only production workflow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(
        identity.transaction_count,
        before.transaction_count + 1,
        "identity={identity:?}, before={before:?}, writes={}",
        String::from_utf8_lossy(&terminal.writes)
    );
    assert_ne!(identity.revision_hash, before.revision_hash);
    assert!(root.join("brep/keyboard-extrude.brep").is_file());

    let output = String::from_utf8_lossy(&terminal.writes);
    for acknowledgement in [
        "[outline] Command Palette",
        "[selection-glyph]",
        "feature-a",
        "[outline] Draft: extrude",
        "[dashed-outline] Preview: extrude",
        "[selection-glyph] Commit: extrude",
        "[cancellation-glyph] Cancellation: command draft discarded",
        "[motion-trail] Orbit",
        "[motion-trail] Pan up",
        "[motion-trail] Zoom in",
    ] {
        assert!(
            output.contains(acknowledgement),
            "missing acknowledgement: {acknowledgement}"
        );
    }
    assert!(
        output.contains("a=d,d=I,i=8"),
        "normal close deletes the latest active Kitty image"
    );
    assert!(
        output.contains("?1049l"),
        "normal close exits alternate screen"
    );
    assert!(
        output.contains("?1016l"),
        "normal close disables pixel mouse reporting"
    );
    assert!(
        output.contains("\x1b[0m"),
        "normal close resets terminal attributes"
    );
    assert_eq!(terminal.prepare_calls, 1);
    assert_eq!(terminal.restore_calls, 1);

    let acknowledgement_ids = terminal
        .read_events
        .iter()
        .filter_map(|event| parse_ack(event).ok())
        .collect::<Vec<_>>();
    assert_eq!(acknowledgement_ids, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(
        terminal.read_events.iter().all(|event| !matches!(
            decode_terminal_input(event),
            Some(
                TerminalInput::Pick { .. }
                    | TerminalInput::PointerPressed { .. }
                    | TerminalInput::PointerMoved { .. }
                    | TerminalInput::PointerReleased { .. }
            )
        )),
        "keyboard-first workflow does not depend on mouse or pick events"
    );

    let committed_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let committed_log = fs::read(root.join("transactions.log")).expect("log reads");
    assert_ne!(committed_manifest, manifest_before);
    assert_ne!(committed_log, log_before);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn interactive_production_event_loop() {
    match OcctWorker::locate() {
        Ok(_) => {}
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("interactive production event loop requires OCCT: {error}");
        }
        Err(error) => {
            eprintln!("interactive production event loop: OCCT worker unavailable: {error}");
            return;
        }
    }

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-interactive-bracket-event-loop-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("keyboard-first seed project persists");
    let before = host.identity(&root).expect("seed identity reads");
    let request = br#"{"bracket_id":"l-bracket","length":60,"width":30,"height":40,"thickness":3}"#;
    let mut events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    events.extend(b"bracket".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(request.iter().map(|byte| vec![*byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=3;OK\x1b\\".to_vec(),
        b"\x1b[B".to_vec(),
        b"\x1b_Gi=4;OK\x1b\\".to_vec(),
        b"w".to_vec(),
        b"\x1b_Gi=5;OK\x1b\\".to_vec(),
        b"+".to_vec(),
        b"\x1b_Gi=6;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("keyboard-first bracket workflow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, before.transaction_count + 1);
    assert_ne!(identity.revision_hash, before.revision_hash);
    assert!(root.join("brep/l-bracket.brep").is_file());

    let output = String::from_utf8_lossy(&terminal.writes);
    for acknowledgement in [
        "[outline] Command Palette",
        "[outline] Draft: bracket",
        "[dashed-outline] Preview: bracket",
        "[selection-glyph] Commit: bracket",
        "[selection-glyph]",
        "[motion-trail] Orbit up",
        "[motion-trail] Pan up",
        "[motion-trail] Zoom in",
    ] {
        assert!(
            output.contains(acknowledgement),
            "missing acknowledgement: {acknowledgement}"
        );
    }
    assert!(output.contains("a=d,d=I,i=5"));
    assert!(output.contains("?1049l"));
    assert_eq!(terminal.prepare_calls, 1);
    assert_eq!(terminal.restore_calls, 1);
    assert_eq!(
        terminal
            .read_events
            .iter()
            .filter_map(|event| parse_ack(event).ok())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );

    let headless_root = std::env::temp_dir().join(format!(
        "threeterm-interactive-bracket-headless-{}-{suffix}",
        std::process::id()
    ));
    let headless_host = Host::new();
    headless_host
        .save(&headless_root, "seed", "box")
        .expect("headless seed project persists");
    let headless_before = headless_host
        .identity(&headless_root)
        .expect("headless identity reads");
    let headless = headless_host
        .execute_domain_command(
            BRACKET_COMMAND_ID,
            json!({
                "bundle_path": headless_root.to_string_lossy(),
                "bracket_id": "l-bracket",
                "length": 60.0,
                "width": 30.0,
                "height": 40.0,
                "thickness": 3.0,
                "expected_revision": headless_before.revision_hash,
            }),
        )
        .expect("headless bracket commit succeeds");
    let headless_identity = headless_host
        .identity(&headless_root)
        .expect("headless committed identity reads");
    assert_eq!(headless["revision_hash"], identity.revision_hash);
    assert_eq!(
        headless_identity.transaction_count,
        identity.transaction_count
    );
    assert_eq!(
        fs::read(root.join("brep/l-bracket.brep")).expect("interactive BREP reads"),
        fs::read(headless_root.join("brep/l-bracket.brep")).expect("headless BREP reads")
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(headless_root);
}

#[test]
fn production_launch_drives_one_hole_draft_through_preview_and_commit() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("interactive hole command requires OCCT worker: {error}")
        }
        Err(error) => {
            eprintln!("interactive command slice: OCCT worker unavailable: {error}");
            return;
        }
    };
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-hole-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("project is persisted");
    host.extrude(
        &root,
        ExtrudeRequest::new(
            "hole-launch-base",
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
            3.0,
        )
        .with_feature_id("base-1"),
        &worker,
    )
    .expect("base solid extrudes");

    let request = br#"{"feature_id":"interactive-hole","base_feature_id":"base-1","position":[1.5,1.5,0.0],"direction":[0.0,0.0,1.0],"diameter":1.0,"hole_kind":"drilled"}"#;
    let mut script = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    script.extend(b"hole".iter().map(|byte| vec![*byte]));
    script.push(b"\r".to_vec());
    script.extend(request.iter().map(|byte| vec![*byte]));
    script.push(b"\x16".to_vec());
    script.push(b"\x1b_Gi=2;OK\x1b\\".to_vec());
    script.push(b"\x1b[13;5u".to_vec());
    script.push(b"\x1b_Gi=3;OK\x1b\\".to_vec());
    script.push(b"q".to_vec());
    script.reverse();
    let mut terminal = ScriptedTerminal {
        events: script,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("production hole palette flow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, 3);
    assert!(root.join("brep/interactive-hole.brep").is_file());
    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("command preview ready"));
    assert!(output.contains("command committed"));
    assert!(output.contains("[selection-glyph]"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_launch_drives_mirror_through_keyboard_and_retains_both_pad_placements() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("interactive mirror command requires OCCT worker: {error}");
        }
        Err(error) => {
            eprintln!("interactive mirror command: OCCT worker unavailable: {error}");
            return;
        }
    };
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-mirror-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.create_bracket(
        &root,
        BracketRequest::new("keyboard-mirror-bracket", 60.0, 30.0, 40.0, 3.0)
            .with_feature_id("l-bracket"),
        &worker,
    )
    .expect("mirror bracket fixture persists");
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
    .expect("reinforcing pad fixture persists");
    let before_cancel = snapshot_tree(&root);
    let request = br#"{"feature_id":"tui-mirror-pad","base_feature_id":"reinforce-pad","plane_point":[0,0,0],"plane_normal":[1,-1,0]}"#;

    let mut cancel_events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    cancel_events.extend(b"mirror".iter().map(|byte| vec![*byte]));
    cancel_events.push(b"\r".to_vec());
    cancel_events.extend(request.iter().map(|byte| vec![*byte]));
    cancel_events.extend([b"\x16".to_vec(), b"\x1b".to_vec(), b"q".to_vec()]);
    cancel_events.reverse();
    let mut cancel_terminal = ScriptedTerminal {
        events: cancel_events,
        ..Default::default()
    };
    launch(&host, &root, &mut cancel_terminal, official_environment())
        .expect("mirror cancellation workflow succeeds");
    assert_eq!(snapshot_tree(&root), before_cancel);
    assert_eq!(
        host.identity(&root)
            .expect("cancel identity reads")
            .transaction_count,
        2
    );
    assert!(
        String::from_utf8_lossy(&cancel_terminal.writes)
            .contains("[cancellation-glyph] Cancellation: command draft discarded")
    );

    let mut commit_events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    commit_events.extend(b"mirror".iter().map(|byte| vec![*byte]));
    commit_events.push(b"\r".to_vec());
    commit_events.extend(request.iter().map(|byte| vec![*byte]));
    commit_events.extend([
        b"\x16".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    commit_events.reverse();
    let mut commit_terminal = ScriptedTerminal {
        events: commit_events,
        ..Default::default()
    };
    launch(&host, &root, &mut commit_terminal, official_environment())
        .expect("mirror commit workflow succeeds");

    let identity = host.identity(&root).expect("mirror identity reads");
    assert_eq!(identity.transaction_count, 3);
    for feature_id in ["reinforce-pad", "tui-mirror-pad"] {
        assert!(
            root.join("brep")
                .join(format!("{feature_id}.brep"))
                .is_file(),
            "retained BREP missing for {feature_id}"
        );
    }
    let bundle = Bundle::at(&root).open().expect("mirror bundle reopens");
    let intent = bundle
        .log
        .entries()
        .last()
        .and_then(|entry| entry.intent.as_ref())
        .expect("mirror intent retained");
    let CanonicalIntent::Mirror(intent) = intent else {
        panic!("last intent is not mirror");
    };
    assert_eq!(intent.deterministic_inputs.base_feature_id, "reinforce-pad");
    assert_eq!(intent.deterministic_inputs.plane_point, [0.0, 0.0, 0.0]);
    assert_eq!(intent.deterministic_inputs.plane_normal, [1.0, -1.0, 0.0]);

    let scene = host
        .read_only_viewport_scene(&root)
        .expect("mirror viewport scene reads");
    let (minimum, maximum) = solid_bounds(&scene, "tui-mirror-pad");
    for (axis, expected) in [(0, (12.0, 15.0)), (1, (27.0, 30.0)), (2, (0.0, 5.0))] {
        assert!((minimum[axis] - expected.0).abs() < 0.1);
        assert!((maximum[axis] - expected.1).abs() < 0.1);
    }
    assert!(
        scene
            .solids
            .iter()
            .any(|solid| solid.feature_id == "reinforce-pad")
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&cancel_terminal.writes),
        String::from_utf8_lossy(&commit_terminal.writes)
    );
    for marker in [
        "[dashed-outline] Preview: mirror",
        "[cancellation-glyph] Cancellation: command draft discarded",
        "[selection-glyph] Commit: mirror",
        "tui-mirror-pad",
        "reinforce-pad",
        "[viewport-status] Viewport presented",
    ] {
        assert!(
            output.contains(marker),
            "missing mirror evidence marker: {marker}"
        );
    }

    fs::remove_dir_all(root).expect("mirror fixture removes");
}

#[test]
fn production_launch_drives_linear_and_circular_patterns_through_keyboard() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("interactive pattern commands require OCCT worker: {error}");
        }
        Err(error) => {
            eprintln!("interactive pattern commands: OCCT worker unavailable: {error}");
            return;
        }
    };
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-patterns-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.create_bracket(
        &root,
        BracketRequest::new("keyboard-pattern-bracket", 60.0, 30.0, 40.0, 3.0)
            .with_feature_id("l-bracket"),
        &worker,
    )
    .expect("pattern bracket fixture persists");
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
    .expect("pattern reinforcing pad fixture persists");
    let initial_revision = host
        .identity(&root)
        .expect("initial pattern identity reads")
        .revision_hash;

    let mirror_request = br#"{"feature_id":"tui-mirror-pad","base_feature_id":"reinforce-pad","plane_point":[0,0,0],"plane_normal":[1,-1,0]}"#;
    let linear_request = br#"{"feature_id":"tui-linear-pads","base_feature_id":"reinforce-pad","direction":[1,0,0],"count":3,"spacing":12}"#;
    let circular_request = br#"{"feature_id":"tui-circular-lugs","base_feature_id":"reinforce-pad","axis_point":[30,15,0],"axis_normal":[0,0,1],"angle_step":1.5707963267948966,"count":4}"#;
    let mut events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()];
    let append_command =
        |events: &mut Vec<Vec<u8>>, command: &[u8], request: &[u8], image_id: u8| {
            events.push(b"\x10".to_vec());
            events.extend(command.iter().map(|byte| vec![*byte]));
            events.push(b"\r".to_vec());
            events.extend(request.iter().map(|byte| vec![*byte]));
            events.extend([
                b"\x16".to_vec(),
                b"\x1b[13;5u".to_vec(),
                format!("\x1b_Gi={image_id};OK\x1b\\").into_bytes(),
            ]);
        };
    append_command(&mut events, b"mirror", mirror_request, 2);
    append_command(&mut events, b"linear-pattern", linear_request, 3);
    append_command(&mut events, b"circular-pattern", circular_request, 4);
    events.push(b"q".to_vec());
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };
    launch(&host, &root, &mut terminal, official_environment())
        .expect("linear and circular pattern workflow succeeds");

    let identity = host.identity(&root).expect("pattern identity reads");
    assert_eq!(identity.transaction_count, 5);
    let bundle = Bundle::at(&root).open().expect("pattern bundle reopens");
    let intents = bundle
        .log
        .entries()
        .iter()
        .filter_map(|entry| entry.intent.as_ref())
        .collect::<Vec<_>>();
    assert!(
        intents
            .iter()
            .any(|intent| matches!(intent, CanonicalIntent::Mirror(_)))
    );
    let linear = intents
        .iter()
        .find_map(|intent| match intent {
            CanonicalIntent::LinearPattern(intent) => Some(intent),
            _ => None,
        })
        .expect("linear pattern intent retained");
    assert_eq!(linear.deterministic_inputs.base_feature_id, "reinforce-pad");
    assert_eq!(linear.deterministic_inputs.direction, [1.0, 0.0, 0.0]);
    assert_eq!(linear.deterministic_inputs.count, 3);
    assert_eq!(linear.deterministic_inputs.spacing, 12.0);
    let circular = intents
        .iter()
        .find_map(|intent| match intent {
            CanonicalIntent::CircularPattern(intent) => Some(intent),
            _ => None,
        })
        .expect("circular pattern intent retained");
    assert_eq!(
        circular.deterministic_inputs.base_feature_id,
        "reinforce-pad"
    );
    assert_eq!(circular.deterministic_inputs.axis_point, [30.0, 15.0, 0.0]);
    assert_eq!(circular.deterministic_inputs.axis_normal, [0.0, 0.0, 1.0]);
    assert_eq!(circular.deterministic_inputs.count, 4);
    assert!((circular.deterministic_inputs.angle_step - std::f64::consts::FRAC_PI_2).abs() < 1e-12);

    let scene = host
        .read_only_viewport_scene(&root)
        .expect("pattern viewport scene reads");
    for (feature_id, expected) in [
        ("tui-mirror-pad", [(12.0, 15.0), (27.0, 30.0), (0.0, 5.0)]),
        ("tui-linear-pads", [(27.0, 54.0), (12.0, 15.0), (0.0, 5.0)]),
        (
            "tui-circular-lugs",
            [(27.0, 33.0), (12.0, 18.0), (0.0, 5.0)],
        ),
    ] {
        let (minimum, maximum) = solid_bounds(&scene, feature_id);
        for (axis, (lower, upper)) in expected.into_iter().enumerate() {
            assert!(
                (minimum[axis] - lower).abs() < 0.1,
                "{feature_id} minimum axis {axis}"
            );
            assert!(
                (maximum[axis] - upper).abs() < 0.1,
                "{feature_id} maximum axis {axis}"
            );
        }
    }
    for (lower, upper) in [(27.0, 30.0), (39.0, 42.0), (51.0, 54.0)] {
        assert!(
            solid_has_triangle_centroid_in_xy_window(
                &scene,
                "tui-linear-pads",
                (lower, upper),
                (12.0, 15.0)
            ),
            "linear pattern has no occupied band x=[{lower},{upper}]"
        );
    }
    for (x_range, y_range) in [
        ((27.0, 30.0), (12.0, 15.0)),
        ((30.0, 33.0), (12.0, 15.0)),
        ((30.0, 33.0), (15.0, 18.0)),
        ((27.0, 30.0), (15.0, 18.0)),
    ] {
        assert!(
            solid_has_triangle_centroid_in_xy_window(&scene, "tui-circular-lugs", x_range, y_range),
            "circular pattern has no occupied sector x={x_range:?} y={y_range:?}"
        );
    }
    for feature_id in [
        "reinforce-pad",
        "tui-mirror-pad",
        "tui-linear-pads",
        "tui-circular-lugs",
    ] {
        assert!(
            root.join("brep")
                .join(format!("{feature_id}.brep"))
                .is_file(),
            "retained BREP missing for {feature_id}"
        );
    }
    let output = String::from_utf8_lossy(&terminal.writes);
    for marker in [
        "[dashed-outline] Preview: mirror",
        "[dashed-outline] Preview: linear-pattern",
        "[dashed-outline] Preview: circular-pattern",
        "[selection-glyph] Commit: mirror",
        "[selection-glyph] Commit: linear-pattern",
        "[selection-glyph] Commit: circular-pattern",
        "tui-mirror-pad",
        "tui-linear-pads",
        "tui-circular-lugs",
        "[viewport-status] Viewport presented",
    ] {
        assert!(
            output.contains(marker),
            "missing pattern evidence marker: {marker}"
        );
    }
    let mirror_revision = committed_revision(&output, "mirror");
    let linear_revision = committed_revision(&output, "linear-pattern");
    let circular_revision = committed_revision(&output, "circular-pattern");
    assert_ne!(mirror_revision, initial_revision);
    assert_ne!(linear_revision, mirror_revision);
    assert_ne!(circular_revision, linear_revision);
    assert_eq!(circular_revision, identity.revision_hash);

    let viewport_evidence = json!({
        "acknowledgement": "viewport-presented",
        "revision": scene.revision.clone(),
        "scene": {
            "solids": scene
                .solids
                .iter()
                .map(|solid| json!({
                    "feature_id": &solid.feature_id,
                    "triangle_count": solid.triangles.len(),
                }))
                .collect::<Vec<_>>(),
        },
    });
    let transcript = root.join("reinforcing-transcript.jsonl");
    let transcript_entries = [
        json!({
            "command": "mirror",
            "request": serde_json::from_slice::<Value>(mirror_request).expect("mirror request JSON"),
            "preview_marker": "[dashed-outline] Preview: mirror",
            "commit_marker": "[selection-glyph] Commit: mirror",
            "response": {
                "feature_id": "tui-mirror-pad",
                "revision": mirror_revision.clone(),
            },
            "viewport_evidence": viewport_evidence.clone(),
        }),
        json!({
            "command": "linear-pattern",
            "request": serde_json::from_slice::<Value>(linear_request).expect("linear request JSON"),
            "preview_marker": "[dashed-outline] Preview: linear-pattern",
            "commit_marker": "[selection-glyph] Commit: linear-pattern",
            "response": {
                "feature_id": "tui-linear-pads",
                "revision": linear_revision.clone(),
            },
            "viewport_evidence": viewport_evidence.clone(),
        }),
        json!({
            "command": "circular-pattern",
            "request": serde_json::from_slice::<Value>(circular_request).expect("circular request JSON"),
            "preview_marker": "[dashed-outline] Preview: circular-pattern",
            "commit_marker": "[selection-glyph] Commit: circular-pattern",
            "response": {
                "feature_id": "tui-circular-lugs",
                "revision": circular_revision,
            },
            "viewport_evidence": viewport_evidence,
        }),
    ];
    fs::write(
        &transcript,
        transcript_entries
            .into_iter()
            .map(|entry| serde_json::to_string(&entry).expect("transcript entry serializes"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .expect("reinforcing transcript writes");
    let transcript_text = fs::read_to_string(&transcript).expect("reinforcing transcript reads");
    for feature_id in ["tui-mirror-pad", "tui-linear-pads", "tui-circular-lugs"] {
        assert!(transcript_text.contains(feature_id));
    }
    for line in transcript_text.lines() {
        let entry: Value = serde_json::from_str(line).expect("production transcript line is JSON");
        assert!(entry["request"].is_object());
        assert!(entry["preview_marker"].as_str().is_some());
        assert!(entry["commit_marker"].as_str().is_some());
        assert!(entry["response"]["revision"].as_str().is_some());
        assert_eq!(
            entry["viewport_evidence"]["acknowledgement"],
            "viewport-presented"
        );
        assert!(
            entry["viewport_evidence"]["scene"]["solids"]
                .as_array()
                .is_some_and(|solids| solids
                    .iter()
                    .all(|solid| { solid["triangle_count"].as_u64().unwrap_or(0) > 0 }))
        );
    }

    let identity_before_replay = host.identity(&root).expect("pre-replay identity reads");
    let manifest_before_replay = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before_replay = fs::read(root.join("transactions.log")).expect("log reads");
    fs::remove_dir_all(root.join("brep")).expect("derived BREP directory removes");
    let replayed_host = Host::new();
    replayed_host
        .load_with_geometry_replay(&root)
        .expect("fresh host replays all reinforcing geometry");
    assert_eq!(
        replayed_host
            .identity(&root)
            .expect("replay identity reads"),
        identity_before_replay
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("replayed manifest reads"),
        manifest_before_replay
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("replayed log reads"),
        log_before_replay
    );
    let replayed_scene = replayed_host
        .presentation_viewport_scene()
        .expect("replayed viewport scene reads");
    for feature_id in [
        "l-bracket",
        "reinforce-pad",
        "tui-mirror-pad",
        "tui-linear-pads",
        "tui-circular-lugs",
    ] {
        let solid = replayed_scene
            .solids
            .iter()
            .find(|solid| solid.feature_id == feature_id)
            .unwrap_or_else(|| panic!("replayed viewport scene has no solid {feature_id}"));
        assert!(
            !solid.triangles.is_empty(),
            "replayed solid is empty: {feature_id}"
        );
    }

    fs::remove_dir_all(root).expect("pattern fixture removes");
}

#[test]
fn production_launch_drives_boolean_fuse_through_palette_and_commit() {
    let Some(worker) = optional_occt_worker("interactive boolean fuse") else {
        return;
    };
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-fuse-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("project is persisted");
    host.extrude(
        &root,
        ExtrudeRequest::new(
            "fuse-launch-arm-x",
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
            3.0,
        )
        .with_feature_id("arm-x"),
        &worker,
    )
    .expect("first arm extrudes");
    host.extrude(
        &root,
        ExtrudeRequest::new(
            "fuse-launch-arm-z",
            vec![(0.0, 0.0), (5.0, 0.0), (5.0, 10.0), (0.0, 10.0)],
            3.0,
        )
        .with_feature_id("arm-z"),
        &worker,
    )
    .expect("second arm extrudes");

    let request =
        br#"{"feature_id":"interactive-fuse","base_feature_id":"arm-x","tool_feature_id":"arm-z"}"#;
    let mut script = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    script.extend(b"boolean-fuse".iter().map(|byte| vec![*byte]));
    script.push(b"\r".to_vec());
    script.extend(request.iter().map(|byte| vec![*byte]));
    script.extend([
        b"\x16".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    script.reverse();
    let mut terminal = ScriptedTerminal {
        events: script,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("production boolean fuse palette flow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, 4);
    assert!(root.join("brep/interactive-fuse.brep").is_file());
    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("command preview ready"));
    assert!(output.contains("Commit: boolean-fuse"));
    assert!(output.contains("[viewport-status] Viewport presented"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_launch_assembles_bracket_foundation_through_tui_controls() {
    let Some(worker) = optional_occt_worker("interactive bracket foundation") else {
        return;
    };
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-bracket-foundation-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("project is persisted");

    let assert_commit = |response: &Value, operation: &str, feature_id: &str| {
        assert_eq!(response["status"], "ok", "{feature_id} commits");
        assert_eq!(response["operation"], operation);
        assert_eq!(response["feature_id"], feature_id);
        assert!(response["revision_hash"].as_str().is_some());
        assert!(root.join(format!("brep/{feature_id}.brep")).is_file());
    };
    let response = run_palette_command(
        &host,
        &root,
        "extrude",
        json!({
            "feature_id": "arm-x",
            "profile": [[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 20.0]],
            "height": 8.0,
            "mode": "additive"
        }),
    );
    assert_commit(&response, "extrude", "arm-x");
    let response = run_palette_command(
        &host,
        &root,
        "extrude",
        json!({
            "feature_id": "arm-z",
            "profile": [[0.0, 0.0], [20.0, 0.0], [20.0, 60.0], [0.0, 60.0]],
            "height": 8.0,
            "mode": "additive"
        }),
    );
    assert_commit(&response, "extrude", "arm-z");
    let response = run_palette_command(
        &host,
        &root,
        "extrude",
        json!({
            "feature_id": "pad-a-seed",
            "profile": [[24.0, 4.0], [36.0, 4.0], [36.0, 16.0], [24.0, 16.0]],
            "height": 12.0,
            "mode": "additive"
        }),
    );
    assert_commit(&response, "extrude", "pad-a-seed");

    let revision = host
        .identity(&root)
        .expect("pad-a source identity")
        .revision_hash;
    let response = run_palette_command(
        &host,
        &root,
        "fillet",
        json!({
            "feature_id": "pad-a",
            "base_feature_id": "pad-a-seed",
            "radius": 0.5,
            "selected_edge": selected_edge(
                "pad-a-seed",
                &revision,
                "pad-a-edge",
                [30.0, 4.0, 0.0],
                12.0
            )
        }),
    );
    assert_commit(&response, "fillet", "pad-a");

    let response = run_palette_command(
        &host,
        &root,
        "extrude",
        json!({
            "feature_id": "pad-b-seed",
            "profile": [[4.0, 24.0], [16.0, 24.0], [16.0, 36.0], [4.0, 36.0]],
            "height": 12.0,
            "mode": "additive"
        }),
    );
    assert_commit(&response, "extrude", "pad-b-seed");

    let revision = host
        .identity(&root)
        .expect("pad-b source identity")
        .revision_hash;
    let response = run_palette_command(
        &host,
        &root,
        "chamfer",
        json!({
            "feature_id": "pad-b",
            "base_feature_id": "pad-b-seed",
            "distance": 0.25,
            "selected_edge": selected_edge(
                "pad-b-seed",
                &revision,
                "pad-b-edge",
                [10.0, 24.0, 0.0],
                12.0
            )
        }),
    );
    assert_commit(&response, "chamfer", "pad-b");

    for (feature_id, base_feature_id, tool_feature_id) in [
        ("bracket-l", "arm-x", "arm-z"),
        ("bracket-lp1", "bracket-l", "pad-a"),
        ("bracket-base", "bracket-lp1", "pad-b"),
    ] {
        let response = run_palette_command(
            &host,
            &root,
            "boolean-fuse",
            json!({
                "feature_id": feature_id,
                "base_feature_id": base_feature_id,
                "tool_feature_id": tool_feature_id
            }),
        );
        assert_commit(&response, "boolean_fuse", feature_id);
    }

    for (feature_id, base_feature_id, position) in [
        ("bracket-hole-1", "bracket-base", [50.0, 10.0, 0.0]),
        ("bracket-foundation", "bracket-hole-1", [10.0, 50.0, 0.0]),
    ] {
        let response = run_palette_command(
            &host,
            &root,
            "hole",
            json!({
                "feature_id": feature_id,
                "base_feature_id": base_feature_id,
                "position": position,
                "direction": [0.0, 0.0, 1.0],
                "diameter": 4.5,
                "hole_kind": "drilled"
            }),
        );
        assert_commit(&response, "hole", feature_id);
    }

    let bundle = Bundle::at(&root).open().expect("interactive bundle opens");
    let entries = bundle.log.entries();
    assert_eq!(entries.len(), 11);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.log_index, index);
        assert!(!entry.terminal_digest.is_empty());
        assert!(
            entry.intent.is_some(),
            "{feature_id} retains its canonical intent",
            feature_id = entry.feature_id
        );
    }
    let feature_ids = entries
        .iter()
        .map(|entry| entry.feature_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        feature_ids,
        [
            "arm-x",
            "arm-z",
            "pad-a-seed",
            "pad-a",
            "pad-b-seed",
            "pad-b",
            "bracket-l",
            "bracket-lp1",
            "bracket-base",
            "bracket-hole-1",
            "bracket-foundation"
        ]
    );

    let intent_value = |index: usize| {
        serde_json::to_value(
            entries[index]
                .intent
                .as_ref()
                .expect("transaction retains canonical intent"),
        )
        .expect("canonical intent serializes")
    };
    for (index, command, operation) in [
        (0, "extrude", "extrude"),
        (1, "extrude", "extrude"),
        (2, "extrude", "extrude"),
        (3, "fillet", "fillet"),
        (4, "extrude", "extrude"),
        (5, "chamfer", "chamfer"),
        (6, "boolean", "fuse"),
        (7, "boolean", "fuse"),
        (8, "boolean", "fuse"),
        (9, "hole", "hole"),
        (10, "hole", "hole"),
    ] {
        let intent = intent_value(index);
        assert_eq!(
            intent["command"], command,
            "intent command at index {index}"
        );
        assert_eq!(
            intent["operation"], operation,
            "intent operation at index {index}"
        );
        assert!(
            intent["source_revision"]
                .as_str()
                .is_some_and(|revision| !revision.is_empty())
        );
    }
    assert_eq!(
        intent_value(0)["deterministic_inputs"]["profile"],
        json!([[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 20.0]])
    );
    assert_eq!(intent_value(0)["deterministic_inputs"]["height"], 8.0);
    assert_eq!(
        intent_value(1)["deterministic_inputs"]["profile"],
        json!([[0.0, 0.0], [20.0, 0.0], [20.0, 60.0], [0.0, 60.0]])
    );
    assert_eq!(
        intent_value(2)["deterministic_inputs"]["profile"],
        json!([[24.0, 4.0], [36.0, 4.0], [36.0, 16.0], [24.0, 16.0]])
    );
    assert_eq!(intent_value(3)["base_feature_id"], "pad-a-seed");
    assert_eq!(intent_value(3)["radius"], 0.5);
    assert_eq!(
        intent_value(3)["selected_edge"]["provenance"]["source_feature_id"],
        "pad-a-seed"
    );
    assert_eq!(intent_value(4)["deterministic_inputs"]["height"], 12.0);
    assert_eq!(intent_value(5)["base_feature_id"], "pad-b-seed");
    assert_eq!(intent_value(5)["distance"], 0.25);
    assert_eq!(
        intent_value(5)["selected_edge"]["provenance"]["source_feature_id"],
        "pad-b-seed"
    );
    for (index, base_feature_id, tool_feature_id) in [
        (6, "arm-x", "arm-z"),
        (7, "bracket-l", "pad-a"),
        (8, "bracket-lp1", "pad-b"),
    ] {
        let intent = intent_value(index);
        assert_eq!(intent["base_feature_id"], base_feature_id);
        assert_eq!(intent["tool_feature_id"], tool_feature_id);
    }
    for (index, base_feature_id, position) in [
        (9, "bracket-base", [50.0, 10.0, 0.0]),
        (10, "bracket-hole-1", [10.0, 50.0, 0.0]),
    ] {
        let intent = intent_value(index);
        assert_eq!(intent["base_feature_id"], base_feature_id);
        assert_eq!(intent["hole_kind"], "drilled");
        assert_eq!(intent["deterministic_inputs"]["position"], json!(position));
        assert_eq!(
            intent["deterministic_inputs"]["direction"],
            json!([0.0, 0.0, 1.0])
        );
        assert_eq!(intent["deterministic_inputs"]["diameter"], 4.5);
    }

    let revision = bundle.revision_hash_hex().to_string();
    let pad_a = worker
        .inspect_edges(
            "tui-pad-a-measurement",
            root.join("brep/pad-a.brep"),
            "pad-a",
            &revision,
            json!({"provenance": {"source_feature_id": "pad-a", "source_revision_id": revision, "source_edge_id": "measurement-anchor"}}),
        )
        .expect("fillet landmarks inspect");
    assert!(pad_a.edge_candidates.iter().any(|candidate| {
        candidate.role == "fillet-transition"
            && (candidate.length - std::f64::consts::FRAC_PI_4).abs() < 1e-3
    }));
    let pad_b = worker
        .inspect_edges(
            "tui-pad-b-measurement",
            root.join("brep/pad-b.brep"),
            "pad-b",
            &revision,
            json!({"provenance": {"source_feature_id": "pad-b", "source_revision_id": revision, "source_edge_id": "measurement-anchor"}}),
        )
        .expect("chamfer landmarks inspect");
    let pad_b_seed = worker
        .inspect_edges(
            "tui-pad-b-seed-measurement",
            root.join("brep/pad-b-seed.brep"),
            "pad-b-seed",
            &revision,
            json!({"provenance": {"source_feature_id": "pad-b-seed", "source_revision_id": revision, "source_edge_id": "measurement-anchor"}}),
        )
        .expect("chamfer seed landmarks inspect");
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
            "tui-final-measurement",
            root.join("brep/bracket-foundation.brep"),
            "bracket-foundation",
            &revision,
            json!({"provenance": {"source_feature_id": "bracket-foundation", "source_revision_id": revision, "source_edge_id": "measurement-anchor"}}),
        )
        .expect("final hole landmarks inspect");
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

    let before_read_only = snapshot_tree(&root);
    let before_scene = host
        .read_only_viewport_scene(&root)
        .expect("final project renders read-only");
    assert_eq!(snapshot_tree(&root), before_read_only);
    assert_eq!(before_scene.solids.len(), 1);
    let final_solid = before_scene
        .solids
        .iter()
        .find(|solid| solid.feature_id == "bracket-foundation")
        .expect("final fused body is visible");
    assert!(!final_solid.triangles.is_empty());
    let baseline_revision = bundle.revision_hash_hex().to_string();
    let baseline_entries = entries.to_vec();
    drop(bundle);
    let reopened = Bundle::at(&root)
        .open_read_only()
        .expect("retained project reopens read-only");
    assert_eq!(reopened.revision_hash_hex(), baseline_revision);
    assert_eq!(reopened.log.entries(), baseline_entries);
    let reopened_scene = host
        .read_only_viewport_scene(&root)
        .expect("retained project renders read-only after reopen");
    assert_eq!(reopened_scene, before_scene);
    assert_eq!(snapshot_tree(&root), before_read_only);

    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires the pinned native OCCT worker"]
fn production_launch_drives_one_revolve_draft_through_preview_and_commit() {
    let worker = OcctWorker::locate().expect("interactive revolve command requires OCCT worker");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-revolve-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("project is persisted");

    let request = br#"{"feature_id":"revolved-collar","profile":[[20,28],[22,28],[22,32],[20,32]],"axis_point":[10,0,0],"axis_direction":[0,-1,0],"angle":1.5707963267948966}"#;
    let mut script = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    script.extend(b"revolve".iter().map(|byte| vec![*byte]));
    script.push(b"\r".to_vec());
    script.extend(request.iter().map(|byte| vec![*byte]));
    script.push(b"\x16".to_vec());
    script.push(b"\x1b[13;5u".to_vec());
    script.push(b"q".to_vec());
    script.reverse();
    let mut terminal = ScriptedTerminal {
        events: script,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("production revolve palette flow succeeds");

    let identity = host.identity(&root).expect("committed identity reads");
    assert_eq!(identity.transaction_count, 2);
    assert!(root.join("brep/revolved-collar.brep").is_file());
    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("command preview ready"));
    assert!(output.contains("command committed"));
    assert!(output.contains("[selection-glyph]"));
    let bundle = Bundle::at(&root)
        .open_read_only()
        .expect("committed bundle opens read-only");
    assert!(matches!(
        bundle
            .log
            .entries()
            .last()
            .and_then(|entry| entry.intent.as_ref()),
        Some(CanonicalIntent::Revolve(_))
    ));
    drop(worker);
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires the pinned native OCCT worker"]
fn production_launch_drives_the_frozen_reinforcement_recipe_through_the_tui() {
    let worker = OcctWorker::locate().expect("reinforcement recipe requires OCCT worker");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-reinforcement-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    prepare_reinforcement_foundation(&host, &root);
    let recipe: Value =
        serde_json::from_str(REINFORCEMENT_RECIPE).expect("reinforcement recipe is valid JSON");

    let mut script = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()];
    for (image_id, step) in (2..).zip(
        recipe["steps"]
            .as_array()
            .expect("recipe steps are an array")
            .iter()
            .skip(12),
    ) {
        let command = step["command"]
            .as_str()
            .expect("recipe command is a string");
        let request = serde_json::to_vec(&step["request"]).expect("recipe request serializes");
        script.push(b"\x10".to_vec());
        script.extend(command.bytes().map(|byte| vec![byte]));
        script.push(b"\r".to_vec());
        script.extend(request.iter().map(|byte| vec![*byte]));
        script.push(b"\x16".to_vec());
        script.push(b"\x1b[13;5u".to_vec());
        script.push(format!("\x1b_Gi={image_id};OK\x1b\\").into_bytes());
    }
    script.push(b"q".to_vec());
    script.reverse();
    let mut terminal = ScriptedTerminal {
        events: script,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("reinforcement recipe keyboard workflow succeeds");

    let identity = host.identity(&root).expect("reinforcement identity reads");
    assert_eq!(identity.transaction_count, 19);
    let bundle = Bundle::at(&root)
        .open_read_only()
        .expect("reinforcement bundle opens read-only");
    let expected_features = recipe["expectations"]["final_feature_ids"]
        .as_array()
        .expect("recipe final feature IDs are an array")
        .iter()
        .map(|feature| feature.as_str().expect("recipe feature ID is a string"))
        .collect::<Vec<_>>();
    assert_eq!(
        bundle
            .log
            .entries()
            .iter()
            .map(|entry| entry.feature_id.as_str())
            .collect::<Vec<_>>(),
        expected_features
    );
    for command in [
        "revolve",
        "extrude",
        "shell",
        "hole",
        "boolean-fuse",
        "save",
    ] {
        assert!(
            String::from_utf8_lossy(&terminal.writes)
                .contains(&format!("[selection-glyph] Commit: {command}")),
            "TUI transcript omits commit acknowledgement for {command}"
        );
    }
    let output = String::from_utf8_lossy(&terminal.writes);
    let removed_volume = output
        .split("removed_volume=")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse::<f64>().ok())
        .expect("TUI transcript includes the removed-volume measurement");
    assert!((removed_volume - 58.90486225480863).abs() <= 0.001);
    assert!(bundle.log.entries()[..18].iter().all(|entry| {
        entry.brep_path.is_some() && entry.brep_sha256.is_some() && entry.intent.is_some()
    }));
    let expected_commands = recipe["steps"]
        .as_array()
        .expect("recipe steps are an array")
        .iter()
        .take(18)
        .map(|step| {
            step["command"]
                .as_str()
                .expect("recipe command is a string")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        bundle.log.entries()[..18]
            .iter()
            .map(|entry| entry
                .intent
                .as_ref()
                .expect("geometry entry retains intent")
                .command())
            .collect::<Vec<_>>(),
        expected_commands
    );
    assert!(bundle.log.entries()[..18].windows(2).all(|entries| {
        entries[0]
            .intent
            .as_ref()
            .expect("geometry entry retains intent")
            .source_revision()
            != entries[1]
                .intent
                .as_ref()
                .expect("geometry entry retains intent")
                .source_revision()
    }));
    for (step, entry) in recipe["steps"]
        .as_array()
        .expect("recipe steps are an array")
        .iter()
        .skip(12)
        .take(6)
        .zip(bundle.log.entries()[12..18].iter())
    {
        let intent = entry
            .intent
            .as_ref()
            .expect("recipe geometry entry retains intent");
        let canonical = serde_json::to_value(intent).expect("canonical intent serializes");
        let request = &step["request"];
        assert_eq!(
            canonical["affected_semantic_ids"],
            json!([step["feature_id"].clone()])
        );
        match step["command"]
            .as_str()
            .expect("recipe command is a string")
        {
            "revolve" => {
                assert_eq!(
                    canonical["deterministic_inputs"]["profile"],
                    request["profile"]
                );
                assert_eq!(
                    canonical["deterministic_inputs"]["axis_point"],
                    request["axis_point"]
                );
                assert_eq!(
                    canonical["deterministic_inputs"]["axis_direction"],
                    request["axis_direction"]
                );
                assert_eq!(canonical["deterministic_inputs"]["angle"], request["angle"]);
            }
            "extrude" => {
                assert_eq!(
                    canonical["deterministic_inputs"]["profile"],
                    request["profile"]
                );
                assert_eq!(
                    canonical["deterministic_inputs"]["height"],
                    request["height"]
                );
                assert_eq!(canonical["mode"], request["mode"]);
            }
            "shell" => {
                assert_eq!(canonical["base_feature_id"], request["base_feature_id"]);
                assert_eq!(canonical["thickness"], request["thickness"]);
            }
            "hole" => {
                assert_eq!(canonical["base_feature_id"], request["base_feature_id"]);
                assert_eq!(canonical["hole_kind"], request["hole_kind"]);
                assert_eq!(
                    canonical["deterministic_inputs"]["position"],
                    request["position"]
                );
                assert_eq!(
                    canonical["deterministic_inputs"]["direction"],
                    request["direction"]
                );
                assert_eq!(
                    canonical["deterministic_inputs"]["diameter"],
                    request["diameter"]
                );
            }
            "boolean-fuse" => {
                assert_eq!(canonical["operation"], "fuse");
                assert_eq!(canonical["base_feature_id"], request["base_feature_id"]);
                assert_eq!(canonical["tool_feature_id"], request["tool_feature_id"]);
            }
            command => panic!("unexpected frozen recipe command: {command}"),
        }
    }
    assert!(
        bundle.log.entries()[..18]
            .windows(2)
            .all(|entries| entries[0].terminal_digest != entries[1].terminal_digest)
    );
    assert!(bundle.log.entries()[18].intent.is_none());

    let mut breps = BTreeMap::new();
    for feature_id in expected_features.iter().filter(|feature_id| {
        **feature_id != "foundation-snapshot" && **feature_id != "reinforcement-snapshot"
    }) {
        let path = root.join("brep").join(format!("{feature_id}.brep"));
        assert!(path.is_file(), "committed BREP is missing for {feature_id}");
        breps.insert(
            (*feature_id).to_string(),
            fs::read(path).expect("committed BREP reads"),
        );
    }
    let log_before_replay = fs::read(root.join("transactions.log")).expect("transaction log reads");

    let collar_measurements = worker
        .inspect_edges(
            "revolved-collar-measurement",
            root.join("brep/revolved-collar.brep"),
            "revolved-collar",
            identity.revision_hash.clone(),
            json!({"source_feature_id":"revolved-collar","source_revision_id":identity.revision_hash,"source_edge_id":"measurement-anchor"}),
        )
        .expect("collar edges inspect");
    for expected_length in [15.707963267948966_f64, 18.84955592153876_f64] {
        assert!(collar_measurements.edge_candidates.iter().any(|candidate| {
            candidate.role == "fillet-transition"
                && (candidate.length - expected_length).abs() <= 1e-3
                && (28.0..=32.0).contains(&candidate.midpoint[1])
        }));
    }
    let shell_measurements = worker
        .inspect_edges(
            "hollow-detail-measurement",
            root.join("brep/hollow-detail.brep"),
            "hollow-detail",
            identity.revision_hash.clone(),
            json!({"source_feature_id":"hollow-detail","source_revision_id":identity.revision_hash,"source_edge_id":"measurement-anchor"}),
        )
        .expect("shell edges inspect");
    assert!(
        (shell_measurements
            .material_volume
            .expect("shell material volume")
            - 1167.0)
            .abs()
            <= 0.001
    );
    for expected_length in [10.0_f64, 7.0_f64] {
        assert!(shell_measurements.edge_candidates.iter().any(|candidate| {
            candidate.role == "outer-perimeter"
                && (candidate.length - expected_length).abs() <= 1e-3
        }));
    }
    let final_measurements = worker
        .inspect_edges(
            "reinforced-foundation-measurement",
            root.join("brep/reinforced-foundation.brep"),
            "reinforced-foundation",
            identity.revision_hash.clone(),
            json!({"source_feature_id":"reinforced-foundation","source_revision_id":identity.revision_hash,"source_edge_id":"measurement-anchor"}),
        )
        .expect("final edges inspect");
    assert!(final_measurements.edge_candidates.iter().any(|candidate| {
        candidate.role == "fillet-transition"
            && (candidate.length - 15.707963267948966).abs() <= 1e-3
            && candidate
                .midpoint
                .into_iter()
                .zip([49.5, 10.0, 20.0])
                .all(|(actual, expected)| (actual - expected).abs() <= 1e-3)
    }));
    for expected_length in [10.0_f64, 7.0_f64] {
        assert!(final_measurements.edge_candidates.iter().any(|candidate| {
            candidate.role == "outer-perimeter"
                && (candidate.length - expected_length).abs() <= 1e-3
        }));
    }
    assert!(final_measurements.edge_candidates.iter().any(|candidate| {
        candidate.role == "fillet-transition"
            && candidate.length > 1e-3
            && (28.0..=32.0).contains(&candidate.midpoint[1])
            && candidate.midpoint[0] >= 20.0
    }));

    fs::remove_dir_all(root.join("brep")).expect("derived BREPs remove");
    let replay_host = Host::new();
    let replayed = replay_host
        .load_with_geometry_replay(&root)
        .expect("reinforcement geometry replay succeeds");
    assert_eq!(replayed.revision_hash, identity.revision_hash);
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("replayed log reads"),
        log_before_replay
    );
    for (feature_id, original) in breps {
        assert_eq!(
            fs::read(root.join("brep").join(format!("{feature_id}.brep")))
                .expect("replayed BREP reads"),
            original,
            "replayed geometry for {feature_id}"
        );
    }
    assert!(
        replay_host
            .presentation_viewport_scene()
            .expect("replayed viewport scene reads")
            .solids
            .iter()
            .any(|solid| solid.feature_id == "reinforced-foundation")
    );
    let replayed_bundle = Bundle::at(&root)
        .open_read_only()
        .expect("replayed bundle opens read-only");
    let mut replay_selector =
        TuiSession::from_feature_graph(&replayed_bundle.graph, replayed.revision_hash.clone());
    let mut selected_final = false;
    for _ in 0..=replayed_bundle.graph.features().count() * 2 {
        if replay_selector.state().selected_target.as_deref() == Some("reinforced-foundation") {
            selected_final = true;
            break;
        }
        replay_selector
            .process_terminal_input(b"\x1b[B")
            .expect("replayed feature selection advances");
    }
    assert!(selected_final, "replayed final feature remains selectable");
    assert!(
        replayed_bundle
            .graph
            .contains_feature("reinforced-foundation")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires the pinned native OCCT worker"]
fn production_launch_retains_preview_cancellation_and_recommit_evidence() {
    OcctWorker::locate().expect("preview cancellation workflow requires the OCCT worker");

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-preview-cancel-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("preview cancellation fixture persists");
    let before_identity = host.identity(&root).expect("canonical identity reads");
    let before_tree = snapshot_tree(&root);
    let before_scene = host
        .read_only_viewport_scene(&root)
        .expect("canonical scene reads");
    let request = br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}"#;

    let mut events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    events.extend(b"extrude".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(request.iter().map(|byte| vec![*byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"\x1b".to_vec(),
        b"\x1b_Gi=3;OK\x1b\\".to_vec(),
        b"\x10".to_vec(),
    ]);
    events.extend(b"extrude".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(request.iter().map(|byte| vec![*byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b_Gi=4;OK\x1b\\".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=5;OK\x1b\\".to_vec(),
        b"\x1b[B".to_vec(),
        b"\x1b_Gi=6;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };

    let outcome = launch(&host, &root, &mut terminal, official_environment())
        .expect("preview cancellation and recommit workflow succeeds");

    assert_eq!(
        host.identity(&root)
            .expect("identity remains readable")
            .transaction_count,
        before_identity.transaction_count + 1
    );
    assert_ne!(
        host.identity(&root).expect("committed identity reads"),
        before_identity
    );
    assert_ne!(snapshot_tree(&root), before_tree);
    assert_eq!(before_scene.revision, before_identity.revision_hash);
    assert!(root.join("brep/keyboard-extrude.brep").is_file());
    assert!(!root.join(".derived").exists());

    let transcript = outcome.action_transcript;
    let kinds = transcript
        .entries
        .iter()
        .map(|entry| entry.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            "draft_opened",
            "preview_ready",
            "cancelled",
            "draft_opened",
            "preview_ready",
            "committed"
        ]
    );
    assert!(
        transcript.entries[1]
            .scene
            .as_ref()
            .is_some_and(|scene| scene.triangle_count > 0 && scene.body_pixels > 0)
    );
    assert_eq!(
        transcript.entries[2].canonical_revision,
        before_identity.revision_hash
    );
    assert!(
        transcript.entries[4]
            .scene
            .as_ref()
            .is_some_and(|scene| scene.triangle_count > 0 && scene.body_pixels > 0)
    );
    assert_ne!(
        transcript.entries[5].canonical_revision,
        before_identity.revision_hash
    );
    assert!(String::from_utf8_lossy(&terminal.writes).contains("[action-transcript]"));

    let bundle = Bundle::at(&root).open().expect("committed bundle opens");
    let intent = bundle
        .log
        .entries()
        .last()
        .and_then(|entry| entry.intent.as_ref());
    let Some(CanonicalIntent::Extrude(intent)) = intent else {
        panic!("recommitted feature retains extrusion intent");
    };
    assert_eq!(intent.deterministic_inputs.height, 3.0);
    assert_eq!(intent.mode, "additive");
    assert_eq!(
        intent.deterministic_inputs.profile,
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]
    );

    fs::remove_dir_all(root).expect("preview cancellation fixture removes");
}

#[test]
#[ignore = "requires the pinned native OCCT worker"]
fn production_launch_cancels_typed_extrusion_without_mutation() {
    OcctWorker::locate().expect("extrusion cancellation requires the OCCT worker");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-production-launch-extrude-cancel-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("cancellation fixture persists");
    let before_identity = host.identity(&root).expect("cancellation identity reads");
    let before_tree = snapshot_tree(&root);
    let request =
        br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}"#;
    let mut events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    events.extend(b"extrude".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(request.iter().map(|byte| vec![*byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
        b"\x1b".to_vec(),
        b"\x1b_Gi=3;OK\x1b\\".to_vec(),
        b"q".to_vec(),
    ]);
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("extrusion cancellation workflow succeeds");

    assert_eq!(
        host.identity(&root).expect("identity remains readable"),
        before_identity
    );
    assert_eq!(snapshot_tree(&root), before_tree);
    assert!(!root.join("brep/keyboard-extrude.brep").exists());
    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains("[dashed-outline] Preview: extrude"));
    assert!(output.contains("[cancellation-glyph] Cancellation: command draft discarded"));

    fs::remove_dir_all(root).expect("cancellation fixture removes");
}

#[test]
#[ignore = "requires the pinned native OCCT worker"]
fn production_launch_creates_project_and_extrudes_typed_profile() {
    OcctWorker::locate().expect("fresh keyboard workflow requires the OCCT worker");

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let workspace = std::env::temp_dir().join(format!(
        "threeterm-fresh-keyboard-workflow-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&workspace).expect("workflow workspace creates");
    let launch_root = workspace.join("launch-placeholder");
    let project_root = workspace.join("created-project");
    assert!(!launch_root.exists());
    assert!(!project_root.exists());

    let host = Host::new();
    let request =
        br#"{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}"#;
    let project_request = format!("{{\"destination\":\"{}\"}}", project_root.to_string_lossy());
    let mut events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()];
    let append_text = |events: &mut Vec<Vec<u8>>, text: &[u8]| {
        events.extend(text.iter().map(|byte| vec![*byte]));
    };
    let append_project_draft = |events: &mut Vec<Vec<u8>>, cancel: bool| {
        events.push(b"\x10".to_vec());
        append_text(events, b"new-project");
        events.push(b"\r".to_vec());
        append_text(events, project_request.as_bytes());
        events.push(b"\x16".to_vec());
        if cancel {
            events.push(b"\x1b".to_vec());
        } else {
            events.push(b"\x1b[13;5u".to_vec());
            events.push(b"\x1b_Gi=2;OK\x1b\\".to_vec());
        }
    };
    append_project_draft(&mut events, true);
    append_project_draft(&mut events, false);

    let append_extrude_draft =
        |events: &mut Vec<Vec<u8>>, cancel: bool, preview_image_id: u8, commit_image_id: u8| {
            events.push(b"\x10".to_vec());
            append_text(events, b"extrude");
            events.push(b"\r".to_vec());
            append_text(events, request);
            events.push(b"\x16".to_vec());
            events.push(format!("\x1b_Gi={preview_image_id};OK\x1b\\").into_bytes());
            if cancel {
                events.push(b"\x1b".to_vec());
                events.push(format!("\x1b_Gi={commit_image_id};OK\x1b\\").into_bytes());
            } else {
                events.push(b"\x1b[13;5u".to_vec());
                events.push(format!("\x1b_Gi={commit_image_id};OK\x1b\\").into_bytes());
            }
        };
    append_extrude_draft(&mut events, true, 3, 4);
    append_extrude_draft(&mut events, false, 5, 6);
    events.push(b"\x1b[B".to_vec());
    events.push(b"\x1b_Gi=7;OK\x1b\\".to_vec());
    events.push(b"q".to_vec());
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        ..Default::default()
    };

    launch(&host, &launch_root, &mut terminal, official_environment())
        .expect("fresh project keyboard workflow succeeds");

    assert!(
        !launch_root.exists(),
        "the launch placeholder remains absent"
    );
    let bundle = Bundle::at(&project_root)
        .open_read_only()
        .expect("created project identity reads");
    assert_eq!(bundle.manifest.transaction_count, 1);
    assert_ne!(bundle.manifest.revision_hash, "empty-project");
    assert!(
        !serde_json::to_string(bundle.log.entries())
            .expect("log entries serialize")
            .contains("empty-project")
    );
    assert_eq!(
        host.current()
            .expect("created project is active")
            .revision_hash,
        bundle.revision_hash_hex()
    );
    assert!(project_root.join("brep/keyboard-extrude.brep").is_file());
    let entry = bundle
        .log
        .entries()
        .last()
        .expect("extrusion transaction is retained");
    let CanonicalIntent::Extrude(intent) = entry.intent.as_ref().expect("extrusion intent exists")
    else {
        panic!("fresh workflow retained a non-extrusion intent");
    };
    assert_eq!(intent.affected_semantic_ids, ["keyboard-extrude"]);
    assert_eq!(
        intent.deterministic_inputs.profile,
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]
    );
    assert_eq!(intent.deterministic_inputs.height, 3.0);
    assert_eq!(intent.mode, "additive");

    let scene = host
        .presentation_viewport_scene()
        .expect("committed extrusion scene reads");
    assert_eq!(scene.solids.len(), 1);
    let solid = &scene.solids[0];
    assert_eq!(solid.feature_id, "keyboard-extrude");
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

    let output = String::from_utf8_lossy(&terminal.writes);
    assert!(output.contains(
        r#""feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive""#
    ));
    for acknowledgement in [
        "[outline] Draft: new-project",
        "[dashed-outline] Preview: new-project",
        "[cancellation-glyph] Cancellation: command draft discarded",
        "Project created:",
        "transaction_count=0",
        "[outline] Draft: extrude",
        "[dashed-outline] Preview: extrude",
        "[selection-glyph] Commit: extrude",
        "keyboard-extrude",
        "selected feature keyboard-extrude",
        "[viewport-status] Viewport presented",
    ] {
        assert!(
            output.contains(acknowledgement),
            "missing acknowledgement: {acknowledgement}"
        );
    }
    assert!(
        output
            .matches("[cancellation-glyph] Cancellation: command draft discarded")
            .count()
            >= 2
    );
    assert!(
        output.contains("\"triangle_count\":") && !output.contains("\"triangle_count\":0"),
        "committed viewport evidence contains tessellated geometry"
    );

    let before_inspection = snapshot_tree(&project_root);
    let _ = Bundle::at(&project_root)
        .open_read_only()
        .expect("second read-only inspection succeeds");
    assert_eq!(snapshot_tree(&project_root), before_inspection);

    fs::remove_dir_all(workspace).expect("workflow workspace removes");
}

#[test]
#[ignore = "requires the pinned native OCCT worker"]
fn production_launch_creates_tapered_reinforcement_through_keyboard() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some()
                || std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() =>
        {
            panic!("keyboard tapered reinforcement requires OCCT worker: {error}");
        }
        Err(error) => {
            eprintln!("keyboard tapered reinforcement: OCCT worker unavailable: {error}");
            return;
        }
    };
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-keyboard-tapered-reinforcement-{}-{suffix}",
        std::process::id()
    ));
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("seed project persists");
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
    .expect("real seed extrusion commits");
    let before = host
        .identity(&root)
        .expect("seed identity reads before keyboard draft");
    let seeded_tree = snapshot_tree(&root);
    let request = json!({
        "feature_id": "tapered-reinforcement",
        "base_feature_id": "taper-seed",
        "angle": 0.05235987755982989,
        "pull_direction": [0, 0, 1]
    })
    .to_string();
    let mut events = vec![b"\x1b_Gi=1;OK\x1b\\".to_vec(), b"\x10".to_vec()];
    events.extend(b"draft".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(request.bytes().map(|byte| vec![byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=2;OK\x1b\\".to_vec(),
    ]);
    let loft_request = json!({
        "feature_id": "lofted-gusset",
        "profiles": [
            [[8, 8, 8], [16, 8, 8], [16, 16, 8], [8, 16, 8]],
            [[10, 10, 18], [14, 10, 18], [14, 14, 18], [10, 14, 18]]
        ],
        "is_solid": true,
        "ruled": false
    })
    .to_string();
    events.push(b"\x10".to_vec());
    events.extend(b"loft".iter().map(|byte| vec![*byte]));
    events.push(b"\r".to_vec());
    events.extend(loft_request.bytes().map(|byte| vec![byte]));
    events.extend([
        b"\x16".to_vec(),
        b"\x1b[13;5u".to_vec(),
        b"\x1b_Gi=3;OK\x1b\\".to_vec(),
    ]);
    events.reverse();
    let mut terminal = ScriptedTerminal {
        events,
        skip_probe_replay: true,
        selection_targets: BTreeSet::from([
            "tapered-reinforcement".to_string(),
            "lofted-gusset".to_string(),
        ]),
        selection_active: true,
        ..Default::default()
    };

    launch(&host, &root, &mut terminal, official_environment())
        .expect("keyboard draft workflow succeeds");
    let output = String::from_utf8_lossy(&terminal.writes);
    let acknowledged_draft_revision = viewport_revision_between(
        &output,
        "[selection-glyph] Commit: draft",
        "[outline] Draft: loft",
    );

    let final_identity = host
        .identity(&root)
        .expect("reinforcement identity reads after keyboard commits");
    assert_eq!(
        final_identity.transaction_count,
        before.transaction_count + 2
    );
    assert_ne!(final_identity.revision_hash, before.revision_hash);
    let final_bundle = Bundle::at(&root)
        .open_read_only()
        .expect("reinforcement bundle opens read-only");
    assert_ne!(snapshot_tree(&root), seeded_tree);
    let draft_entry = final_bundle
        .log
        .entries()
        .iter()
        .find(|entry| entry.feature_id == "tapered-reinforcement")
        .expect("draft transaction is retained");
    let CanonicalIntent::Draft(intent) = draft_entry.intent.as_ref().expect("draft intent exists")
    else {
        panic!("keyboard draft retained a non-draft intent");
    };
    assert_authenticated_brep(&root, draft_entry);
    assert_eq!(intent.base_feature_id, "taper-seed");
    assert_eq!(intent.angle, 0.05235987755982989);
    assert_eq!(intent.pull_direction, [0.0, 0.0, 1.0]);
    assert_eq!(intent.source_revision, before.revision_hash);
    let brep = root.join("brep/tapered-reinforcement.brep");
    assert!(brep.is_file(), "keyboard draft BREP is persisted");
    let loft_entry = final_bundle
        .log
        .entries()
        .last()
        .expect("loft transaction is retained");
    let CanonicalIntent::Loft(loft_intent) =
        loft_entry.intent.as_ref().expect("loft intent exists")
    else {
        panic!("keyboard loft retained a non-loft intent");
    };
    assert_authenticated_brep(&root, loft_entry);
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
        final_bundle
            .feature_brep_source_revision("tapered-reinforcement")
            .expect("draft source revision derives from the canonical log")
    );
    assert_eq!(loft_intent.source_revision, acknowledged_draft_revision);
    assert!(root.join("brep/lofted-gusset.brep").is_file());
    assert_eq!(
        terminal.selection_acknowledged_targets,
        BTreeSet::from([
            "tapered-reinforcement".to_string(),
            "lofted-gusset".to_string(),
        ])
    );
    assert!(!terminal.selection_ack_pending);

    let before_inspection = snapshot_tree(&root);
    let scene = Host::new()
        .read_only_viewport_scene(&root)
        .expect("reinforcement scene reads read-only");
    assert_eq!(snapshot_tree(&root), before_inspection);
    let tapered = scene_solid(&scene, "tapered-reinforcement");
    let lofted = scene_solid(&scene, "lofted-gusset");
    assert_closed_positive_mesh(tapered);
    assert_closed_positive_mesh(lofted);
    let tapered_bottom = section_bounds(tapered, 0.0).expect("tapered lower section exists");
    let tapered_top = section_bounds(tapered, 12.0).expect("tapered upper section exists");
    assert!(
        (tapered_bottom[1] - tapered_bottom[0] - (tapered_top[1] - tapered_top[0])).abs()
            > SECTION_TOLERANCE_MM
            || (tapered_bottom[3] - tapered_bottom[2] - (tapered_top[3] - tapered_top[2])).abs()
                > SECTION_TOLERANCE_MM,
        "drafted sections must differ along the pull direction: bottom={tapered_bottom:?} top={tapered_top:?}"
    );
    let lofted_lower = section_bounds(lofted, 8.0).expect("loft lower section exists");
    let lofted_upper = section_bounds(lofted, 18.0).expect("loft upper section exists");
    for (actual, expected) in [
        (lofted_lower, [8.0, 16.0, 8.0, 16.0]),
        (lofted_upper, [10.0, 14.0, 10.0, 14.0]),
    ] {
        for (value, target) in actual.into_iter().zip(expected) {
            assert!((value - target).abs() <= SECTION_TOLERANCE_MM);
        }
    }

    for acknowledgement in [
        "[outline] Draft: draft",
        "[dashed-outline] Preview: draft",
        "[selection-glyph] Commit: draft",
        "tapered-reinforcement",
        "[outline] Draft: loft",
        "[dashed-outline] Preview: loft",
        "[selection-glyph] Commit: loft",
        "lofted-gusset",
        "selected feature tapered-reinforcement",
        "selected feature lofted-gusset",
        "[viewport-status] Viewport presented",
    ] {
        assert!(
            output.contains(acknowledgement),
            "missing keyboard draft acknowledgement: {acknowledgement}"
        );
    }
    assert!(
        terminal.read_events.iter().all(|event| !matches!(
            decode_terminal_input(event),
            Some(TerminalInput::Pick { .. })
        )),
        "tapered reinforcement does not depend on pointer selection"
    );

    fs::remove_dir_all(root).expect("keyboard draft project removes");
}

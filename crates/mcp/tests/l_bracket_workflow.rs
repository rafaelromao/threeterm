use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_cli::dispatch::{DispatchError, dispatch_registered_command};
use threeterm_host::{Host, HostError};
use threeterm_mcp::server::{JsonRpcRequest, McpServer};
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::{
    BRACKET_INTENT_SCHEMA_VERSION, Bundle, CanonicalIntent, canonical_bracket_request_id,
};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, BRACKET_EDIT_COMMAND_ID, EXPORT_COMMAND_ID, LOAD_COMMAND_ID,
};
use threeterm_tui::TuiViewportSession;
use threeterm_tui::execute_domain_command;
use threeterm_viewport::{
    CapabilityState, FrameAcknowledgement, GhosttyRenderer, TerminalCapabilityVector, ViewportScene,
};

#[derive(Debug, Default)]
struct RecordingWriter {
    bytes: Vec<u8>,
}

impl std::io::Write for RecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn valid_capabilities() -> TerminalCapabilityVector {
    TerminalCapabilityVector {
        state: CapabilityState::Valid,
        direct_ghostty: true,
        kitty_rgb_zlib: true,
        kitty_acknowledgements: true,
        kitty_keyboard: true,
        sgr_mouse_cell: true,
        sgr_mouse_pixel: true,
        focus_reporting: true,
        alternate_screen: true,
        resize_events: true,
    }
}

fn root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-l-bracket-{label}-{suffix}"))
}

fn required_worker(test_name: &str) -> bool {
    match OcctWorker::locate() {
        Ok(_) => true,
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some()
                || std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() =>
        {
            panic!("{test_name}: OCCT worker is required: {error}")
        }
        Err(_) => {
            eprintln!("{test_name}: OCCT worker unavailable; skipping");
            false
        }
    }
}

fn bracket_request(root: &Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "bracket_id": "l-bracket",
        "length": 60.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0,
    })
}

fn mcp_call(server: &McpServer, wire_name: &str, arguments: Value) -> Value {
    let response = server.handle_request(&JsonRpcRequest {
        id: json!(wire_name),
        is_notification: false,
        method: "tools/call".to_string(),
        params: json!({
            "name": wire_name,
            "arguments": arguments,
        }),
    });
    assert!(
        response.error.is_none(),
        "MCP call failed: {:?}",
        response.error
    );
    response.result.expect("MCP call has a result")["structuredContent"].clone()
}

fn portable_bracket_response(value: &Value) -> Value {
    let mut portable = value.clone();
    let object = portable
        .as_object_mut()
        .expect("bracket response is an object");
    for field in ["request_id", "generation_id", "brep_path"] {
        object.remove(field);
    }
    if let Some(derived) = object
        .get_mut("derived_result")
        .and_then(Value::as_object_mut)
    {
        derived.remove("request_id");
    }
    portable
}

enum AdapterSession {
    Cli { root: PathBuf, host: Host },
    Mcp { root: PathBuf, server: McpServer },
    Tui { root: PathBuf, host: Host },
}

impl AdapterSession {
    fn cli(root: PathBuf) -> Self {
        Bundle::create(&root).expect("CLI bundle creates");
        Self::Cli {
            root,
            host: Host::new(),
        }
    }

    fn mcp(root: PathBuf) -> Self {
        Bundle::create(&root).expect("MCP bundle creates");
        Self::Mcp {
            root,
            server: McpServer::new(),
        }
    }

    fn tui(root: PathBuf) -> Self {
        Bundle::create(&root).expect("TUI bundle creates");
        Self::Tui {
            root,
            host: Host::new(),
        }
    }

    fn root(&self) -> &Path {
        match self {
            Self::Cli { root, .. } | Self::Mcp { root, .. } | Self::Tui { root, .. } => root,
        }
    }

    fn execute(
        &self,
        command: threeterm_protocol::schema::CommandId,
        wire_name: &str,
        request: Value,
    ) -> Value {
        match self {
            Self::Cli { host, .. } => match dispatch_registered_command(host, command, request) {
                Ok(response) => response,
                Err(DispatchError::Host(HostError::DraftInputConflict {
                    draft_id,
                    source_revision,
                    current_revision,
                    recovery,
                })) => draft_input_conflict_response(
                    "open",
                    draft_id,
                    source_revision,
                    current_revision,
                    recovery,
                ),
                Err(error) => panic!("CLI {wire_name} command fails: {error:?}"),
            },
            Self::Mcp { server, .. } => mcp_call(server, wire_name, request),
            Self::Tui { host, .. } => match execute_domain_command(host, command, request) {
                Ok(response) => response,
                Err(ExecutionError::Handler(HostError::DraftInputConflict {
                    draft_id,
                    source_revision,
                    current_revision,
                    recovery,
                })) => draft_input_conflict_response(
                    "open",
                    draft_id,
                    source_revision,
                    current_revision,
                    recovery,
                ),
                Err(error) => panic!("TUI {wire_name} command fails: {error:?}"),
            },
        }
    }

    fn create(&self, length: f64) -> Value {
        let mut request = bracket_request(self.root());
        request["length"] = json!(length);
        self.execute(BRACKET_COMMAND_ID, "threeterm.command.bracket/1", request)
    }

    fn load(&self) -> Value {
        self.execute(
            LOAD_COMMAND_ID,
            "threeterm.command.load/1",
            json!({"bundle_path": self.root().to_string_lossy()}),
        )
    }

    fn export(&self) -> Value {
        self.execute(
            EXPORT_COMMAND_ID,
            "threeterm.command.export/1",
            json!({
                "bundle_path": self.root().to_string_lossy(),
                "feature_id": "l-bracket",
                "formats": ["stl", "3mf", "step"],
                "output_dir": self.root().join("exports").to_string_lossy(),
                "tessellation_deflection": 0.5,
                "override_warnings": false,
                "accept_stale_geometry": false,
            }),
        )
    }

    fn edit(
        &self,
        phase: &str,
        draft_id: &str,
        length: f64,
        draft_sequence: Option<u64>,
        input_fingerprint: Option<&str>,
    ) -> Value {
        let mut request = json!({
            "phase": phase,
            "bundle_path": self.root().to_string_lossy(),
            "draft_id": draft_id,
            "bracket_id": "l-bracket",
            "length": length,
            "width": 30.0,
            "height": 40.0,
            "thickness": 3.0,
        });
        if let Some(sequence) = draft_sequence {
            request["draft_sequence"] = json!(sequence);
        }
        if let Some(fingerprint) = input_fingerprint {
            request["input_fingerprint"] = json!(fingerprint);
        }
        self.execute(
            BRACKET_EDIT_COMMAND_ID,
            "threeterm.command.bracket-edit/1",
            request,
        )
    }
}

fn draft_input_conflict_response(
    phase: &str,
    draft_id: String,
    source_revision: String,
    current_revision: String,
    recovery: &str,
) -> Value {
    json!({
        "status": "rejected",
        "phase": phase,
        "draft_id": draft_id,
        "diagnostic": {
            "kind": "draft_input_conflict",
            "draft_id": draft_id,
            "source_revision": source_revision,
            "current_revision": current_revision,
            "recovery": recovery,
        },
    })
}

struct EditedObservation {
    commit: Value,
    brep_sha256: String,
    feature_graph_hash: String,
    source_revision: String,
    preview_revision: String,
    current_revision: String,
    transaction_count: usize,
    terminal_log_digest: String,
    worker_fingerprint: Value,
    transaction_shape: Value,
    lifecycle: Value,
}

fn transaction_shape(root: &Path) -> Value {
    let bundle = Bundle::at(root).open().expect("edited bundle opens");
    Value::Array(
        bundle
            .log
            .entries()
            .iter()
            .map(|entry| {
                json!({
                    "log_index": entry.log_index,
                    "feature_id": entry.feature_id,
                    "kind": entry.kind,
                })
            })
            .collect(),
    )
}

fn viewport_shape(scene: &ViewportScene) -> Value {
    json!({
        "features": scene.features.iter().map(|feature| json!({
            "id": feature.id,
            "kind": feature.kind,
        })).collect::<Vec<_>>(),
        "solids": scene.solids.iter().map(|solid| json!({
            "feature_id": solid.feature_id,
            "triangles": solid.triangles.iter().map(|triangle| triangle.vertices).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "selected_id": scene.selected_id,
        "layer1_references": scene.layer1_references,
        "fit_relationships": scene.fit_relationships,
    })
}

/// Zero the volatile `FILE_NAME` timestamp OCCT embeds in STEP exports so
/// adapter comparisons assert geometry content rather than wall-clock time.
fn normalize_step_timestamp(bytes: &[u8]) -> Vec<u8> {
    const PREFIX: &[u8] = b"FILE_NAME('Open CASCADE Shape Model','";
    let Some(prefix_start) = bytes
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)
    else {
        return bytes.to_vec();
    };
    let timestamp_start = prefix_start + PREFIX.len();
    let Some(timestamp_len) = bytes[timestamp_start..]
        .windows(2)
        .position(|window| window == b"',")
    else {
        return bytes.to_vec();
    };
    let mut normalized = bytes.to_vec();
    normalized[timestamp_start..timestamp_start + timestamp_len].fill(b'0');
    normalized
}

fn portable_export_response(value: &Value, root: &Path) -> Value {
    let mut portable = value.clone();
    if let Some(object) = portable.as_object_mut() {
        object.remove("generation_id");
    }
    if let Some(artifacts) = portable["artifacts"].as_array_mut() {
        for artifact in artifacts {
            let path = artifact
                .as_str()
                .and_then(|path| Path::new(path).file_name())
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            *artifact = json!(path);
        }
    }
    if let Some(derived) = portable["derived_artifacts"].as_array_mut() {
        for artifact in derived {
            let object = artifact
                .as_object_mut()
                .expect("derived export artifact is an object");
            object.remove("request_id");
            if object["artifact_kind"] == "step"
                && let Some(name) = object["artifact_name"].as_str()
            {
                let bytes = fs::read(root.join("exports").join(name))
                    .expect("STEP export reads for portable comparison");
                object.insert(
                    "sha256".to_string(),
                    json!(sha256_hex(&normalize_step_timestamp(&bytes))),
                );
            }
        }
    }
    portable
}

fn portable_edit_response(value: &Value) -> Value {
    let mut portable = value.clone();
    if let Some(object) = portable.as_object_mut() {
        object.remove("request_id");
        object.remove("generation_id");
    }
    portable
}

fn normalized_edit_diagnostic(value: &Value) -> Value {
    json!({
        "command": "threeterm.command.bracket-edit/1",
        "status": value["status"],
        "phase": value["phase"],
        "draft_id": value["draft_id"],
        "kind": value["diagnostic"]["kind"],
        "source_revision": value["diagnostic"]["source_revision"],
        "current_revision": value["diagnostic"]["current_revision"],
    })
}

fn assert_bracket_intent(root: &Path, expected_length: f64) {
    let bundle = Bundle::at(root).open().expect("bracket bundle opens");
    let entry = bundle
        .log
        .entries()
        .iter()
        .find(|entry| {
            entry.feature_id == "l-bracket"
                && matches!(
                    entry.intent.as_ref(),
                    Some(CanonicalIntent::Bracket(intent))
                        if intent.deterministic_inputs.length == expected_length
                )
        })
        .expect("canonical bracket entry exists");
    let Some(CanonicalIntent::Bracket(intent)) = entry.intent.as_ref() else {
        panic!("canonical bracket entry carries a bracket intent");
    };
    assert_eq!(intent.schema_version, BRACKET_INTENT_SCHEMA_VERSION);
    assert_eq!(intent.command, "bracket");
    assert_eq!(intent.operation, "bracket");
    assert_eq!(
        intent.affected_semantic_ids,
        vec![
            "l-bracket".to_string(),
            "l-bracket-base".to_string(),
            "l-bracket-bend".to_string(),
            "l-bracket-finish".to_string(),
            "l-bracket-independent-base".to_string(),
            "l-bracket-independent-finish".to_string(),
        ]
    );
    assert_eq!(intent.deterministic_inputs.length, expected_length);
    assert_eq!(intent.deterministic_inputs.width, 30.0);
    assert_eq!(intent.deterministic_inputs.height, 40.0);
    assert_eq!(intent.deterministic_inputs.thickness, 3.0);
    assert_eq!(
        intent.request_id,
        canonical_bracket_request_id("l-bracket", expected_length, 30.0, 40.0, 3.0)
    );
    assert_eq!(intent.source_revision, bundle.revision_hash_hex());
    assert_eq!(intent.worker_requirements.worker_kind, "occt");
    assert!(!intent.worker_requirements.worker_schema_version.is_empty());
    assert!(
        !intent
            .worker_requirements
            .protocol_schema_version
            .is_empty()
    );
}

#[test]
fn l_bracket_adapter_parity() {
    if !required_worker("l_bracket_adapter_parity") {
        return;
    }
    let cli_root = root("create-cli");
    let mcp_root = root("create-mcp");
    let tui_root = root("create-tui");
    for path in [&cli_root, &mcp_root, &tui_root] {
        Bundle::create(path).expect("bundle creates");
    }

    let cli =
        dispatch_registered_command(&Host::new(), BRACKET_COMMAND_ID, bracket_request(&cli_root))
            .expect("CLI bracket command commits");
    let mcp = mcp_call(
        &McpServer::new(),
        "threeterm.command.bracket/1",
        bracket_request(&mcp_root),
    );
    let tui = execute_domain_command(&Host::new(), BRACKET_COMMAND_ID, bracket_request(&tui_root))
        .expect("TUI bracket command commits");

    for (path, response) in [(&cli_root, &cli), (&mcp_root, &mcp), (&tui_root, &tui)] {
        assert_eq!(response["status"], "ok");
        assert_eq!(response["operation"], "bracket");
        assert_eq!(response["feature_id"], "l-bracket");
        assert_eq!(response["artifact_kind"], "brep");
        assert_eq!(response["worker_fingerprint"]["worker_kind"], "occt");
        assert!(path.join("brep/l-bracket.brep").is_file());
        assert_bracket_intent(path, 60.0);
    }
    assert_eq!(cli["feature_graph_hash"], mcp["feature_graph_hash"]);
    assert_eq!(cli["feature_graph_hash"], tui["feature_graph_hash"]);
    assert_eq!(cli["brep_sha256"], mcp["brep_sha256"]);
    assert_eq!(cli["brep_sha256"], tui["brep_sha256"]);
    assert_eq!(
        portable_bracket_response(&cli),
        portable_bracket_response(&mcp),
        "CLI and MCP bracket domain results differ"
    );
    assert_eq!(
        portable_bracket_response(&cli),
        portable_bracket_response(&tui),
        "CLI and TUI bracket domain results differ"
    );

    for path in [cli_root, mcp_root, tui_root] {
        let _ = fs::remove_dir_all(path);
    }
}

#[test]
fn l_bracket_supervision() {
    if !required_worker("l_bracket_supervision") {
        return;
    }
    let root = root("supervision");
    Bundle::create(&root).expect("supervision bundle creates");
    let response =
        dispatch_registered_command(&Host::new(), BRACKET_COMMAND_ID, bracket_request(&root))
            .expect("supervised bracket command commits");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["operation"], "bracket");
    assert_eq!(response["feature_id"], "l-bracket");
    assert_eq!(response["artifact_kind"], "brep");
    assert_eq!(
        response["request_id"],
        response["derived_result"]["request_id"]
    );
    assert_eq!(response["derived_result"]["operation"], "bracket");
    assert_eq!(response["derived_result"]["feature_id"], "l-bracket");
    assert_eq!(response["worker_fingerprint"]["worker_kind"], "occt");
    assert_eq!(
        response["derived_result"]["worker_fingerprint"],
        response["worker_fingerprint"]
    );
    assert_eq!(
        response["derived_result"]["byte_count"],
        response["brep_bytes"]
    );
    assert_eq!(
        response["derived_result"]["sha256"],
        response["brep_sha256"]
    );
    assert_eq!(
        response["derived_result"]["source_revision_id"],
        response["source_snapshot"]["revision_hash"]
    );
    assert_bracket_intent(&root, 60.0);
    let bundle = Bundle::at(&root)
        .open()
        .expect("supervision bundle reloads");
    for (feature_id, kind) in [
        (
            "l-bracket",
            "bracket:length=60.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
        ),
        ("l-bracket-plate-vertical", "plate-vertical"),
        ("l-bracket-plate-horizontal", "plate-horizontal"),
    ] {
        assert!(bundle.graph.contains_feature(feature_id));
        assert!(
            bundle
                .log
                .entries()
                .iter()
                .any(|entry| entry.feature_id == feature_id && entry.kind == kind)
        );
    }
    assert!(!root.join(".derived").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn l_bracket_canonical_intent() {
    if !required_worker("l_bracket_canonical_intent") {
        return;
    }
    let mut sessions = vec![
        AdapterSession::cli(root("edit-cli")),
        AdapterSession::mcp(root("edit-mcp")),
        AdapterSession::tui(root("edit-tui")),
    ];
    let mut observations = Vec::new();

    for session in &mut sessions {
        let created = session.create(60.0);
        let manifest_before_discard =
            fs::read(session.root().join("manifest.json")).expect("manifest reads before discard");
        let transactions_before_discard = fs::read(session.root().join("transactions.log"))
            .expect("transactions read before discard");
        let brep_before_discard = fs::read(session.root().join("brep/l-bracket.brep"))
            .expect("BREP reads before discard");
        let opened_discard = session.edit("open", "discard-edit", 61.0, None, None);
        let discarded = session.edit("discard", "discard-edit", 61.0, None, None);
        assert_eq!(opened_discard["status"], "ok");
        assert_eq!(discarded["status"], "ok");
        assert_eq!(discarded["phase"], "discard");
        assert_eq!(
            fs::read(session.root().join("manifest.json")).expect("manifest reads after discard"),
            manifest_before_discard
        );
        assert_eq!(
            fs::read(session.root().join("transactions.log"))
                .expect("transactions read after discard"),
            transactions_before_discard
        );
        assert_eq!(
            fs::read(session.root().join("brep/l-bracket.brep")).expect("BREP reads after discard"),
            brep_before_discard
        );

        let opened = session.edit("open", "commit-edit", 65.0, None, None);
        let source_revision = opened["source_revision"]
            .as_str()
            .expect("open returns source revision")
            .to_string();
        let input_fingerprint = opened["input_fingerprint"]
            .as_str()
            .expect("open returns input fingerprint");
        assert_eq!(opened["phase"], "open");
        assert_eq!(opened["draft_sequence"], 0);

        let updated = session.edit(
            "update",
            "commit-edit",
            65.0,
            Some(0),
            Some(input_fingerprint),
        );
        assert_eq!(updated["phase"], "update");
        assert_eq!(updated["draft_sequence"], 1);

        let preview = session.edit("preview", "commit-edit", 65.0, None, None);
        assert_eq!(preview["phase"], "preview");
        assert_eq!(preview["source_revision"], source_revision);
        assert_ne!(preview["preview_revision"], preview["source_revision"]);
        let preview_revision = preview["preview_revision"]
            .as_str()
            .expect("preview returns preview revision")
            .to_string();

        let commit = session.edit("commit", "commit-edit", 65.0, None, None);
        assert_eq!(commit["status"], "ok");
        assert_eq!(commit["phase"], "commit");
        assert_eq!(commit["source_revision"], source_revision);
        assert_ne!(commit["current_revision"], commit["source_revision"]);
        let current_revision = commit["current_revision"]
            .as_str()
            .expect("commit returns current revision")
            .to_string();
        assert_ne!(preview_revision, current_revision);
        let edited_brep = fs::read(session.root().join("brep/l-bracket.brep"))
            .expect("edited BREP reads for identity");
        assert_ne!(sha256_hex(&edited_brep), created["brep_sha256"]);
        let loaded = Bundle::at(session.root())
            .open()
            .expect("edited bundle reloads");
        assert_bracket_intent(session.root(), 60.0);
        assert_bracket_intent(session.root(), 65.0);
        assert_eq!(loaded.log.len(), 6);
        let history = loaded.history.active_snapshot();
        assert_eq!(history.features["l-bracket-base"].input_value, 65.0);
        assert_eq!(history.features["l-bracket-bend"].input_value, 30.0);
        assert_eq!(history.features["l-bracket-finish"].input_value, 40.0);
        let bracket_kinds = loaded
            .log
            .entries()
            .iter()
            .filter(|entry| entry.feature_id == "l-bracket")
            .map(|entry| entry.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            bracket_kinds,
            [
                "bracket:length=60.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
                "bracket:length=65.00000000000000000;width=30.00000000000000000;height=40.00000000000000000;thickness=3.00000000000000000",
            ]
        );
        let lifecycle = json!({
            "discard_open": portable_edit_response(&opened_discard),
            "discard": portable_edit_response(&discarded),
            "open": portable_edit_response(&opened),
            "update": portable_edit_response(&updated),
            "preview": portable_edit_response(&preview),
            "commit": portable_edit_response(&commit),
        });
        observations.push(EditedObservation {
            commit,
            brep_sha256: sha256_hex(&edited_brep),
            feature_graph_hash: loaded.feature_graph_hash_hex().to_string(),
            source_revision,
            preview_revision,
            current_revision,
            transaction_count: loaded.log.len(),
            terminal_log_digest: loaded.manifest.terminal_log_digest.clone(),
            worker_fingerprint: serde_json::to_value(&loaded.manifest.occt_worker)
                .expect("worker fingerprint serializes"),
            transaction_shape: transaction_shape(session.root()),
            lifecycle,
        });
        assert_eq!(created["operation"], "bracket");
    }

    assert_eq!(observations.len(), 3);
    assert!(observations.iter().all(|observation| {
        observation.brep_sha256 == observations[0].brep_sha256
            && observation.feature_graph_hash == observations[0].feature_graph_hash
            && observation.source_revision == observations[0].source_revision
            && observation.preview_revision == observations[0].preview_revision
            && observation.current_revision == observations[0].current_revision
            && observation.transaction_count == observations[0].transaction_count
            && observation.terminal_log_digest == observations[0].terminal_log_digest
            && observation.worker_fingerprint == observations[0].worker_fingerprint
            && observation.transaction_shape == observations[0].transaction_shape
            && observation.lifecycle == observations[0].lifecycle
            && observation.commit["phase"] == "commit"
    }));

    for session in sessions {
        let _ = fs::remove_dir_all(session.root());
    }
}

#[test]
fn l_bracket_artifact_discard_replay() {
    if !required_worker("l_bracket_artifact_discard_replay") {
        return;
    }
    let mut sessions = vec![
        AdapterSession::cli(root("reload-cli")),
        AdapterSession::mcp(root("reload-mcp")),
        AdapterSession::tui(root("reload-tui")),
    ];
    let mut observations = Vec::new();

    for session in &mut sessions {
        session.create(60.0);
        let opened = session.edit("open", "reload-edit", 65.0, None, None);
        let fingerprint = opened["input_fingerprint"]
            .as_str()
            .expect("reload edit open returns fingerprint");
        session.edit("update", "reload-edit", 65.0, Some(0), Some(fingerprint));
        let committed = session.edit("commit", "reload-edit", 65.0, None, None);
        assert_eq!(committed["status"], "ok");
        let before = Bundle::at(session.root())
            .open()
            .expect("bundle opens before reload");
        let expected_revision = before.revision_hash_hex().to_string();
        let expected_graph = before.feature_graph_hash_hex().to_string();
        let expected_brep = sha256_hex(
            &fs::read(session.root().join("brep/l-bracket.brep"))
                .expect("committed BREP reads before derived deletion"),
        );
        let expected_terminal_log_digest = before.manifest.terminal_log_digest.clone();
        let expected_worker_fingerprint = serde_json::to_value(&before.manifest.occt_worker)
            .expect("reload worker fingerprint serializes");
        let expected_transaction_count = before.log.len();
        let transient_export_stage = session.root().join("exports/.threeterm-export-fixture");
        fs::create_dir_all(&transient_export_stage).expect("transient export stage creates");
        fs::write(transient_export_stage.join("partial.brep"), b"discardable")
            .expect("transient export artifact writes");
        let manifest_before = fs::read(session.root().join("manifest.json"))
            .expect("manifest reads before derived deletion");
        let transactions_before = fs::read(session.root().join("transactions.log"))
            .expect("transactions read before derived deletion");

        fs::remove_file(session.root().join("brep/l-bracket.brep"))
            .expect("derived bracket BREP removes");
        for disposable in ["cache", ".derived"] {
            let path = session.root().join(disposable);
            if path.exists() {
                fs::remove_dir_all(path).expect("disposable derived directory removes");
            }
        }
        if let Ok(entries) = fs::read_dir(session.root().join("exports")) {
            for entry in entries {
                let entry = entry.expect("export staging entry reads");
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".threeterm-export-")
                {
                    fs::remove_dir_all(entry.path()).expect("transient export stage removes");
                }
            }
        }
        assert!(!transient_export_stage.exists());

        let loaded = session.load();
        assert_eq!(
            loaded["schema_version"],
            "threeterm.command.load.response/2"
        );
        assert_eq!(loaded["feature_graph_hash"], expected_graph);
        assert_eq!(loaded["revision_hash"], expected_revision);
        assert!(loaded["recovered_from_previous"].is_boolean());
        assert_eq!(
            fs::read(session.root().join("manifest.json")).expect("manifest reads after reload"),
            manifest_before
        );
        assert_eq!(
            fs::read(session.root().join("transactions.log"))
                .expect("transactions read after reload"),
            transactions_before
        );
        assert!(session.root().join("brep/l-bracket.brep").is_file());
        assert_eq!(
            sha256_hex(
                &fs::read(session.root().join("brep/l-bracket.brep"))
                    .expect("recomputed BREP reads after reload")
            ),
            expected_brep
        );
        let after = Bundle::at(session.root())
            .open()
            .expect("bundle opens after reload");
        assert_eq!(after.log.len(), expected_transaction_count);
        assert_eq!(
            after.manifest.terminal_log_digest,
            expected_terminal_log_digest
        );
        assert_eq!(
            serde_json::to_value(&after.manifest.occt_worker)
                .expect("reloaded worker fingerprint serializes"),
            expected_worker_fingerprint
        );

        let loaded_again = session.load();
        assert_eq!(
            loaded_again["schema_version"],
            "threeterm.command.load.response/2"
        );
        assert_eq!(loaded_again["feature_graph_hash"], expected_graph);
        assert_eq!(loaded_again["revision_hash"], expected_revision);
        assert_eq!(
            sha256_hex(
                &fs::read(session.root().join("brep/l-bracket.brep"))
                    .expect("recomputed BREP reads after second reload")
            ),
            expected_brep
        );

        let host = Host::new();
        host.load(session.root())
            .expect("host reloads committed bracket");
        let scene = host
            .presentation_viewport_scene()
            .expect("production viewport scene builds");
        let solid = scene
            .solids
            .iter()
            .find(|solid| solid.feature_id == "l-bracket")
            .expect("viewport contains the bracket solid");
        assert!(!solid.triangles.is_empty());

        let mut renderer = GhosttyRenderer::new(RecordingWriter::default());
        renderer
            .admit(&valid_capabilities())
            .expect("viewport renderer admits capabilities");
        let mut viewport = TuiViewportSession::from_host(&host, 64, 48, renderer)
            .expect("TUI viewport session builds from the reloaded host");
        let submission = viewport
            .process_terminal_input(b"\x1b[C")
            .expect("viewport navigation submits a frame");
        let identity = submission
            .submission
            .started
            .expect("viewport frame starts");
        let visible = viewport
            .acknowledge(FrameAcknowledgement::from(&identity))
            .expect("viewport acknowledgement succeeds")
            .visible
            .expect("acknowledged frame is visible");
        assert!(!visible.rgb.is_empty());
        assert_ne!(viewport.camera().yaw_degrees, 0);

        let exported = session.export();
        assert_eq!(exported["status"], "ok");
        assert_eq!(exported["feature_id"], "l-bracket");
        assert_eq!(exported["source_revision_id"], expected_revision);
        let derived = exported["derived_artifacts"]
            .as_array()
            .expect("export returns derived artifacts");
        assert_eq!(derived.len(), 3);
        let mut file_shapes = Vec::new();
        for format in ["stl", "3mf", "step"] {
            let path = session
                .root()
                .join("exports")
                .join(format!("l-bracket.{format}"));
            let bytes = fs::read(&path).expect("exported artifact reads");
            assert!(!bytes.is_empty());
            match format {
                "stl" => assert!(bytes.starts_with(b"solid")),
                "3mf" => assert!(bytes.starts_with(b"PK\x03\x04")),
                "step" => assert!(bytes.starts_with(b"ISO-10303-21")),
                _ => unreachable!(),
            }
            let metadata = derived
                .iter()
                .find(|artifact| artifact["artifact_kind"] == format)
                .expect("export metadata names every format");
            assert!(
                metadata["request_id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
            );
            assert_eq!(metadata["source_revision_id"], expected_revision);
            assert_eq!(metadata["artifact_name"], format!("l-bracket.{format}"));
            assert_eq!(metadata["byte_count"], bytes.len());
            assert_eq!(metadata["sha256"], sha256_hex(&bytes));
            assert_eq!(metadata["operation"], "export");
            assert_eq!(metadata["feature_id"], "l-bracket");
            let comparable_bytes = if format == "step" {
                normalize_step_timestamp(&bytes)
            } else {
                bytes.clone()
            };
            file_shapes.push(json!({
                "format": format,
                "bytes": comparable_bytes.len(),
                "sha256": sha256_hex(&comparable_bytes),
            }));
        }
        let reloaded_brep =
            fs::read(session.root().join("brep/l-bracket.brep")).expect("reloaded BREP reads");
        observations.push(json!({
            "brep_sha256": sha256_hex(&reloaded_brep),
            "transaction_count": expected_transaction_count,
            "terminal_log_digest": expected_terminal_log_digest,
            "worker_fingerprint": expected_worker_fingerprint,
            "viewport": viewport_shape(&scene),
            "export": portable_export_response(&exported, session.root()),
            "files": file_shapes,
        }));
    }

    assert_eq!(observations.len(), 3);
    assert!(observations.iter().all(|observation| {
        observation["brep_sha256"] == observations[0]["brep_sha256"]
            && observation["transaction_count"] == observations[0]["transaction_count"]
            && observation["terminal_log_digest"] == observations[0]["terminal_log_digest"]
            && observation["worker_fingerprint"] == observations[0]["worker_fingerprint"]
            && observation["viewport"] == observations[0]["viewport"]
            && observation["export"] == observations[0]["export"]
            && observation["files"] == observations[0]["files"]
    }));

    for session in sessions {
        let _ = fs::remove_dir_all(session.root());
    }
}

#[test]
fn l_bracket_failure_atomicity() {
    if !required_worker("l_bracket_failure_atomicity") {
        return;
    }
    let mut sessions = vec![
        AdapterSession::cli(root("diagnostic-cli")),
        AdapterSession::mcp(root("diagnostic-mcp")),
        AdapterSession::tui(root("diagnostic-tui")),
    ];
    let mut diagnostics = Vec::new();

    for session in &mut sessions {
        session.create(60.0);
        session.edit("open", "conflict-edit", 65.0, None, None);
        let manifest_before = fs::read(session.root().join("manifest.json"))
            .expect("diagnostic manifest reads before conflict");
        let transactions_before = fs::read(session.root().join("transactions.log"))
            .expect("diagnostic transactions read before conflict");
        let brep_before = fs::read(session.root().join("brep/l-bracket.brep"))
            .expect("diagnostic BREP reads before conflict");
        let conflict = session.edit("open", "conflict-edit", 66.0, None, None);
        assert_eq!(conflict["status"], "rejected");
        assert_eq!(conflict["diagnostic"]["kind"], "draft_input_conflict");
        assert_eq!(conflict["diagnostic"]["draft_id"], "conflict-edit");
        assert!(conflict["diagnostic"]["source_revision"].is_string());
        assert!(conflict["diagnostic"]["current_revision"].is_string());
        assert_eq!(
            fs::read(session.root().join("manifest.json"))
                .expect("diagnostic manifest reads after conflict"),
            manifest_before
        );
        assert_eq!(
            fs::read(session.root().join("transactions.log"))
                .expect("diagnostic transactions read after conflict"),
            transactions_before
        );
        assert_eq!(
            fs::read(session.root().join("brep/l-bracket.brep"))
                .expect("diagnostic BREP reads after conflict"),
            brep_before
        );
        diagnostics.push(normalized_edit_diagnostic(&conflict));
    }

    assert_eq!(diagnostics.len(), 3);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic == &diagnostics[0])
    );
    for session in sessions {
        let _ = fs::remove_dir_all(session.root());
    }
}

#[test]
fn mcp_boolean_pattern_cancellation_preserves_canonical_state_and_progress_identity() {
    use std::os::unix::fs::PermissionsExt;

    let root = root("cancellation");
    Bundle::create(&root).expect("cancellation bundle creates");
    let worker_path = root.join("worker.sh");
    let worker_request_id_path = root.join("worker-request-id");
    fs::write(
        &worker_path,
        (r##"#!/bin/sh
printf '%s\n' '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"occt"}'
read request
rid=$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
printf '%s' "$rid" > "__WORKER_REQUEST_ID__"
printf '%s\n' '{"kind":"progress","schema_version":"threeterm.protocol/1","request_id":"'$rid'","stage":"boolean_pattern:1/324","percent":1}'
read cancel
printf '%s\n' '{"kind":"cancelled","schema_version":"threeterm.protocol/1","request_id":"'$rid'","reason":"cancelled by client"}'
        "##)
        .replace(
            "__WORKER_REQUEST_ID__",
            &worker_request_id_path.to_string_lossy(),
        ),
    )
    .expect("cancellation worker writes");
    let mut permissions = fs::metadata(&worker_path)
        .expect("cancellation worker metadata reads")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&worker_path, permissions).expect("cancellation worker is executable");

    let loaded = Bundle::at(&root).open().expect("cancellation bundle opens");
    Bundle::at(&root)
        .append_new_feature_with_brep_if_revision_and_provenance(
            "base",
            "brep",
            loaded.revision_hash_hex(),
            "seed-base",
            "cancellation fixture",
            b"seed-base-brep",
        )
        .expect("cancellation base BREP publishes");
    let manifest_before =
        fs::read(root.join("manifest.json")).expect("cancellation manifest reads");
    let transactions_before =
        fs::read(root.join("transactions.log")).expect("cancellation transactions read");
    let call = json!({
        "jsonrpc": "2.0",
        "id": "cancel-1",
        "method": "tools/call",
        "params": {
            "name": "threeterm.command.boolean-pattern/1",
            "_meta": {"progressToken": "progress-1"},
            "arguments": {
                "bundle_path": root.to_string_lossy(),
                "feature_id": "pattern",
                "base_feature_id": "base",
                "origin": [0.0, 0.0, 0.0],
                "spacing": [1.0, 1.0],
                "columns": 18,
                "rows": 18,
                "diameter": 1.0
            }
        }
    });
    let cancel = json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": "cancel-1", "reason": "stop"}
    });
    let mut input = serde_json::to_vec(&call).expect("cancellation call serializes");
    input.push(b'\n');
    input.extend(serde_json::to_vec(&cancel).expect("cancellation notification serializes"));
    input.push(b'\n');
    let mut output = Vec::new();
    McpServer::new()
        .with_boolean_pattern_worker(
            OcctWorker::with_binary_path(worker_path).with_expected_worker_id("occt"),
        )
        .run(&mut input.as_slice(), &mut output)
        .expect("MCP cancellation run succeeds");
    let responses: Vec<Value> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("cancellation output is JSON"))
        .collect();
    let progress: Vec<_> = responses
        .iter()
        .filter(|response| response["method"] == "notifications/progress")
        .collect();
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0]["params"]["progressToken"], "progress-1");
    assert_eq!(progress[0]["params"]["progress"], 1);
    let terminal = responses
        .iter()
        .find(|response| response["id"] == "cancel-1")
        .expect("cancellation returns a terminal response");
    assert_eq!(terminal["result"]["isError"], true);
    assert_eq!(terminal["id"], "cancel-1");
    assert_eq!(
        terminal["result"]["structuredContent"]["code"],
        "worker_failure"
    );
    let diagnostic_arg = terminal["result"]["structuredContent"]["arg"]
        .as_str()
        .expect("cancellation diagnostic arg is text");
    let diagnostic: Value =
        serde_json::from_str(diagnostic_arg).expect("cancellation diagnostic is JSON");
    assert_eq!(diagnostic["kind"], "worker_terminated");
    let worker_request_id =
        fs::read_to_string(&worker_request_id_path).expect("worker request identity reads");
    assert_eq!(diagnostic["request_id"], worker_request_id);
    assert_eq!(diagnostic["stage"], "cancelled");
    assert_eq!(
        diagnostic["last_progress"]["stage"],
        "boolean_pattern:1/324"
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("cancellation manifest reads after cancel"),
        manifest_before
    );
    assert_eq!(
        fs::read(root.join("transactions.log"))
            .expect("cancellation transactions read after cancel"),
        transactions_before
    );
    assert!(!root.join("brep/pattern.brep").exists());
    assert!(root.join("brep/base.brep").is_file());
    Bundle::at(&root)
        .open()
        .expect("canonical recovery state remains readable after cancellation");
    let _ = fs::remove_dir_all(root);
}

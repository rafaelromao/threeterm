use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::bracket_oracle::{
    assert_bracket_mesh, assert_complete_intents, assert_reinforcement_intents, mesh_number,
    recipe_steps, selected_edge_from_recipe,
};
use threeterm_host::stl_integrity::{self, StlFormat};
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::{Bundle, CanonicalIntent, EXTRUDE_INTENT_SCHEMA_VERSION};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, EXPORT_COMMAND_ID, EXTRUDE_COMMAND_ID, IDENTITY_COMMAND_ID,
    LOAD_COMMAND_ID, NEW_PROJECT_COMMAND_ID, SAVE_COMMAND_ID, VALIDATE_COMMAND_ID, find,
    find_by_name, iter,
};
use threeterm_protocol::schema_validator::validate;

const PINNED_MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const NEW_PROJECT_TOOL: &str = "threeterm.command.new-project/1";
const EXTRUDE_TOOL: &str = "threeterm.command.extrude/2";
const COMPLETE_RECIPE: &str = include_str!("../../host/tests/data/bracket_complete_recipe.v1.json");

type StreamLine = Result<Vec<u8>, String>;

#[derive(Debug)]
struct McpEvidence {
    protocol: Vec<Value>,
    protocol_errors: Vec<Value>,
    server_diagnostics: String,
    domain_errors: Vec<Value>,
}

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<StreamLine>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<Result<Vec<u8>, String>>>,
    protocol: Vec<Value>,
    pending: BTreeMap<String, Value>,
    protocol_errors: Vec<Value>,
    domain_errors: Vec<Value>,
}

impl McpProcess {
    fn spawn() -> Self {
        let mut child = Command::new(threeterm_mcp_binary())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("production MCP process starts");

        let stdout_pipe = child
            .stdout
            .take()
            .expect("production MCP stdout is captured");
        let stderr_pipe = child
            .stderr
            .take()
            .expect("production MCP stderr is captured");
        let (stdout_sender, stdout) = mpsc::channel();
        let stdout_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stdout_pipe);
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => return,
                    Ok(_) => {
                        if stdout_sender.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = stdout_sender.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        });
        let stderr_thread = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr_pipe
                .take(16 * 1024 * 1024)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            Ok(bytes)
        });
        let stdin = child
            .stdin
            .take()
            .expect("production MCP stdin is captured");

        Self {
            child,
            stdin: Some(stdin),
            stdout,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            protocol: Vec::new(),
            pending: BTreeMap::new(),
            protocol_errors: Vec::new(),
            domain_errors: Vec::new(),
        }
    }

    fn send(&mut self, request: &Value) {
        let mut bytes = serde_json::to_vec(request).expect("MCP request serializes");
        bytes.push(b'\n');
        let stdin = self.stdin.as_mut().expect("MCP stdin remains open");
        stdin.write_all(&bytes).expect("MCP request writes");
        stdin.flush().expect("MCP request flushes");
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    fn request(&mut self, id: &str, method: &str, params: Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.response(id)
    }

    fn call_tool(&mut self, id: &str, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    fn response(&mut self, id: &str) -> Value {
        let expected_id = Value::String(id.to_string());
        let expected_key = request_key(&expected_id);
        if let Some(response) = self.pending.remove(&expected_key) {
            return response;
        }

        loop {
            let line = self
                .stdout
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|error| {
                    panic!(
                        "timed out waiting for MCP response id {id:?}: {error}; evidence={:?}",
                        self.protocol
                    )
                })
                .unwrap_or_else(|error| panic!("MCP stdout read failed: {error}"));
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let value: Value = serde_json::from_slice(&line).unwrap_or_else(|error| {
                panic!(
                    "MCP stdout is not a JSON-RPC envelope: {error}; raw={:?}; evidence={:?}",
                    String::from_utf8_lossy(&line),
                    self.protocol
                )
            });
            self.protocol.push(value.clone());
            assert_eq!(
                value["jsonrpc"], "2.0",
                "MCP protocol traffic has the wrong version: {value}"
            );

            let Some(response_id) = value.get("id") else {
                continue;
            };
            let response_key = request_key(response_id);
            if response_key == expected_key {
                if value.get("error").is_some_and(Value::is_object) {
                    self.protocol_errors.push(value.clone());
                }
                if value["result"]["isError"] == true {
                    self.domain_errors.push(value.clone());
                }
                return value;
            }
            self.pending.insert(response_key, value);
        }
    }

    fn finish(mut self) -> McpEvidence {
        drop(self.stdin.take());
        let status = wait_for_exit(&mut self.child, Duration::from_secs(10));
        let stdout_reader = self
            .stdout_thread
            .take()
            .expect("MCP stdout reader remains owned");
        assert!(
            join_reader(stdout_reader, Duration::from_secs(1)).is_some(),
            "MCP stdout reader did not stop within the cleanup deadline"
        );

        while let Ok(line) = self.stdout.try_recv() {
            let line = line.unwrap_or_else(|error| panic!("MCP stdout read failed: {error}"));
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let value: Value = serde_json::from_slice(&line).expect("MCP trailing stdout is JSON");
            assert_eq!(value["jsonrpc"], "2.0");
            self.protocol.push(value);
        }

        let stderr = join_reader(
            self.stderr_thread
                .take()
                .expect("MCP stderr reader remains owned"),
            Duration::from_secs(1),
        )
        .expect("MCP stderr reader did not stop within the cleanup deadline")
        .unwrap_or_else(|error| panic!("MCP stderr read failed: {error}"));
        assert!(
            status.success(),
            "production MCP exits unsuccessfully: status={status:?}, server_diagnostics={}",
            String::from_utf8_lossy(&stderr)
        );

        McpEvidence {
            protocol: std::mem::take(&mut self.protocol),
            protocol_errors: std::mem::take(&mut self.protocol_errors),
            server_diagnostics: String::from_utf8_lossy(&stderr).into_owned(),
            domain_errors: std::mem::take(&mut self.domain_errors),
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        drop(self.stdin.take());
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        }
        if let Some(reader) = self.stdout_thread.take() {
            let _ = join_reader(reader, Duration::from_secs(1));
        }
        if let Some(reader) = self.stderr_thread.take() {
            let _ = join_reader(reader, Duration::from_secs(1));
        }
    }
}

fn join_reader<T>(reader: JoinHandle<T>, timeout: Duration) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while !reader.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if reader.is_finished() {
        Some(reader.join().expect("MCP stream reader joins"))
    } else {
        None
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let kill_deadline = Instant::now() + Duration::from_secs(1);
                while Instant::now() < kill_deadline {
                    match child.try_wait() {
                        Ok(Some(status)) => return status,
                        Ok(None) => thread::sleep(Duration::from_millis(10)),
                        Err(error) => panic!("MCP process wait after termination failed: {error}"),
                    }
                }
                panic!("MCP process did not exit after forced termination");
            }
            Err(error) => panic!("MCP process wait failed: {error}"),
        }
    }
}

fn request_key(id: &Value) -> String {
    serde_json::to_string(id).expect("JSON-RPC id serializes")
}

fn threeterm_mcp_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_threeterm_mcp") {
        return PathBuf::from(path);
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("debug/threeterm-mcp")
}

fn fresh_root() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-mcp-production-lifecycle-{}-{suffix}",
        std::process::id()
    ))
}

fn require_native_worker(test_name: &str) {
    match OcctWorker::locate() {
        Ok(worker) => {
            drop(worker);
        }
        Err(error) => panic!("{test_name}: native OCCT worker is required: {error}"),
    }
}

fn assert_protocol_success(response: &Value, id: &str) -> Value {
    assert_eq!(
        response["id"], id,
        "MCP response id is not correlated: {response}"
    );
    assert!(
        response.get("error").is_none() || response["error"].is_null(),
        "MCP request returned a protocol error: {response}"
    );
    response["result"].clone()
}

fn structured_tool_success(response: &Value, id: &str) -> Value {
    let result = assert_protocol_success(response, id);
    assert_eq!(result["isError"], false, "MCP tool failed: {response}");
    let structured = result["structuredContent"].clone();
    let text = result["content"][0]["text"]
        .as_str()
        .expect("MCP success includes text content");
    assert_eq!(
        serde_json::from_str::<Value>(text).expect("MCP text content is JSON"),
        structured,
        "MCP text and structured content differ"
    );
    structured
}

fn assert_correlated_structured_ok(protocol: &[&Value], call_id: &str, command_name: &str) {
    let response = protocol
        .iter()
        .find(|value| value["id"] == call_id && value.get("result").is_some())
        .unwrap_or_else(|| {
            panic!("journey evidence lacks a result for {command_name} call {call_id}")
        });
    let result = &response["result"];
    assert_eq!(
        result["isError"], false,
        "journey evidence reports failure for {command_name} call {call_id}: {response}"
    );
    let structured = result["structuredContent"].clone();
    assert!(
        structured.is_object(),
        "journey evidence lacks structuredContent for {command_name} call {call_id}: {response}"
    );
    let text = result["content"][0]["text"].as_str().unwrap_or_else(|| {
        panic!("journey evidence lacks text for {command_name} call {call_id}: {response}")
    });
    assert_eq!(
        serde_json::from_str::<Value>(text).expect("journey evidence text is JSON"),
        structured,
        "journey evidence text and structured content differ for {command_name} call {call_id}"
    );
}

fn advertised_tools(client: &mut McpProcess) -> Vec<Value> {
    let mut cursor: Option<String> = None;
    let mut pages = Vec::new();
    let mut seen_cursors = HashSet::new();
    loop {
        let mut params = serde_json::Map::new();
        if let Some(cursor) = &cursor {
            params.insert("cursor".to_string(), Value::String(cursor.clone()));
        }
        let response = client.request(
            if cursor.is_some() {
                "tools-list-next"
            } else {
                "tools-list"
            },
            "tools/list",
            Value::Object(params),
        );
        let result = assert_protocol_success(
            &response,
            if cursor.is_some() {
                "tools-list-next"
            } else {
                "tools-list"
            },
        );
        pages.extend(
            result["tools"]
                .as_array()
                .expect("tools/list returns a tools array")
                .iter()
                .cloned(),
        );
        cursor = result["nextCursor"].as_str().map(str::to_string);
        let Some(next) = &cursor else { break };
        assert!(
            seen_cursors.insert(next.clone()),
            "tools/list cursor repeats"
        );
    }
    pages
}

fn domain_diagnostic_schema() -> Value {
    json!({
        "type": "object",
        "required": ["code", "arg", "schema_version"],
        "properties": {
            "code": {"type": "string", "minLength": 1},
            "arg": {"type": "string", "minLength": 1},
            "schema_version": {"const": "threeterm.protocol/1"},
            "affected_ids": {
                "type": "array",
                "uniqueItems": true,
                "items": {"type": "string", "minLength": 1}
            },
            "source": {"type": "string", "minLength": 1},
            "detail": {"type": "string", "minLength": 1},
            "recovery": {"type": "string", "minLength": 1}
        },
        "additionalProperties": false
    })
}

fn assert_fresh_project(root: &Path, generation_id: &str) -> String {
    let bundle = Bundle::at(root).open().expect("MCP-created project opens");
    assert_eq!(bundle.manifest.generation_id, generation_id);
    assert_eq!(bundle.manifest.transaction_count, 0);
    assert!(bundle.log.entries().is_empty());
    assert!(bundle.graph.features().next().is_none());
    revision_hash(root)
}

fn revision_hash(root: &Path) -> String {
    Bundle::at(root)
        .open()
        .expect("MCP-created project has a revision")
        .revision_hash_hex()
        .to_string()
}

struct CleanupRoot(PathBuf);

impl Drop for CleanupRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn production_mcp_initializes_discovers_creates_project_and_extrudes_over_stdio() {
    let root = fresh_root();
    assert!(!root.exists(), "the MCP project destination starts absent");
    let mut client = McpProcess::spawn();

    let initialized = client.request(
        "initialize",
        "initialize",
        json!({
            "protocolVersion": PINNED_MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "threeterm-production-test-client", "version": "1.0.0"}
        }),
    );
    let initialize_result = assert_protocol_success(&initialized, "initialize");
    assert_eq!(
        initialize_result["protocolVersion"],
        PINNED_MCP_PROTOCOL_VERSION
    );
    assert_eq!(initialize_result["serverInfo"]["name"], "threeterm-mcp");
    assert!(initialize_result["capabilities"]["tools"].is_object());
    client.notify("notifications/initialized", json!({}));

    let tools = advertised_tools(&mut client);
    let tool_names = tools
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .expect("advertised tool name is a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    let unique_names = tool_names.iter().collect::<HashSet<_>>();
    assert_eq!(
        unique_names.len(),
        tool_names.len(),
        "tool names are unique"
    );
    let expected_names = iter()
        .map(|schema| schema.schema_version.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names, expected_names,
        "tools/list exposes the registry"
    );
    for schema in iter() {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == schema.schema_version)
            .expect("registered tool is advertised");
        assert_eq!(tool["inputSchema"], schema.request_schema);
        assert_eq!(tool["outputSchema"], schema.response_schema);
    }
    assert!(tool_names.iter().any(|name| name == NEW_PROJECT_TOOL));
    assert!(tool_names.iter().any(|name| name == EXTRUDE_TOOL));

    require_native_worker(
        "production_mcp_initializes_discovers_creates_project_and_extrudes_over_stdio",
    );

    let project = client.call_tool(
        "create",
        NEW_PROJECT_TOOL,
        json!({"destination": root.to_string_lossy()}),
    );
    let project_value = structured_tool_success(&project, "create");
    validate(
        &find(NEW_PROJECT_COMMAND_ID)
            .expect("new-project remains registered")
            .response_schema,
        &project_value,
    )
    .expect("MCP new-project response validates");
    let generation_id = project_value["generation_id"]
        .as_str()
        .expect("new-project returns a generation ID");
    assert!(root.join("manifest.json").is_file());
    assert!(root.join("transactions.log").is_file());
    let initial_revision = assert_fresh_project(&root, generation_id);
    let extrusion_profile = vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]];

    let extrusion = client.call_tool(
        "extrude",
        EXTRUDE_TOOL,
        json!({
            "bundle_path": root.to_string_lossy(),
            "expected_revision": initial_revision,
            "feature_id": "mcp-extrude",
            "profile": extrusion_profile.clone(),
            "height": 2.0,
            "mode": "additive"
        }),
    );
    let extrusion_value = structured_tool_success(&extrusion, "extrude");
    validate(
        &find(EXTRUDE_COMMAND_ID)
            .expect("extrude remains registered")
            .response_schema,
        &extrusion_value,
    )
    .expect("MCP extrusion response validates");
    assert_eq!(extrusion_value["status"], "ok");
    assert_eq!(extrusion_value["operation"], "extrude");
    assert_eq!(extrusion_value["feature_id"], "mcp-extrude");
    assert_eq!(extrusion_value["mode"], "additive");
    assert_eq!(extrusion_value["target_feature_id"], Value::Null);
    assert_eq!(
        extrusion_value["source_snapshot"]["revision_hash"],
        initial_revision
    );
    assert_ne!(extrusion_value["revision_hash"], initial_revision);
    assert_eq!(extrusion_value["authoritative"], true);
    assert_eq!(extrusion_value["worker_fingerprint"]["worker_kind"], "occt");
    assert_eq!(
        extrusion_value["worker_fingerprint"]["protocol_schema_version"],
        "threeterm.protocol/1"
    );

    let brep_path = PathBuf::from(
        extrusion_value["brep_path"]
            .as_str()
            .expect("MCP extrusion returns a BREP path"),
    );
    assert!(brep_path.is_file());
    assert!(brep_path.starts_with(root.join("brep")));
    let brep = fs::read(&brep_path).expect("promoted MCP BREP reads");
    assert_eq!(extrusion_value["brep_bytes"], brep.len());
    assert_eq!(extrusion_value["brep_sha256"], sha256_hex(&brep));
    let loaded = Bundle::at(&root).open().expect("extruded project opens");
    assert_eq!(loaded.log.len(), 1);
    assert_eq!(loaded.revision_hash_hex(), extrusion_value["revision_hash"]);
    let entry = loaded
        .log
        .entries()
        .last()
        .expect("extrusion transaction exists");
    assert_eq!(entry.feature_id, "mcp-extrude");
    assert!(matches!(
        entry.intent.as_ref(),
        Some(CanonicalIntent::Extrude(intent))
            if intent.command == "extrude"
                && intent.schema_version == EXTRUDE_INTENT_SCHEMA_VERSION
                && intent.operation == "additive"
                && intent.mode == "additive"
                && intent.target_feature_id.is_none()
                && !intent.request_id.is_empty()
                && intent.deterministic_inputs.profile == extrusion_profile
                && intent.deterministic_inputs.height == 2.0
                && intent.source_revision == initial_revision
                && intent.affected_semantic_ids == ["mcp-extrude"]
                && intent.worker_requirements.worker_kind == "occt"
                && !intent.worker_requirements.worker_schema_version.is_empty()
                && intent.worker_requirements.protocol_schema_version == "threeterm.protocol/1"
    ));
    if let Some(CanonicalIntent::Extrude(intent)) = entry.intent.as_ref() {
        intent
            .validate("mcp-extrude")
            .expect("persisted MCP extrusion intent validates");
    }

    let manifest_before_error = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before_error = fs::read(root.join("transactions.log")).expect("log reads");
    let brep_before_error = fs::read(&brep_path).expect("BREP reads");
    let invalid = client.call_tool(
        "invalid",
        EXTRUDE_TOOL,
        json!({
            "bundle_path": root.to_string_lossy(),
            "feature_id": "invalid",
            "profile": [[0.0, 0.0], [10.0, 10.0], [0.0, 10.0], [10.0, 0.0]],
            "height": 1.0,
            "mode": "additive"
        }),
    );
    assert_eq!(invalid["id"], "invalid");
    assert!(invalid.get("error").is_none() || invalid["error"].is_null());
    assert_eq!(invalid["result"]["isError"], true);
    assert_eq!(
        invalid["result"]["structuredContent"]["code"],
        "brep_invalid"
    );
    validate(
        &domain_diagnostic_schema(),
        &invalid["result"]["structuredContent"],
    )
    .expect("MCP domain diagnostic validates");
    assert_eq!(
        invalid["result"]["structuredContent"]["schema_version"],
        "threeterm.protocol/1"
    );
    assert!(invalid["result"]["structuredContent"]["arg"].is_string());
    assert_eq!(
        invalid["result"]["structuredContent"]["affected_ids"],
        json!(["invalid"])
    );
    assert_eq!(
        invalid["result"]["structuredContent"]["recovery"],
        "correct_geometry_or_restore_revision"
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads after error"),
        manifest_before_error
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads after error"),
        log_before_error
    );
    assert_eq!(
        fs::read(&brep_path).expect("BREP reads after error"),
        brep_before_error
    );
    assert!(!root.join("brep/invalid.brep").exists());
    let after_error = Bundle::at(&root)
        .open()
        .expect("project reopens after domain error");
    assert_eq!(
        after_error.revision_hash_hex(),
        extrusion_value["revision_hash"]
    );
    assert!(!after_error.graph.contains_feature("invalid"));

    let evidence = client.finish();
    assert!(
        evidence
            .protocol
            .iter()
            .all(|message| message["jsonrpc"] == "2.0"),
        "protocol evidence contains a non-JSON-RPC message: {evidence:?}"
    );
    assert_eq!(
        evidence.domain_errors.len(),
        1,
        "domain errors are separately attributed; protocol={:?}; server_diagnostics={:?}",
        evidence.protocol,
        evidence.server_diagnostics
    );
    assert_eq!(evidence.domain_errors[0]["id"], "invalid");
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn production_mcp_saves_restarts_loads_validates_and_exports_l_bracket_with_independent_stl_check()
{
    require_native_worker(
        "production_mcp_saves_restarts_loads_validates_and_exports_l_bracket_with_independent_stl_check",
    );
    let root = fresh_root();
    let _cleanup = CleanupRoot(root.clone());
    let project = root.join("project");
    let output = root.join("export");
    let logs = root.join("logs");
    let mut client = McpProcess::spawn();

    let initialized = client.request(
        "initialize",
        "initialize",
        json!({
            "protocolVersion": PINNED_MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "threeterm-mcp-lifecycle-test", "version": "1.0.0"}
        }),
    );
    assert_protocol_success(&initialized, "initialize");
    client.notify("notifications/initialized", json!({}));
    let tools = advertised_tools(&mut client);
    for command in [
        NEW_PROJECT_COMMAND_ID,
        BRACKET_COMMAND_ID,
        SAVE_COMMAND_ID,
        LOAD_COMMAND_ID,
        VALIDATE_COMMAND_ID,
        EXPORT_COMMAND_ID,
    ] {
        let schema = find(command).expect("lifecycle command is registered");
        assert!(
            tools
                .iter()
                .any(|tool| tool["name"] == schema.schema_version),
            "MCP discovery omits {}",
            schema.schema_version
        );
    }

    let created = client.call_tool(
        "create",
        find(NEW_PROJECT_COMMAND_ID)
            .expect("new-project is registered")
            .schema_version,
        json!({"destination": project.to_string_lossy()}),
    );
    let created = structured_tool_success(&created, "create");
    validate(
        &find(NEW_PROJECT_COMMAND_ID)
            .expect("new-project is registered")
            .response_schema,
        &created,
    )
    .expect("MCP new-project response validates");

    let bracket = client.call_tool(
        "bracket",
        find(BRACKET_COMMAND_ID)
            .expect("bracket is registered")
            .schema_version,
        json!({
            "bundle_path": project.to_string_lossy(),
            "bracket_id": "l-bracket",
            "length": 60.0,
            "width": 30.0,
            "height": 40.0,
            "thickness": 3.0,
        }),
    );
    let bracket = structured_tool_success(&bracket, "bracket");
    validate(
        &find(BRACKET_COMMAND_ID)
            .expect("bracket is registered")
            .response_schema,
        &bracket,
    )
    .expect("MCP bracket response validates");
    assert_eq!(bracket["feature_id"], "l-bracket");

    let saved = client.call_tool(
        "save",
        find(SAVE_COMMAND_ID)
            .expect("save is registered")
            .schema_version,
        json!({
            "bundle_path": project.to_string_lossy(),
            "feature_id": "mcp-lifecycle-save-marker",
            "kind": "lifecycle-marker",
        }),
    );
    let saved = structured_tool_success(&saved, "save");
    validate(
        &find(SAVE_COMMAND_ID)
            .expect("save is registered")
            .response_schema,
        &saved,
    )
    .expect("MCP save response validates");
    assert_ne!(saved["revision_hash"], bracket["revision_hash"]);

    let first_evidence = client.finish();
    assert!(first_evidence.server_diagnostics.is_empty());
    fs::create_dir_all(&logs).expect("MCP log directory creates");
    fs::write(
        logs.join("first-server.stderr"),
        first_evidence.server_diagnostics.as_bytes(),
    )
    .expect("first MCP diagnostics are retained");

    let mut restarted = McpProcess::spawn();
    let initialized = restarted.request(
        "restart-initialize",
        "initialize",
        json!({
            "protocolVersion": PINNED_MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "threeterm-mcp-lifecycle-test", "version": "1.0.0"}
        }),
    );
    assert_protocol_success(&initialized, "restart-initialize");
    restarted.notify("notifications/initialized", json!({}));

    let loaded = restarted.call_tool(
        "load",
        find(LOAD_COMMAND_ID)
            .expect("load is registered")
            .schema_version,
        json!({"bundle_path": project.to_string_lossy()}),
    );
    let loaded = structured_tool_success(&loaded, "load");
    validate(
        &find(LOAD_COMMAND_ID)
            .expect("load is registered")
            .response_schema,
        &loaded,
    )
    .expect("MCP load response validates");
    assert_eq!(loaded["feature_graph_hash"], saved["feature_graph_hash"]);
    assert_eq!(loaded["revision_hash"], saved["revision_hash"]);

    let validated = restarted.call_tool(
        "validate",
        find(VALIDATE_COMMAND_ID)
            .expect("validate is registered")
            .schema_version,
        json!({
            "bundle_path": project.to_string_lossy(),
            "feature_id": "l-bracket",
        }),
    );
    let validated = structured_tool_success(&validated, "validate");
    validate(
        &find(VALIDATE_COMMAND_ID)
            .expect("validate is registered")
            .response_schema,
        &validated,
    )
    .expect("MCP validate response validates");
    assert_eq!(validated["status"], "ok");
    assert_eq!(validated["valid"], true);
    assert_eq!(validated["feature_id"], "l-bracket");
    assert_eq!(validated["revision_hash"], loaded["revision_hash"]);

    assert!(!output.exists(), "export destination starts absent");
    let exported = restarted.call_tool(
        "export",
        find(EXPORT_COMMAND_ID)
            .expect("export is registered")
            .schema_version,
        json!({
            "bundle_path": project.to_string_lossy(),
            "feature_id": "l-bracket",
            "formats": ["stl"],
            "output_dir": output.to_string_lossy(),
            "tessellation_deflection": 0.1,
            "override_warnings": false,
            "accept_stale_geometry": false,
        }),
    );
    let exported = structured_tool_success(&exported, "export");
    validate(
        &find(EXPORT_COMMAND_ID)
            .expect("export is registered")
            .response_schema,
        &exported,
    )
    .expect("MCP export response validates");
    assert_eq!(exported["status"], "ok");
    assert_eq!(exported["feature_id"], validated["feature_id"]);
    assert_eq!(exported["source_revision_id"], validated["revision_hash"]);
    for field in [
        "feature_id",
        "revision_id",
        "feature_graph_hash",
        "revision_hash",
        "brep_path",
        "brep_sha256",
        "valid",
    ] {
        assert_eq!(
            exported["validation"][field], validated[field],
            "export validation is not bound to MCP validation for {field}"
        );
    }

    let protocol_failure = restarted.call_tool(
        "protocol-invalid",
        find(SAVE_COMMAND_ID)
            .expect("save is registered")
            .schema_version,
        json!({"bundle_path": project.to_string_lossy()}),
    );
    assert_eq!(protocol_failure["id"], "protocol-invalid");
    assert_eq!(protocol_failure["error"]["code"], -32602);
    assert!(protocol_failure["result"].is_null());

    let domain_failure = restarted.call_tool(
        "domain-invalid",
        find(VALIDATE_COMMAND_ID)
            .expect("validate is registered")
            .schema_version,
        json!({
            "bundle_path": project.to_string_lossy(),
            "feature_id": "missing-solid",
        }),
    );
    assert_eq!(domain_failure["id"], "domain-invalid");
    assert!(domain_failure.get("error").is_none() || domain_failure["error"].is_null());
    assert_eq!(domain_failure["result"]["isError"], true);
    validate(
        &domain_diagnostic_schema(),
        &domain_failure["result"]["structuredContent"],
    )
    .expect("MCP domain diagnostic validates");

    let stl_path = output.join("l-bracket.stl");
    assert!(stl_path.is_file());
    assert_eq!(
        exported["artifacts"],
        json!([stl_path.to_string_lossy().to_string()])
    );
    let report = stl_integrity::verify_path(&stl_path).expect("MCP STL passes independent check");
    assert_eq!(report.format, StlFormat::Ascii);
    assert!(report.triangle_count > 0);
    assert!(report.unique_vertex_count > 0);
    assert_eq!(report.shell_count, 1);
    assert!(report.signed_volume > 0.0);
    assert!(report.material_volume > 0.0);
    assert_eq!(report.policy, stl_integrity::VALIDATION_POLICY);
    let artifact = exported["derived_artifacts"]
        .as_array()
        .expect("MCP export includes derived artifact")
        .first()
        .expect("MCP export includes STL metadata");
    let bytes = fs::read(&stl_path).expect("MCP STL reads");
    assert_eq!(artifact["artifact_kind"], "stl");
    assert_eq!(artifact["byte_count"], bytes.len());
    assert_eq!(artifact["sha256"], sha256_hex(&bytes));
    assert_eq!(artifact["source_revision_id"], validated["revision_hash"]);
    assert_eq!(
        fs::read_dir(&output)
            .expect("export directory reads")
            .map(|entry| entry.expect("export entry reads").file_name())
            .collect::<Vec<_>>(),
        vec![std::ffi::OsString::from("l-bracket.stl")]
    );

    let second_evidence = restarted.finish();
    assert!(second_evidence.server_diagnostics.is_empty());
    fs::write(
        logs.join("second-server.stderr"),
        second_evidence.server_diagnostics.as_bytes(),
    )
    .expect("second MCP diagnostics are retained");
    assert!(
        first_evidence
            .protocol
            .iter()
            .chain(second_evidence.protocol.iter())
            .all(|message| message["jsonrpc"] == "2.0")
    );
    assert_eq!(second_evidence.protocol_errors.len(), 1);
    assert_eq!(second_evidence.protocol_errors[0]["id"], "protocol-invalid");
    assert_eq!(second_evidence.domain_errors.len(), 1);
    assert_eq!(second_evidence.domain_errors[0]["id"], "domain-invalid");
    assert!(logs.join("first-server.stderr").is_file());
    assert!(logs.join("second-server.stderr").is_file());
    assert!(project.is_dir());
    assert!(output.is_dir());

    for path in [&project, &output, &logs] {
        fs::remove_dir_all(path).expect("lifecycle directory cleans");
        assert!(!path.exists(), "lifecycle directory remains: {path:?}");
    }
    drop(_cleanup);
    assert!(!root.exists(), "lifecycle root remains after cleanup");
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn e2e_stl_mcp_all_tools_l_bracket() {
    require_native_worker("e2e_stl_mcp_all_tools_l_bracket");

    let recipe: Value =
        serde_json::from_str(COMPLETE_RECIPE).expect("complete recipe is valid JSON");
    assert_eq!(
        recipe_steps(&recipe).len(),
        33,
        "complete recipe has 33 steps"
    );

    let root = fresh_root();
    let _cleanup = CleanupRoot(root.clone());
    let project = root.join("project");
    let export_root = root.join("export");
    assert!(
        !project.exists(),
        "journey starts with no server-side project"
    );
    assert!(
        !export_root.exists(),
        "journey starts with no export fixture"
    );

    let mut journey_evidence: Vec<(String, String)> = Vec::new();
    let mut client = McpProcess::spawn();
    let initialized = client.request(
        "initialize",
        "initialize",
        json!({
            "protocolVersion": PINNED_MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "threeterm-mcp-e2e-journey", "version": "1.0.0"}
        }),
    );
    let initialize_result = assert_protocol_success(&initialized, "initialize");
    assert_eq!(
        initialize_result["protocolVersion"],
        PINNED_MCP_PROTOCOL_VERSION
    );
    client.notify("notifications/initialized", json!({}));

    let tools = advertised_tools(&mut client);
    let tool_names = tools
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .expect("advertised tool name is a string")
        })
        .collect::<HashSet<_>>();
    let mut required_tools = vec![
        find(NEW_PROJECT_COMMAND_ID)
            .expect("new-project is registered")
            .schema_version,
        find(IDENTITY_COMMAND_ID)
            .expect("identity is registered")
            .schema_version,
    ];
    for step in recipe_steps(&recipe) {
        let command_name = step["command"].as_str().expect("step has a command");
        let registered = find_by_name(command_name)
            .unwrap_or_else(|| panic!("recipe references unknown command {command_name}"));
        if !required_tools.contains(&registered.schema_version) {
            required_tools.push(registered.schema_version);
        }
    }
    for command_id in [LOAD_COMMAND_ID, VALIDATE_COMMAND_ID, EXPORT_COMMAND_ID] {
        required_tools.push(
            find(command_id)
                .expect("lifecycle command is registered")
                .schema_version,
        );
    }
    for tool in &required_tools {
        assert!(
            tool_names.contains(tool),
            "MCP discovery omits journey tool {tool}"
        );
    }

    let created = client.call_tool(
        "create",
        find(NEW_PROJECT_COMMAND_ID)
            .expect("new-project is registered")
            .schema_version,
        json!({"destination": project.to_string_lossy()}),
    );
    let created = structured_tool_success(&created, "create");
    validate(
        &find(NEW_PROJECT_COMMAND_ID)
            .expect("new-project is registered")
            .response_schema,
        &created,
    )
    .expect("MCP new-project response validates");
    journey_evidence.push(("new-project".to_string(), "create".to_string()));
    let generation_id = created["generation_id"]
        .as_str()
        .expect("new-project returns a generation ID")
        .to_string();
    let initial_revision = assert_fresh_project(&project, &generation_id);

    let identity = client.call_tool(
        "identity",
        find(IDENTITY_COMMAND_ID)
            .expect("identity is registered")
            .schema_version,
        json!({"bundle_path": project.to_string_lossy()}),
    );
    let identity = structured_tool_success(&identity, "identity");
    validate(
        &find(IDENTITY_COMMAND_ID)
            .expect("identity is registered")
            .response_schema,
        &identity,
    )
    .expect("MCP identity response validates");
    journey_evidence.push(("identity".to_string(), "identity".to_string()));
    assert_eq!(identity["transaction_count"], 0, "journey starts empty");
    assert_eq!(
        identity["revision_hash"], initial_revision,
        "identity seeds the initial revision"
    );

    let empty = Bundle::at(&project).open().expect("journey project opens");
    assert!(empty.log.is_empty(), "journey starts with an empty log");
    assert!(
        empty.graph.features().next().is_none(),
        "journey starts with no features"
    );
    assert!(!export_root.exists(), "export destination starts absent");

    let mut revision = initial_revision;
    for step in recipe_steps(&recipe) {
        let command_name = step["command"].as_str().expect("journey step command");
        let feature_id = step["feature_id"].as_str().expect("journey feature ID");
        let step_index = step["index"].as_u64().expect("journey step index");
        let registered = find_by_name(command_name)
            .unwrap_or_else(|| panic!("recipe references unknown command {command_name}"));
        let mut request = step["request"].clone();
        request["bundle_path"] = project.to_string_lossy().into_owned().into();
        if matches!(
            command_name,
            "extrude"
                | "revolve"
                | "fillet"
                | "chamfer"
                | "hole"
                | "shell"
                | "mirror"
                | "linear-pattern"
                | "circular-pattern"
                | "draft"
                | "loft"
                | "save"
        ) {
            request["expected_revision"] = revision.clone().into();
        }
        if matches!(command_name, "fillet" | "chamfer") {
            request["selected_edge"] = selected_edge_from_recipe(
                request["base_feature_id"]
                    .as_str()
                    .expect("finishing request has a base feature ID"),
                &revision,
                &step["edge_selection"],
            );
        }

        let call_id = format!("step-{step_index}");
        let response = client.call_tool(&call_id, registered.schema_version, request);
        let response = structured_tool_success(&response, &call_id);
        validate(&registered.response_schema, &response).unwrap_or_else(|error| {
            panic!("step {feature_id} response violates its schema: {error}")
        });
        let next_revision = response["revision_hash"]
            .as_str()
            .expect("journey response has a revision hash")
            .to_string();
        assert_ne!(
            next_revision, revision,
            "step {feature_id} advances revision"
        );
        if command_name != "save" {
            assert_eq!(response["status"], "ok", "step {feature_id} succeeds");
            assert_eq!(response["operation"], command_name);
            assert_eq!(response["feature_id"], feature_id);
        }
        journey_evidence.push((command_name.to_string(), call_id));
        revision = next_revision;
    }

    let expected_feature_ids: Vec<_> = recipe_steps(&recipe)
        .iter()
        .map(|step| step["feature_id"].as_str().expect("journey feature ID"))
        .collect();
    let saved = Bundle::at(&project)
        .open()
        .expect("journey bundle opens after the recipe");
    assert_eq!(
        saved.log.len(),
        recipe_steps(&recipe).len(),
        "journey retains every recipe transaction"
    );
    assert_eq!(
        saved
            .log
            .entries()
            .iter()
            .map(|entry| entry.feature_id.as_str())
            .collect::<Vec<_>>(),
        expected_feature_ids,
        "journey log feature ids match the recipe order"
    );
    assert_eq!(
        saved.manifest.transaction_count, recipe["expectations"]["transaction_count"],
        "journey manifest counts every recipe transaction"
    );
    assert_reinforcement_intents(&recipe, &saved);
    assert!(
        saved.log.entries()[19..32]
            .iter()
            .all(|entry| entry.intent.is_some()),
        "complete geometry steps retain canonical intents"
    );
    assert!(saved.log.entries()[32].intent.is_none());
    assert_complete_intents(&recipe, &saved);
    assert_eq!(
        saved.graph.features().count(),
        recipe_steps(&recipe).len(),
        "journey feature graph retains every recipe feature"
    );
    let finished_revision = revision.clone();
    let finished_graph_hash = saved.feature_graph_hash_hex().to_string();
    let finished_transaction_count = saved.manifest.transaction_count;

    let first_evidence = client.finish();
    assert!(first_evidence.server_diagnostics.is_empty());
    assert!(first_evidence.protocol_errors.is_empty());
    assert!(first_evidence.domain_errors.is_empty());

    let mut restarted = McpProcess::spawn();
    let reinitialized = restarted.request(
        "restart-initialize",
        "initialize",
        json!({
            "protocolVersion": PINNED_MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "threeterm-mcp-e2e-journey", "version": "1.0.0"}
        }),
    );
    let reinitialize_result = assert_protocol_success(&reinitialized, "restart-initialize");
    assert_eq!(
        reinitialize_result["protocolVersion"],
        PINNED_MCP_PROTOCOL_VERSION
    );
    restarted.notify("notifications/initialized", json!({}));

    let loaded = restarted.call_tool(
        "load",
        find(LOAD_COMMAND_ID)
            .expect("load is registered")
            .schema_version,
        json!({"bundle_path": project.to_string_lossy()}),
    );
    let loaded = structured_tool_success(&loaded, "load");
    validate(
        &find(LOAD_COMMAND_ID)
            .expect("load is registered")
            .response_schema,
        &loaded,
    )
    .expect("MCP load response validates");
    journey_evidence.push(("load".to_string(), "load".to_string()));
    assert_eq!(loaded["revision_hash"], finished_revision);
    assert_eq!(loaded["feature_graph_hash"], finished_graph_hash);

    let reloaded_identity = restarted.call_tool(
        "identity",
        find(IDENTITY_COMMAND_ID)
            .expect("identity is registered")
            .schema_version,
        json!({"bundle_path": project.to_string_lossy()}),
    );
    let reloaded_identity = structured_tool_success(&reloaded_identity, "identity");
    validate(
        &find(IDENTITY_COMMAND_ID)
            .expect("identity is registered")
            .response_schema,
        &reloaded_identity,
    )
    .expect("MCP reloaded identity response validates");
    journey_evidence.push(("identity".to_string(), "identity".to_string()));
    assert_eq!(
        reloaded_identity["transaction_count"], finished_transaction_count,
        "reloaded identity reports every recipe transaction"
    );
    assert_eq!(
        reloaded_identity["transaction_count"],
        recipe["expectations"]["transaction_count"]
    );
    assert_eq!(reloaded_identity["revision_hash"], finished_revision);
    assert_eq!(reloaded_identity["feature_graph_hash"], finished_graph_hash);
    assert_eq!(loaded["revision_hash"], reloaded_identity["revision_hash"]);
    assert_eq!(
        loaded["feature_graph_hash"],
        reloaded_identity["feature_graph_hash"]
    );

    let validated = restarted.call_tool(
        "validate",
        find(VALIDATE_COMMAND_ID)
            .expect("validate is registered")
            .schema_version,
        json!({
            "bundle_path": project.to_string_lossy(),
            "feature_id": "complete-bracket",
        }),
    );
    let validated = structured_tool_success(&validated, "validate");
    validate(
        &find(VALIDATE_COMMAND_ID)
            .expect("validate is registered")
            .response_schema,
        &validated,
    )
    .expect("MCP validate response validates");
    journey_evidence.push(("validate".to_string(), "validate".to_string()));
    assert_eq!(validated["status"], "ok", "MCP validate succeeds");
    assert_eq!(
        validated["valid"], true,
        "MCP validate accepts complete-bracket"
    );
    assert_eq!(validated["feature_id"], "complete-bracket");
    assert_eq!(validated["revision_hash"], finished_revision);
    assert_eq!(validated["feature_graph_hash"], finished_graph_hash);

    assert!(
        !export_root.exists(),
        "export destination stays absent before export"
    );
    let exported = restarted.call_tool(
        "export",
        find(EXPORT_COMMAND_ID)
            .expect("export is registered")
            .schema_version,
        json!({
            "bundle_path": project.to_string_lossy(),
            "feature_id": "complete-bracket",
            "formats": ["stl"],
            "output_dir": export_root.to_string_lossy(),
            "tessellation_deflection": mesh_number(&recipe["frozen"]["mesh"], "export_deflection"),
            "override_warnings": false,
            "accept_stale_geometry": false,
        }),
    );
    let exported = structured_tool_success(&exported, "export");
    validate(
        &find(EXPORT_COMMAND_ID)
            .expect("export is registered")
            .response_schema,
        &exported,
    )
    .expect("MCP export response validates");
    journey_evidence.push(("export".to_string(), "export".to_string()));
    assert_eq!(exported["status"], "ok", "MCP export succeeds");
    assert_eq!(exported["feature_id"], "complete-bracket");
    assert_eq!(exported["source_revision_id"], finished_revision);

    let stl_path = export_root.join("complete-bracket.stl");
    assert!(stl_path.is_file(), "journey STL exists after MCP export");
    assert_eq!(
        exported["artifacts"],
        json!([stl_path.to_string_lossy()]),
        "MCP export reports the journey STL artifact"
    );

    let report = stl_integrity::verify_path(&stl_path)
        .expect("journey STL passes independent integrity verification");
    let mesh = stl_integrity::observe_path(&stl_path).expect("journey STL observations parse");
    assert_bracket_mesh(&recipe, &report, &mesh).unwrap_or_else(|failure| {
        panic!(
            "journey bracket mesh failed landmark {}: {failure}",
            failure.landmark
        )
    });

    let second_evidence = restarted.finish();
    assert!(second_evidence.server_diagnostics.is_empty());
    assert!(second_evidence.protocol_errors.is_empty());
    assert!(second_evidence.domain_errors.is_empty());

    let mut required_commands = vec!["new-project".to_string(), "identity".to_string()];
    for step in recipe_steps(&recipe) {
        let command_name = step["command"].as_str().expect("journey step command");
        if !required_commands.iter().any(|name| name == command_name) {
            required_commands.push(command_name.to_string());
        }
    }
    for command_id in [LOAD_COMMAND_ID, VALIDATE_COMMAND_ID, EXPORT_COMMAND_ID] {
        let registered = find(command_id).expect("lifecycle command is registered");
        if !required_commands.iter().any(|name| name == registered.name) {
            required_commands.push(registered.name.to_string());
        }
    }

    let chained_protocol: Vec<&Value> = first_evidence
        .protocol
        .iter()
        .chain(second_evidence.protocol.iter())
        .collect();
    for command_name in &required_commands {
        let (_, call_id) = journey_evidence
            .iter()
            .find(|(recorded, _)| recorded == command_name)
            .unwrap_or_else(|| panic!("journey evidence omits required command {command_name}"));
        assert_correlated_structured_ok(&chained_protocol, call_id, command_name);
    }
}

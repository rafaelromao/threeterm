use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::{Bundle, CanonicalIntent, EXTRUDE_INTENT_SCHEMA_VERSION};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::schema::{EXTRUDE_COMMAND_ID, NEW_PROJECT_COMMAND_ID, find, iter};
use threeterm_protocol::schema_validator::validate;

const PINNED_MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const NEW_PROJECT_TOOL: &str = "threeterm.command.new-project/1";
const EXTRUDE_TOOL: &str = "threeterm.command.extrude/2";

type StreamLine = Result<Vec<u8>, String>;

#[derive(Debug)]
struct McpEvidence {
    protocol: Vec<Value>,
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
                if value["result"]["isError"] == true {
                    self.domain_errors.push(value.clone());
                }
                return value;
            }
            self.pending.insert(response_key, value);
        }
    }

    fn finish(mut self) -> McpEvidence {
        self.stdin.take();
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
            server_diagnostics: String::from_utf8_lossy(&stderr).into_owned(),
            domain_errors: std::mem::take(&mut self.domain_errors),
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        self.stdin.take();
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/threeterm-mcp")
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

//! MCP (Model Context Protocol) adapter for the ThreeTerm domain command API.
//!
//! The adapter speaks newline-framed JSON-RPC 2.0 over stdio so it can be
//! driven by any MCP-compatible client. The registered commands in
//! `threeterm_protocol::schema::COMMAND_REGISTRY` are advertised as
//! `tools/list` entries, and `tools/call` dispatches each named tool through
//! the shared CLI dispatcher (`threeterm_cli::dispatch`) so the CLI and MCP
//! transports share the same dispatch code path.
//!
//! Success returns a JSON-RPC success envelope with the validated
//! response-schema value as `result.structuredContent` (plus a `content`
//! payload). Errors return a JSON-RPC error envelope with one of the
//! standard codes:
//!
//! - `-32600` invalid request
//! - `-32601` method not found
//! - `-32602` invalid params (schema validation failure)
//! - `-32603` internal error (host/dispatch failure)
//!
//! Every error path leaves the on-disk bundle byte-identical to its prior
//! successful load (canonical host state preservation is inherited from the
//! `Host` and `Bundle` layers).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value, json};
use threeterm_cli::dispatch::{
    DispatchError, EXIT_OK, dispatch_bracket, dispatch_registered_command, host_error_diagnostic,
};
use threeterm_host::{BREP_SUBDIR, Host, HostError};
use threeterm_occt_worker::{BooleanPatternRequest, BracketRequest, OcctWorker, new_request_id};
use threeterm_persistence::Bundle;
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::frame::MAX_FRAME_BUFFER;
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, BOOLEAN_PATTERN_COMMAND_ID, BRACKET_COMMAND_ID, BRACKET_EDIT_COMMAND_ID,
    CommandSchema, IDENTITY_COMMAND_ID, iter,
};
use threeterm_protocol::schema_validator::validate;

pub const JSONRPC_VERSION: &str = "2.0";
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

pub const ERROR_INVALID_REQUEST: i32 = -32600;
pub const ERROR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERROR_INVALID_PARAMS: i32 = -32602;
pub const ERROR_INTERNAL: i32 = -32603;
pub const ERROR_PARSE: i32 = -32700;
pub const MAX_PROGRESS_NOTIFICATIONS: usize = 100;
const MAX_PROGRESS_STAGE_CHARS: usize = 256;
const MAX_PENDING_CANCELLATIONS: usize = 128;

/// Tool descriptor exposed by `tools/list`. The wire shape follows the MCP
/// tool advertisement convention with `inputSchema` and `outputSchema`
/// populated from the protocol registry row.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(rename = "outputSchema", skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

impl ToolDescriptor {
    fn from_schema(schema: &CommandSchema) -> Self {
        Self {
            name: schema.schema_version.to_string(),
            description: "ThreeTerm versioned domain command (see threeterm_protocol::schema).",
            input_schema: schema.request_schema.clone(),
            output_schema: (schema.response_schema["type"] == "object")
                .then(|| schema.response_schema.clone()),
        }
    }
}

/// One JSON-RPC 2.0 request. The `params` field is kept as a generic
/// `Value` so the dispatcher can validate the inner `arguments` object
/// against the registered request schema.
#[derive(Debug, Clone)]
pub struct JsonRpcRequest {
    pub id: Value,
    pub is_notification: bool,
    pub method: String,
    pub params: Value,
}

/// One JSON-RPC 2.0 response. Either `result` is `Some` (success) or
/// `error` is `Some` (failure); both being `None` is treated as a
/// programming error and is not produced by the dispatcher.
#[derive(Debug, Clone)]
pub struct JsonRpcResponse {
    pub id: Value,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i32, message: String) -> Self {
        Self {
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

/// Driver for the MCP server. The request loop reads newline-framed
/// JSON-RPC 2.0 envelopes from a `BufRead` source and writes responses
/// to a `Write` sink. One `McpServer` is constructed per process.
#[derive(Debug)]
struct BracketEditSession {
    host: Host,
    worker: OcctWorker,
}

#[derive(Debug)]
enum RunEvent {
    Request(JsonRpcRequest),
    ParseError(JsonRpcResponse),
    EndOfInput,
    Progress {
        request_key: String,
        token: Value,
        progress: threeterm_protocol::supervisor::Progress,
    },
    Completed {
        request_key: String,
        response: Result<Value, HostError>,
    },
}

#[derive(Debug)]
enum ControlEvent {
    Cancellation(JsonRpcRequest),
    ReadError(std::io::Error),
}

#[derive(Debug)]
struct ActiveRequest {
    request_key: String,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
pub struct McpServer {
    bracket_edits: RefCell<HashMap<(PathBuf, String), BracketEditSession>>,
    boolean_pattern_worker: Option<OcctWorker>,
}

impl McpServer {
    pub fn new() -> Self {
        Self::default()
    }

    #[doc(hidden)]
    pub fn with_boolean_pattern_worker(mut self, worker: OcctWorker) -> Self {
        self.boolean_pattern_worker = Some(worker);
        self
    }

    /// Build the `tools/list` response payload from the static command
    /// registry. Insertion order is the registry's natural BTreeMap order
    /// (alphabetical by `CommandId`), so the wire output is deterministic.
    pub fn advertise_tools(&self) -> Vec<ToolDescriptor> {
        iter().map(ToolDescriptor::from_schema).collect()
    }

    /// Dispatch one parsed JSON-RPC request. Pure with respect to the
    /// host state — the caller controls the `Host` instance via the
    /// `dispatch_bracket` pure function. Frame-level errors are returned
    /// as a structured `JsonRpcResponse` with code `-32603`.
    pub fn handle_request(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        match request.method.as_str() {
            "tools/list" => self.handle_tools_list(request),
            "tools/call" => self.handle_tools_call(request),
            "initialize" => Self::handle_initialize(request),
            _ => JsonRpcResponse::error(
                request.id.clone(),
                ERROR_METHOD_NOT_FOUND,
                format!("method not found: {:?}", request.method),
            ),
        }
    }

    fn handle_tools_list(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let tools = self.advertise_tools();
        JsonRpcResponse::success(request.id.clone(), json!({ "tools": tools }))
    }

    fn handle_initialize(request: &JsonRpcRequest) -> JsonRpcResponse {
        let params = match request.params.as_object() {
            Some(params) => params,
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    ERROR_INVALID_PARAMS,
                    "initialize params must be an object".to_string(),
                );
            }
        };
        if params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none()
        {
            return JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INVALID_PARAMS,
                "initialize params.protocolVersion must be a string".to_string(),
            );
        }
        if !params.get("capabilities").is_some_and(Value::is_object) {
            return JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INVALID_PARAMS,
                "initialize params.capabilities must be an object".to_string(),
            );
        }
        let Some(client_info) = params.get("clientInfo").and_then(Value::as_object) else {
            return JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INVALID_PARAMS,
                "initialize params.clientInfo must be an object".to_string(),
            );
        };
        if !client_info.get("name").is_some_and(Value::is_string)
            || !client_info.get("version").is_some_and(Value::is_string)
        {
            return JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INVALID_PARAMS,
                "initialize params.clientInfo requires string name and version".to_string(),
            );
        }
        JsonRpcResponse::success(
            request.id.clone(),
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "serverInfo": {
                    "name": "threeterm-mcp",
                    "version": crate::schema_version(),
                },
                "capabilities": {
                    "tools": {}
                }
            }),
        )
    }

    fn handle_tools_call(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let name = match request.params.get("name").and_then(Value::as_str) {
            Some(name) => name,
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    ERROR_INVALID_PARAMS,
                    "tools/call params.name must be a non-empty string".to_string(),
                );
            }
        };
        let arguments = request
            .params
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));

        let schema_entry = match find_by_wire_name(name) {
            Some(entry) => entry,
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    ERROR_METHOD_NOT_FOUND,
                    format!("tool not found: {name}"),
                );
            }
        };

        if matches!(
            schema_entry.id,
            IDENTITY_COMMAND_ID | APPLY_COMMAND_ID | BOOLEAN_PATTERN_COMMAND_ID
        ) {
            return self.handle_domain_command(request, schema_entry.id, arguments);
        }

        if let Err(reason) = validate(&schema_entry.request_schema, &arguments) {
            return JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INVALID_PARAMS,
                format!("tools/call arguments failed request-schema validation: {reason}"),
            );
        }
        if schema_entry.id == BRACKET_EDIT_COMMAND_ID
            && arguments.get("phase").and_then(Value::as_str) == Some("update")
        {
            for field in ["draft_sequence", "input_fingerprint"] {
                if !arguments.get(field).is_some_and(|value| !value.is_null()) {
                    return JsonRpcResponse::error(
                        request.id.clone(),
                        ERROR_INVALID_PARAMS,
                        format!("bracket-edit update requires {field}"),
                    );
                }
            }
        }

        let result = match schema_entry.id {
            BRACKET_COMMAND_ID => dispatch_bracket_tool(&arguments),
            BRACKET_EDIT_COMMAND_ID => self.dispatch_bracket_edit_tool(&arguments),
            _ => dispatch_registered_command(&Host::new(), schema_entry.id, arguments.clone()),
        };

        match result {
            Ok(value) => {
                if let Err(reason) = validate(&schema_entry.response_schema, &value) {
                    return JsonRpcResponse::error(
                        request.id.clone(),
                        ERROR_INTERNAL,
                        format!(
                            "dispatcher produced a response that fails the registered response schema: {reason}"
                        ),
                    );
                }
                let envelope = tool_result(value, false);
                JsonRpcResponse::success(request.id.clone(), envelope)
            }
            Err(error) => match error {
                DispatchError::UnsupportedTool { .. } => JsonRpcResponse::error(
                    request.id.clone(),
                    ERROR_METHOD_NOT_FOUND,
                    format!("{error}"),
                ),
                DispatchError::Host(error) if schema_entry.id == BRACKET_EDIT_COMMAND_ID => {
                    let value = bracket_edit_failure_response(&arguments, &error);
                    JsonRpcResponse::success(request.id.clone(), tool_result(value, true))
                }
                DispatchError::Host(_)
                | DispatchError::Validation(_)
                | DispatchError::UnknownCommand(_) => JsonRpcResponse::success(
                    request.id.clone(),
                    tool_execution_error(format!("host dispatch failed: {error}")),
                ),
            },
        }
    }

    fn handle_domain_command(
        &self,
        request: &JsonRpcRequest,
        command: threeterm_protocol::schema::CommandId,
        arguments: Value,
    ) -> JsonRpcResponse {
        match Host::new().execute_domain_command(command, arguments) {
            Ok(value) => JsonRpcResponse::success(request.id.clone(), tool_result(value, false)),
            Err(ExecutionError::InvalidRequest(reason)) => JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INVALID_PARAMS,
                format!("tools/call arguments failed request-schema validation: {reason}"),
            ),
            Err(ExecutionError::Handler(error)) => JsonRpcResponse::success(
                request.id.clone(),
                tool_execution_error(format!("domain command failed: {error}")),
            ),
            Err(ExecutionError::InvalidResponse(reason)) => JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INTERNAL,
                format!("domain command response failed response-schema validation: {reason}"),
            ),
            Err(ExecutionError::UnknownCommand(command)) => JsonRpcResponse::error(
                request.id.clone(),
                ERROR_METHOD_NOT_FOUND,
                format!("command not found: {}", command.0),
            ),
        }
    }

    fn dispatch_bracket_edit_tool(&self, arguments: &Value) -> Result<Value, DispatchError> {
        self.bracket_edits
            .borrow_mut()
            .retain(|(root, draft_id), session| {
                session
                    .host
                    .prune_expired_drafts(Duration::from_secs(30 * 60));
                session.host.has_bracket_parameter_draft(root, draft_id)
            });
        let phase = arguments
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bundle = arguments
            .get("bundle_path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let draft_id = arguments
            .get("draft_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let bracket_id = arguments
            .get("bracket_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let draft_sequence = arguments
            .get("draft_sequence")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        let input_fingerprint = arguments
            .get("input_fingerprint")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let request = || {
            BracketRequest::new(
                new_request_id(),
                arguments["length"].as_f64().unwrap_or_default(),
                arguments["width"].as_f64().unwrap_or_default(),
                arguments["height"].as_f64().unwrap_or_default(),
                arguments["thickness"].as_f64().unwrap_or_default(),
            )
            .with_output_path(&bundle, "unused.brep")
            .with_feature_id(&bracket_id)
        };
        let canonical_bundle = Bundle::at(&bundle).canonical_root().to_path_buf();
        let key = (canonical_bundle, draft_id.clone());
        match phase {
            "open" => {
                if let Some(session) = self.bracket_edits.borrow().get(&key) {
                    let source_revision = session
                        .host
                        .bracket_draft_source_revision(&bundle, &draft_id)
                        .unwrap_or_default();
                    let current_revision = Bundle::at(&bundle)
                        .open()
                        .map(|loaded| loaded.revision_hash_hex().to_string())
                        .unwrap_or_else(|_| source_revision.clone());
                    return Err(DispatchError::Host(HostError::DraftInputConflict {
                        draft_id,
                        source_revision,
                        current_revision,
                        recovery: "use_update_or_refresh_draft",
                    }));
                }
                let worker = OcctWorker::locate().map_err(|error| {
                    DispatchError::Host(HostError::WorkerUnavailable {
                        detail: error.to_string(),
                    })
                })?;
                let host = Host::new();
                let draft =
                    host.open_bracket_parameter_draft(&bundle, &draft_id, &bracket_id, request())?;
                let draft_fingerprint = host
                    .bracket_draft_fingerprint(&bundle, &draft_id)
                    .expect("open keeps draft");
                self.bracket_edits
                    .borrow_mut()
                    .insert(key, BracketEditSession { host, worker });
                Ok(bracket_edit_response(
                    "open",
                    &draft.draft_id,
                    &draft.source_revision,
                    None,
                    None,
                    Some(&draft_fingerprint),
                    Some(draft.sequence),
                ))
            }
            "preview" => {
                let sessions = self.bracket_edits.borrow();
                let session = sessions.get(&key).ok_or_else(|| {
                    DispatchError::Host(HostError::DraftNotFound {
                        draft_id: draft_id.clone(),
                    })
                })?;
                session.host.validate_bracket_parameter_draft_request(
                    &bundle,
                    &draft_id,
                    request(),
                )?;
                let preview = session.host.preview_bracket_parameter_draft(
                    &bundle,
                    &draft_id,
                    &session.worker,
                )?;
                Ok(bracket_edit_response(
                    "preview",
                    &preview.draft_id,
                    &preview.source_revision,
                    Some(&preview.source_revision),
                    Some(&preview.preview_revision),
                    Some(&preview.input_fingerprint),
                    Some(
                        session
                            .host
                            .bracket_draft_sequence(&bundle, &draft_id)
                            .expect("preview keeps draft"),
                    ),
                ))
            }
            "commit" => {
                let mut sessions = self.bracket_edits.borrow_mut();
                let session = sessions.get_mut(&key).ok_or_else(|| {
                    DispatchError::Host(HostError::DraftNotFound {
                        draft_id: draft_id.clone(),
                    })
                })?;
                session.host.validate_bracket_parameter_draft_request(
                    &bundle,
                    &draft_id,
                    request(),
                )?;
                let source_revision = session
                    .host
                    .bracket_draft_source_revision(&bundle, &draft_id)
                    .ok_or_else(|| {
                        DispatchError::Host(HostError::DraftNotFound {
                            draft_id: draft_id.clone(),
                        })
                    })?;
                let committed = session.host.commit_bracket_parameter_draft(
                    &bundle,
                    &draft_id,
                    &session.worker,
                )?;
                sessions.remove(&key);
                Ok(bracket_edit_response(
                    "commit",
                    &draft_id,
                    &source_revision,
                    Some(&committed.snapshot.revision_hash),
                    None,
                    Some(&committed.input_fingerprint),
                    None,
                ))
            }
            "discard" => {
                let mut sessions = self.bracket_edits.borrow_mut();
                let session = sessions.get_mut(&key).ok_or_else(|| {
                    DispatchError::Host(HostError::DraftNotFound {
                        draft_id: draft_id.clone(),
                    })
                })?;
                let source_revision = session
                    .host
                    .discard_bracket_parameter_draft(&bundle, &draft_id)?;
                sessions.remove(&key);
                Ok(bracket_edit_response(
                    "discard",
                    &draft_id,
                    &source_revision,
                    None,
                    None,
                    None,
                    None,
                ))
            }
            "update" => {
                let mut sessions = self.bracket_edits.borrow_mut();
                let session = sessions.get_mut(&key).ok_or_else(|| {
                    DispatchError::Host(HostError::DraftNotFound {
                        draft_id: draft_id.clone(),
                    })
                })?;
                let draft = session.host.update_bracket_parameter_draft(
                    &bundle,
                    &draft_id,
                    draft_sequence,
                    input_fingerprint,
                    request(),
                )?;
                let draft_fingerprint = session
                    .host
                    .bracket_draft_fingerprint(&bundle, &draft_id)
                    .expect("update keeps draft");
                Ok(bracket_edit_response(
                    "update",
                    &draft.draft_id,
                    &draft.source_revision,
                    None,
                    None,
                    Some(&draft_fingerprint),
                    Some(draft.sequence),
                ))
            }
            _ => Err(DispatchError::Validation(
                "bracket-edit phase is invalid".to_string(),
            )),
        }
    }

    /// Drive the newline-framed JSON-RPC 2.0 loop over `reader`/`writer`.
    /// Returns the number of requests handled. The function returns when
    /// `reader` hits EOF or a malformed frame aborts the current chunk; in
    /// the latter case the buffered bytes are dropped so the supervisor
    /// can resync on the next chunk (closed issue #49 contract).
    pub fn run<R: BufRead + Send, W: Write>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<usize, std::io::Error> {
        let (events, receive) = mpsc::sync_channel(128);
        let (control_events, control_receive) = mpsc::sync_channel(128);
        let queued_request_ids = Arc::new(Mutex::new(HashSet::new()));
        thread::scope(|scope| {
            let sender = events.clone();
            let control_sender = control_events.clone();
            let reader_queued_request_ids = Arc::clone(&queued_request_ids);
            scope.spawn(move || {
                let mut buffer = Vec::with_capacity(4096);
                loop {
                    buffer.clear();
                    let read = match read_bounded_frame(reader, &mut buffer) {
                        Ok(read) => read,
                        Err(error) => {
                            let _ = control_sender.send(ControlEvent::ReadError(error));
                            return;
                        }
                    };
                    if read == 0 {
                        let _ = sender.send(RunEvent::EndOfInput);
                        return;
                    }
                    for line in split_newlines(&buffer) {
                        if line.is_empty() {
                            continue;
                        }
                        let value: Value = match serde_json::from_slice(line) {
                            Ok(value) => value,
                            Err(error) => {
                                if sender
                                    .try_send(RunEvent::ParseError(JsonRpcResponse::error(
                                        Value::Null,
                                        ERROR_PARSE,
                                        format!("frame is not valid JSON: {error}"),
                                    )))
                                    .is_err()
                                {
                                    let _ = control_sender.send(ControlEvent::ReadError(
                                        std::io::Error::other("MCP event queue is full"),
                                    ));
                                    return;
                                }
                                continue;
                            }
                        };
                        let request = match parse_request(&value) {
                            Ok(request) => request,
                            Err(error) => {
                                if sender
                                    .try_send(RunEvent::ParseError(JsonRpcResponse::error(
                                        extract_id(&value),
                                        ERROR_INVALID_REQUEST,
                                        error,
                                    )))
                                    .is_err()
                                {
                                    let _ = control_sender.send(ControlEvent::ReadError(
                                        std::io::Error::other("MCP event queue is full"),
                                    ));
                                    return;
                                }
                                continue;
                            }
                        };
                        if request.method == "notifications/cancelled" {
                            if control_sender
                                .send(ControlEvent::Cancellation(request))
                                .is_err()
                            {
                                return;
                            }
                        } else {
                            if request.method == "tools/call" && !request.is_notification {
                                reader_queued_request_ids
                                    .lock()
                                    .expect("queued request IDs mutex is not poisoned")
                                    .insert(request_key(&request.id));
                            }
                            if sender.try_send(RunEvent::Request(request)).is_err() {
                                let _ = control_sender.send(ControlEvent::ReadError(
                                    std::io::Error::other("MCP event queue is full"),
                                ));
                                return;
                            }
                        }
                    }
                }
            });

            let mut active: Option<ActiveRequest> = None;
            let mut handled = 0usize;
            let mut input_finished = false;
            let mut read_error = None;
            let mut pending_cancellations = HashSet::new();
            loop {
                while let Ok(control) = control_receive.try_recv() {
                    match control {
                        ControlEvent::Cancellation(request) => {
                            if let Some(active) = &active
                                && cancellation_targets(&request, &active.request_key)
                            {
                                active.cancel.store(true, Ordering::SeqCst);
                            } else if pending_cancellations.len() < MAX_PENDING_CANCELLATIONS
                                && let Some(request_id) = request.params.get("requestId")
                                && queued_request_ids
                                    .lock()
                                    .expect("queued request IDs mutex is not poisoned")
                                    .contains(&request_key(request_id))
                            {
                                pending_cancellations.insert(request_key(request_id));
                            }
                            handled += 1;
                        }
                        ControlEvent::ReadError(error) => {
                            if let Some(active) = &active {
                                active.cancel.store(true, Ordering::SeqCst);
                            }
                            read_error = Some(error);
                            input_finished = true;
                        }
                    }
                }
                if input_finished && active.is_none() {
                    break match read_error.take() {
                        Some(error) => Err(error),
                        None => Ok(handled),
                    };
                }
                let event = match receive.recv_timeout(Duration::from_millis(10)) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(handled),
                };
                match event {
                    RunEvent::Request(request) if request.method == "notifications/cancelled" => {
                        if let Some(active) = &active
                            && cancellation_targets(&request, &active.request_key)
                        {
                            active.cancel.store(true, Ordering::SeqCst);
                        }
                        handled += 1;
                    }
                    RunEvent::Request(request) if is_boolean_pattern_call(&request) => {
                        if request.is_notification {
                            handled += 1;
                            continue;
                        }
                        let arguments = request
                            .params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| Value::Object(Default::default()));
                        if let Err(reason) = validate(
                            &threeterm_protocol::schema::BOOLEAN_PATTERN_REQUEST_SCHEMA,
                            &arguments,
                        ) {
                            if let Err(error) = write_envelope(
                                writer,
                                &JsonRpcResponse::error(
                                    request.id.clone(),
                                    ERROR_INVALID_PARAMS,
                                    format!(
                                        "tools/call arguments failed request-schema validation: {reason}"
                                    ),
                                ),
                            ) {
                                if let Some(active) = &active {
                                    active.cancel.store(true, Ordering::SeqCst);
                                }
                                return Err(error);
                            }
                            handled += 1;
                            continue;
                        }
                        if active.is_some() {
                            queued_request_ids
                                .lock()
                                .expect("queued request IDs mutex is not poisoned")
                                .remove(&request_key(&request.id));
                            if let Err(error) = write_envelope(
                                writer,
                                &JsonRpcResponse::success(
                                    request.id.clone(),
                                    tool_execution_error(
                                        "another expensive command is already active".to_string(),
                                    ),
                                ),
                            ) {
                                if let Some(active) = &active {
                                    active.cancel.store(true, Ordering::SeqCst);
                                }
                                return Err(error);
                            }
                        } else {
                            let request_key = request_key(&request.id);
                            queued_request_ids
                                .lock()
                                .expect("queued request IDs mutex is not poisoned")
                                .remove(&request_key);
                            let cancel = Arc::new(AtomicBool::new(false));
                            if pending_cancellations.remove(&request_key) {
                                cancel.store(true, Ordering::SeqCst);
                            }
                            let worker_cancel = Arc::clone(&cancel);
                            let sender = events.clone();
                            let token = progress_token(&request);
                            let configured_worker = self.boolean_pattern_worker.clone();
                            let event_key = request_key.clone();
                            scope.spawn(move || {
                                let mut emitted = 0usize;
                                let mut last = None;
                                let mut on_progress =
                                    |progress: &threeterm_protocol::supervisor::Progress| {
                                        let Some(token) = &token else { return };
                                        if emitted >= MAX_PROGRESS_NOTIFICATIONS
                                            || last.as_ref() == Some(progress)
                                        {
                                            return;
                                        }
                                        let mut progress = progress.clone();
                                        progress.stage = progress
                                            .stage
                                            .chars()
                                            .take(MAX_PROGRESS_STAGE_CHARS)
                                            .collect();
                                        last = Some(progress.clone());
                                        emitted += 1;
                                        let _ = sender.send(RunEvent::Progress {
                                            request_key: event_key.clone(),
                                            token: token.clone(),
                                            progress,
                                        });
                                    };
                                let response = execute_boolean_pattern(
                                    arguments,
                                    &worker_cancel,
                                    &mut on_progress,
                                    configured_worker.as_ref(),
                                );
                                let _ = sender.send(RunEvent::Completed {
                                    request_key: event_key,
                                    response,
                                });
                            });
                            active = Some(ActiveRequest {
                                request_key,
                                cancel,
                            });
                        }
                        handled += 1;
                    }
                    RunEvent::Request(request)
                        if active.is_some() && request.method == "tools/call" =>
                    {
                        if !request.is_notification {
                            let response = JsonRpcResponse::success(
                                request.id,
                                tool_execution_error(
                                    "another expensive command is already active".to_string(),
                                ),
                            );
                            if let Err(error) = write_envelope(writer, &response) {
                                if let Some(active) = &active {
                                    active.cancel.store(true, Ordering::SeqCst);
                                }
                                return Err(error);
                            }
                        }
                        handled += 1;
                    }
                    RunEvent::Request(request) => {
                        if request.method == "tools/call" && !request.is_notification {
                            queued_request_ids
                                .lock()
                                .expect("queued request IDs mutex is not poisoned")
                                .remove(&request_key(&request.id));
                        }
                        let response = self.handle_request(&request);
                        if !request.is_notification
                            && let Err(error) = write_envelope(writer, &response)
                        {
                            if let Some(active) = &active {
                                active.cancel.store(true, Ordering::SeqCst);
                            }
                            return Err(error);
                        }
                        handled += 1;
                    }
                    RunEvent::Progress {
                        request_key: event_key,
                        token,
                        progress,
                    } => {
                        if active
                            .as_ref()
                            .is_some_and(|request| request.request_key == event_key)
                            && let Err(error) = write_progress(writer, &token, &progress)
                        {
                            if let Some(active) = &active {
                                active.cancel.store(true, Ordering::SeqCst);
                            }
                            return Err(error);
                        }
                    }
                    RunEvent::Completed {
                        request_key: event_key,
                        response,
                    } => {
                        if active
                            .as_ref()
                            .is_some_and(|request| request.request_key == event_key)
                        {
                            while let Ok(control) = control_receive.try_recv() {
                                match control {
                                    ControlEvent::Cancellation(request) => {
                                        if cancellation_targets(&request, &event_key)
                                            && let Some(active) = &active
                                        {
                                            active.cancel.store(true, Ordering::SeqCst);
                                        }
                                        handled += 1;
                                    }
                                    ControlEvent::ReadError(error) => {
                                        if let Some(active) = &active {
                                            active.cancel.store(true, Ordering::SeqCst);
                                        }
                                        read_error = Some(error);
                                        input_finished = true;
                                    }
                                }
                            }
                            let request_id =
                                active.take().expect("active request exists").request_key;
                            let response = match response {
                                Ok(value) => JsonRpcResponse::success(
                                    parse_request_id(&request_id),
                                    tool_result(value, false),
                                ),
                                Err(error) => JsonRpcResponse::success(
                                    parse_request_id(&request_id),
                                    host_tool_execution_error(&error),
                                ),
                            };
                            if let Err(error) = write_envelope(writer, &response) {
                                if let Some(active) = &active {
                                    active.cancel.store(true, Ordering::SeqCst);
                                }
                                return Err(error);
                            }
                        }
                    }
                    RunEvent::ParseError(response) => {
                        if let Err(error) = write_envelope(writer, &response) {
                            if let Some(active) = &active {
                                active.cancel.store(true, Ordering::SeqCst);
                            }
                            return Err(error);
                        }
                    }
                    RunEvent::EndOfInput => {
                        input_finished = true;
                        if active.is_none() {
                            break Ok(handled);
                        }
                    }
                }
                if input_finished && active.is_none() {
                    break match read_error.take() {
                        Some(error) => Err(error),
                        None => Ok(handled),
                    };
                }
            }
        })
    }
}

fn is_boolean_pattern_call(request: &JsonRpcRequest) -> bool {
    request.method == "tools/call"
        && request.params.get("name").and_then(Value::as_str)
            == Some("threeterm.command.boolean-pattern/1")
}

fn request_key(id: &Value) -> String {
    serde_json::to_string(id).expect("valid JSON-RPC id serializes")
}

fn parse_request_id(key: &str) -> Value {
    serde_json::from_str(key).expect("active JSON-RPC id remains valid JSON")
}

fn cancellation_targets(request: &JsonRpcRequest, expected_key: &str) -> bool {
    request
        .params
        .get("requestId")
        .is_some_and(|id| request_key(id) == expected_key)
}

fn progress_token(request: &JsonRpcRequest) -> Option<Value> {
    request
        .params
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("progressToken"))
        .filter(|token| token.is_string() || token.as_i64().is_some() || token.as_u64().is_some())
        .cloned()
}

fn valid_feature_path_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn write_progress<W: Write>(
    writer: &mut W,
    token: &Value,
    progress: &threeterm_protocol::supervisor::Progress,
) -> std::io::Result<()> {
    let payload = json!({
        "jsonrpc": JSONRPC_VERSION,
        "method": "notifications/progress",
        "params": {
            "progressToken": token,
            "progress": progress.percent,
            "total": 100,
            "message": progress.stage,
        },
    });
    let mut bytes = serde_json::to_vec(&payload).expect("progress serializes");
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

fn execute_boolean_pattern(
    arguments: Value,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(&threeterm_protocol::supervisor::Progress),
    configured_worker: Option<&OcctWorker>,
) -> Result<Value, HostError> {
    let bundle = arguments
        .get("bundle_path")
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::Validation {
            detail: "missing bundle_path".to_string(),
        })?;
    let feature_id = arguments
        .get("feature_id")
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::Validation {
            detail: "missing feature_id".to_string(),
        })?;
    let base_feature_id = arguments
        .get("base_feature_id")
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::Validation {
            detail: "missing base_feature_id".to_string(),
        })?;
    let origin: [f64; 3] =
        serde_json::from_value(arguments["origin"].clone()).map_err(|error| {
            HostError::Validation {
                detail: format!("invalid origin: {error}"),
            }
        })?;
    let spacing: [f64; 2] =
        serde_json::from_value(arguments["spacing"].clone()).map_err(|error| {
            HostError::Validation {
                detail: format!("invalid spacing: {error}"),
            }
        })?;
    let columns = arguments["columns"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| HostError::Validation {
            detail: "invalid columns".to_string(),
        })?;
    let rows = arguments["rows"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| HostError::Validation {
            detail: "invalid rows".to_string(),
        })?;
    let diameter = arguments["diameter"]
        .as_f64()
        .ok_or_else(|| HostError::Validation {
            detail: "invalid diameter".to_string(),
        })?;
    if !valid_feature_path_component(feature_id) || !valid_feature_path_component(base_feature_id) {
        return Err(HostError::Validation {
            detail: "boolean pattern feature IDs must be plain path components".to_string(),
        });
    }
    let root = Bundle::at(bundle).canonical_root().to_path_buf();
    let request = BooleanPatternRequest::new(
        new_request_id(),
        root.join(BREP_SUBDIR)
            .join(format!("{base_feature_id}.brep")),
        origin,
        spacing,
        columns,
        rows,
        diameter,
    )
    .with_output_path(root.join("stage"), "boolean-pattern.brep")
    .with_feature_id(feature_id);
    let located_worker;
    let worker = if let Some(worker) = configured_worker {
        worker
    } else {
        located_worker = OcctWorker::locate().map_err(|error| HostError::WorkerUnavailable {
            detail: error.to_string(),
        })?;
        &located_worker
    };
    let value = Host::new()
        .boolean_pattern_with_cancel_and_progress(bundle, request, worker, cancel, on_progress)?
        .response_value(threeterm_protocol::schema::BOOLEAN_PATTERN_RESPONSE_SCHEMA_VERSION);
    validate(
        &threeterm_protocol::schema::BOOLEAN_PATTERN_RESPONSE_SCHEMA,
        &value,
    )
    .map_err(|reason| HostError::Validation {
        detail: format!("boolean pattern response failed schema validation: {reason}"),
    })?;
    Ok(value)
}

fn host_tool_execution_error(error: &HostError) -> Value {
    let diagnostic =
        serde_json::to_value(host_error_diagnostic(error)).expect("diagnostic serializes");
    let text = serde_json::to_string(&diagnostic).expect("diagnostic text serializes");
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": diagnostic,
        "isError": true,
    })
}

fn bracket_edit_failure_response(arguments: &Value, error: &HostError) -> Value {
    let mut source_revision = arguments
        .get("source_revision")
        .and_then(Value::as_str)
        .filter(|revision| revision.len() == 64)
        .unwrap_or("0000000000000000000000000000000000000000000000000000000000000000")
        .to_string();
    let phase = arguments
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let draft_id = arguments
        .get("draft_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut diagnostic = json!({
        "kind": "bracket_edit_rejected",
        "draft_id": draft_id,
        "idempotency_key": draft_id,
        "detail": error.to_string(),
        "recovery": "inspect_diagnostic_and_reopen",
    });
    if matches!(error, HostError::DraftAlreadyExists { .. }) {
        let current_revision = arguments
            .get("bundle_path")
            .and_then(Value::as_str)
            .and_then(|bundle| Bundle::at(bundle).open().ok())
            .map(|loaded| loaded.revision_hash_hex().to_string())
            .unwrap_or_else(|| source_revision.clone());
        diagnostic["kind"] = Value::String("draft_input_conflict".to_string());
        diagnostic["source_revision"] = Value::String(current_revision.clone());
        diagnostic["current_revision"] = Value::String(current_revision);
        diagnostic["recovery"] = Value::String("use_update_or_refresh_draft".to_string());
    }
    let status = if let HostError::DraftUnknownOutcome {
        source_revision: authoritative_source,
        current_revision,
        recovery,
        ..
    } = error
    {
        diagnostic["kind"] = Value::String("bracket_edit_unknown_outcome".to_string());
        diagnostic["recovery"] = Value::String((*recovery).to_string());
        diagnostic["source_revision"] = Value::String(authoritative_source.clone());
        diagnostic["current_revision"] = Value::String(current_revision.clone());
        source_revision = authoritative_source.clone();
        "unknown"
    } else {
        "rejected"
    };
    if let HostError::DraftSequenceConflict {
        expected, current, ..
    } = error
    {
        diagnostic["kind"] = Value::String("draft_sequence_conflict".to_string());
        diagnostic["expected_sequence"] = Value::from(*expected);
        diagnostic["current_sequence"] = Value::from(*current);
        diagnostic["recovery"] = Value::String("refresh_draft_and_retry".to_string());
    }
    if let HostError::DraftIdempotencyConflict {
        source_revision: authoritative_source,
        recovery,
        ..
    } = error
    {
        diagnostic["kind"] = Value::String("draft_idempotency_conflict".to_string());
        diagnostic["source_revision"] = Value::String(authoritative_source.clone());
        diagnostic["recovery"] = Value::String((*recovery).to_string());
        source_revision = authoritative_source.clone();
    }
    if let HostError::DraftInputConflict {
        source_revision: authoritative_source,
        recovery,
        ..
    } = error
    {
        diagnostic["kind"] = Value::String("draft_input_conflict".to_string());
        diagnostic["source_revision"] = Value::String(authoritative_source.clone());
        diagnostic["recovery"] = Value::String((*recovery).to_string());
        source_revision = authoritative_source.clone();
    }
    let current_revision = match error {
        HostError::DraftStale {
            source_revision: authoritative_source,
            current_revision,
            recovery,
            ..
        }
        | HostError::DraftSourceChanged {
            source_revision: authoritative_source,
            current_revision,
            recovery,
            ..
        }
        | HostError::DraftIdempotencyConflict {
            source_revision: authoritative_source,
            current_revision,
            recovery,
            ..
        }
        | HostError::DraftInputConflict {
            source_revision: authoritative_source,
            current_revision,
            recovery,
            ..
        } => {
            source_revision = authoritative_source.clone();
            diagnostic["source_revision"] = Value::String(authoritative_source.clone());
            diagnostic["recovery"] = Value::String((*recovery).to_string());
            Some(current_revision.as_str())
        }
        HostError::DraftUnknownOutcome {
            source_revision: authoritative_source,
            current_revision,
            recovery,
            ..
        } => {
            source_revision = authoritative_source.clone();
            diagnostic["source_revision"] = Value::String(authoritative_source.clone());
            diagnostic["recovery"] = Value::String((*recovery).to_string());
            Some(current_revision.as_str())
        }
        _ => None,
    };
    if let Some(current_revision) = current_revision {
        diagnostic["current_revision"] = Value::String(current_revision.to_string());
    }
    // An open failure is always a conflict with the draft already being edited.
    if phase == "open" {
        let current_revision = arguments
            .get("bundle_path")
            .and_then(Value::as_str)
            .and_then(|bundle| Bundle::at(bundle).open().ok())
            .map(|loaded| loaded.revision_hash_hex().to_string())
            .unwrap_or_else(|| source_revision.clone());
        diagnostic["kind"] = Value::String("draft_input_conflict".to_string());
        diagnostic["source_revision"] = Value::String(source_revision.clone());
        diagnostic["current_revision"] = Value::String(current_revision);
        diagnostic["recovery"] = Value::String("use_update_or_refresh_draft".to_string());
    }
    let mut response = bracket_edit_response(
        phase,
        draft_id,
        &source_revision,
        current_revision,
        None,
        None,
        None,
    );
    response["status"] = Value::String(status.to_string());
    response["diagnostic"] = diagnostic;
    response
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&value).expect("JSON values serialize");
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
    });
    if value.is_object() {
        result["structuredContent"] = value;
    }
    if is_error {
        result["isError"] = Value::Bool(true);
    }
    result
}

fn tool_execution_error(message: String) -> Value {
    json!({
        "content": [{"type": "text", "text": message}],
        "isError": true,
    })
}

fn bracket_edit_response(
    phase: &str,
    draft_id: &str,
    source_revision: &str,
    current_revision: Option<&str>,
    preview_revision: Option<&str>,
    input_fingerprint: Option<&str>,
    draft_sequence: Option<u64>,
) -> Value {
    let mut response = json!({
        "status": "ok",
        "phase": phase,
        "draft_id": draft_id,
        "source_revision": source_revision,
        "schema_version": threeterm_protocol::schema::BRACKET_EDIT_RESPONSE_SCHEMA_VERSION,
    });
    if let Some(preview_revision) = preview_revision {
        response["preview_revision"] = Value::String(preview_revision.to_string());
    }
    if let Some(current_revision) = current_revision {
        response["current_revision"] = Value::String(current_revision.to_string());
    }
    if let Some(input_fingerprint) = input_fingerprint {
        response["input_fingerprint"] = Value::String(input_fingerprint.to_string());
    }
    if let Some(draft_sequence) = draft_sequence {
        response["draft_sequence"] = Value::from(draft_sequence);
    }
    response
}

fn dispatch_bracket_tool(arguments: &Value) -> Result<Value, DispatchError> {
    let bundle = arguments
        .get("bundle_path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let bracket_id = arguments
        .get("bracket_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let length = arguments
        .get("length")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let width = arguments
        .get("width")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let height = arguments
        .get("height")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let thickness = arguments
        .get("thickness")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let view = dispatch_bracket(bundle, bracket_id, length, width, height, thickness)?;
    Ok(json!({
        "status": view.result.status,
        "operation": "bracket",
        "feature_id": view.result.feature_id,
        "request_id": view.result.request_id,
        "source_snapshot": {
            "feature_graph_hash": view.source_snapshot.feature_graph_hash,
            "revision_hash": view.source_snapshot.revision_hash,
        },
        "feature_graph_hash": view.snapshot.feature_graph_hash,
        "revision_hash": view.snapshot.revision_hash,
        "authoritative": true,
        "artifact_kind": view.artifact.artifact_kind,
        "artifact_name": view.artifact.artifact_name,
        "brep_path": view.result.brep_path,
        "brep_sha256": view.result.brep_sha256,
        "brep_bytes": view.result.brep_bytes,
        "worker_fingerprint": {
            "worker_kind": view.artifact.worker_fingerprint.worker_kind,
            "worker_schema_version": view.artifact.worker_fingerprint.worker_schema_version,
            "protocol_schema_version": view.artifact.worker_fingerprint.protocol_schema_version,
        },
        "derived_result": {
            "request_id": view.artifact.request_id,
            "operation": view.artifact.operation,
            "feature_id": view.artifact.feature_id,
            "source_revision_id": view.source_snapshot.revision_hash,
            "worker_fingerprint": {
                "worker_kind": view.artifact.worker_fingerprint.worker_kind,
                "worker_schema_version": view.artifact.worker_fingerprint.worker_schema_version,
                "protocol_schema_version": view.artifact.worker_fingerprint.protocol_schema_version,
            },
            "artifact_kind": view.artifact.artifact_kind,
            "artifact_name": view.artifact.artifact_name,
            "byte_count": view.artifact.byte_count,
            "sha256": view.artifact.sha256,
        },
        "schema_version": threeterm_protocol::schema::BRACKET_RESPONSE_SCHEMA_VERSION,
    }))
}

fn find_by_wire_name(name: &str) -> Option<&'static CommandSchema> {
    iter().find(|schema| schema.schema_version == name)
}

fn parse_request(value: &Value) -> Result<JsonRpcRequest, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "request must be a JSON object".to_string())?;
    let jsonrpc = object
        .get("jsonrpc")
        .and_then(Value::as_str)
        .ok_or_else(|| "request is missing jsonrpc field".to_string())?;
    if jsonrpc != JSONRPC_VERSION {
        return Err(format!(
            "request jsonrpc must be {JSONRPC_VERSION:?}, got {jsonrpc:?}"
        ));
    }
    if let Some(id) = object.get("id")
        && !is_valid_request_id(id)
    {
        return Err("request id must be a string, number, or null".to_string());
    }
    if let Some(params) = object.get("params")
        && !params.is_object()
        && !params.is_array()
    {
        return Err("request params must be an object or array".to_string());
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "request is missing method field".to_string())?
        .to_string();
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    Ok(JsonRpcRequest {
        id,
        is_notification: object.get("id").is_none(),
        method,
        params,
    })
}

fn extract_id(value: &Value) -> Value {
    value
        .as_object()
        .and_then(|object| object.get("id"))
        .filter(|id| is_valid_request_id(id))
        .cloned()
        .unwrap_or(Value::Null)
}

fn is_valid_request_id(value: &Value) -> bool {
    value.is_string() || value.as_i64().is_some() || value.as_u64().is_some()
}

fn read_bounded_frame<R: BufRead>(reader: &mut R, buffer: &mut Vec<u8>) -> std::io::Result<usize> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(buffer.len());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let length = newline.map_or(available.len(), |index| index + 1);
        if buffer.len() + length > MAX_FRAME_BUFFER {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "frame buffer exceeded maximum size: {} > {}",
                    buffer.len() + length,
                    MAX_FRAME_BUFFER
                ),
            ));
        }
        buffer.extend_from_slice(&available[..length]);
        reader.consume(length);
        if newline.is_some() {
            return Ok(buffer.len());
        }
    }
}

fn split_newlines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(&bytes[start..index]);
            start = index + 1;
        }
    }
    if start < bytes.len() {
        lines.push(&bytes[start..]);
    }
    lines
}

fn write_envelope<W: Write>(writer: &mut W, response: &JsonRpcResponse) -> std::io::Result<()> {
    let payload = json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": response.id,
    });
    let mut object = match payload {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    match (&response.result, &response.error) {
        (Some(result), None) => {
            object.insert("result".to_string(), result.clone());
        }
        (None, Some(error)) => {
            object.insert(
                "error".to_string(),
                serde_json::to_value(error).expect("error serializes"),
            );
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "response has neither result nor error",
            ));
        }
    }
    let mut bytes = serde_json::to_vec(&Value::Object(object)).expect("response serializes");
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

#[doc(hidden)]
pub fn _exit_ok_for_test() -> i32 {
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_envelope(id: Value, method: &str, params: Value) -> Value {
        let mut object = Map::new();
        object.insert(
            "jsonrpc".to_string(),
            Value::String(JSONRPC_VERSION.to_string()),
        );
        object.insert("id".to_string(), id);
        object.insert("method".to_string(), Value::String(method.to_string()));
        object.insert("params".to_string(), params);
        Value::Object(object)
    }

    #[test]
    fn advertise_tools_emits_one_entry_per_registered_command() {
        let server = McpServer::new();
        let tools = server.advertise_tools();
        let bracket = tools
            .iter()
            .find(|tool| tool.name == "threeterm.command.bracket/1")
            .expect("bracket is advertised");
        assert!(bracket.input_schema.is_object());
        assert!(bracket.output_schema.as_ref().is_some_and(Value::is_object));
        assert_eq!(
            bracket.input_schema["required"],
            json!([
                "bundle_path",
                "bracket_id",
                "length",
                "width",
                "height",
                "thickness"
            ])
        );
    }

    #[test]
    fn handle_request_dispatches_tools_list_to_the_advertised_payload() {
        let server = McpServer::new();
        let request = JsonRpcRequest {
            id: Value::Number(1.into()),
            is_notification: false,
            method: "tools/list".to_string(),
            params: Value::Null,
        };
        let response = server.handle_request(&request);
        let result = response.result.expect("tools/list is success");
        let tools = result["tools"].as_array().expect("tools is an array");
        assert!(
            tools
                .iter()
                .any(|tool| tool["name"] == "threeterm.command.bracket/1")
        );
    }

    #[test]
    fn bracket_edit_open_returns_a_schema_validated_structured_lifecycle_result() {
        let server = McpServer::new();
        let request = JsonRpcRequest {
            id: Value::Number(1.into()),
            is_notification: false,
            method: "tools/call".to_string(),
            params: json!({
                "name": "threeterm.command.bracket-edit/1",
                "arguments": {
                    "phase": "open",
                    "bundle_path": "/tmp/missing-bracket-edit-bundle",
                    "draft_id": "draft-1",
                    "bracket_id": "l-bracket",
                    "length": 100.0,
                    "width": 60.0,
                    "height": 40.0,
                    "thickness": 5.0
                }
            }),
        };
        let response = server.handle_request(&request);
        let result = response
            .result
            .expect("lifecycle failures remain structured");
        validate(
            &threeterm_protocol::schema::BRACKET_EDIT_RESPONSE_SCHEMA,
            &result["structuredContent"],
        )
        .expect("lifecycle response satisfies its registered schema");
        assert_eq!(result["structuredContent"]["status"], "rejected");
        assert_eq!(
            result["structuredContent"]["diagnostic"]["idempotency_key"],
            "draft-1"
        );
    }

    #[test]
    fn handle_request_rejects_unknown_method_with_method_not_found_code() {
        let server = McpServer::new();
        let request = JsonRpcRequest {
            id: Value::Number(1.into()),
            is_notification: false,
            method: "tools/not-a-method".to_string(),
            params: Value::Null,
        };
        let response = server.handle_request(&request);
        let error = response.error.expect("unknown method is an error");
        assert_eq!(error.code, ERROR_METHOD_NOT_FOUND);
        assert!(error.message.contains("tools/not-a-method"));
    }

    #[test]
    fn handle_request_rejects_unknown_tool_with_method_not_found_code() {
        let server = McpServer::new();
        let request = JsonRpcRequest {
            id: Value::Number(1.into()),
            is_notification: false,
            method: "tools/call".to_string(),
            params: json!({
                "name": "threeterm.command.does-not-exist/1",
                "arguments": {}
            }),
        };
        let response = server.handle_request(&request);
        let error = response.error.expect("unknown tool is an error");
        assert_eq!(error.code, ERROR_METHOD_NOT_FOUND);
    }

    #[test]
    fn handle_request_rejects_invalid_bracket_arguments_with_invalid_params_code() {
        let server = McpServer::new();
        let request = JsonRpcRequest {
            id: Value::Number(7.into()),
            is_notification: false,
            method: "tools/call".to_string(),
            params: json!({
                "name": "threeterm.command.bracket/1",
                "arguments": {
                    "bundle_path": "/tmp/whatever",
                    "bracket_id": "l-1",
                    "length": "not-a-number",
                    "width": 30.0,
                    "height": 40.0,
                    "thickness": 3.0
                }
            }),
        };
        let response = server.handle_request(&request);
        let error = response.error.expect("invalid arguments is an error");
        assert_eq!(error.code, ERROR_INVALID_PARAMS);
        assert!(error.message.contains("length"));
    }

    #[test]
    fn run_writes_a_tools_list_envelope_and_returns_count() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n".to_vec();
        let mut output = Vec::new();
        let server = McpServer::new();
        let handled = server
            .run(&mut input.as_slice(), &mut output)
            .expect("run succeeds");
        assert_eq!(handled, 1);
        let parsed: Value = serde_json::from_slice(&output).expect("response is JSON");
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert!(parsed["result"]["tools"].is_array());
    }

    #[test]
    fn run_writes_an_error_envelope_for_a_malformed_frame() {
        let input = b"this is not json\n".to_vec();
        let mut output = Vec::new();
        let server = McpServer::new();
        let _ = server
            .run(&mut input.as_slice(), &mut output)
            .expect("run survives a malformed frame");
        let parsed: Value = serde_json::from_slice(&output).expect("response is JSON");
        assert_eq!(parsed["error"]["code"], ERROR_PARSE);
    }

    #[test]
    fn build_envelope_round_trips_through_parse_request() {
        let value = build_envelope(Value::Number(1.into()), "tools/list", json!({}));
        let request = parse_request(&value).expect("build_envelope parses");
        assert_eq!(request.method, "tools/list");
        assert_eq!(request.id, Value::Number(1.into()));
        assert!(!request.is_notification);
    }

    #[test]
    fn read_bounded_frame_rejects_an_unterminated_oversized_frame_before_growth() {
        let input = vec![b'x'; MAX_FRAME_BUFFER + 1];
        let mut buffer = Vec::new();
        let error = read_bounded_frame(&mut input.as_slice(), &mut buffer)
            .expect_err("oversized frame must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(buffer.len() <= MAX_FRAME_BUFFER);
    }

    #[test]
    fn boolean_pattern_rejects_feature_ids_that_escape_the_bundle() {
        let arguments = json!({
            "bundle_path": "/tmp/project",
            "feature_id": "pattern",
            "base_feature_id": "../outside",
            "origin": [0.0, 0.0, 0.0],
            "spacing": [1.0, 1.0],
            "columns": 1,
            "rows": 1,
            "diameter": 1.0
        });
        let cancel = AtomicBool::new(false);
        let mut on_progress = |_progress: &threeterm_protocol::supervisor::Progress| {};
        let error = execute_boolean_pattern(arguments, &cancel, &mut on_progress, None)
            .expect_err("path traversal must fail before worker lookup");
        assert!(matches!(error, HostError::Validation { .. }));
    }

    #[test]
    fn run_does_not_write_responses_for_notifications() {
        let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/list\"}\n";
        let mut output = Vec::new();
        let server = McpServer::new();
        let handled = server
            .run(&mut input.as_slice(), &mut output)
            .expect("run succeeds");

        assert_eq!(handled, 2);
        let responses: Vec<Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("response is JSON"))
            .collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], 7);
    }

    #[test]
    fn initialize_rejects_missing_required_client_parameters() {
        let server = McpServer::new();
        let response = server.handle_request(&JsonRpcRequest {
            id: Value::Number(1.into()),
            is_notification: false,
            method: "initialize".to_string(),
            params: json!({}),
        });

        assert_eq!(
            response.error.expect("invalid initialize is an error").code,
            ERROR_INVALID_PARAMS
        );
    }

    #[test]
    fn initialize_selects_the_server_protocol_version_when_client_requests_another_version() {
        let response = McpServer::new().handle_request(&JsonRpcRequest {
            id: Value::Number(1.into()),
            is_notification: false,
            method: "initialize".to_string(),
            params: json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "fixture", "version": "0"}
            }),
        });

        assert_eq!(
            response.result.expect("version negotiation succeeds")["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
    }

    #[test]
    fn null_id_is_rejected_as_an_invalid_request() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"tools/list\"}\n";
        let mut output = Vec::new();
        McpServer::new()
            .run(&mut input.as_slice(), &mut output)
            .expect("run succeeds");

        let response: Value = serde_json::from_slice(&output).expect("response is JSON");
        assert!(response["id"].is_null());
        assert_eq!(response["error"]["code"], ERROR_INVALID_REQUEST);
    }

    #[test]
    fn fractional_id_is_rejected_as_an_invalid_request() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1.5,\"method\":\"tools/list\"}\n";
        let mut output = Vec::new();
        McpServer::new()
            .run(&mut input.as_slice(), &mut output)
            .expect("run succeeds");

        let response: Value = serde_json::from_slice(&output).expect("response is JSON");
        assert_eq!(response["error"]["code"], ERROR_INVALID_REQUEST);
        assert!(response["id"].is_null());
    }

    #[test]
    fn invalid_id_type_returns_an_invalid_request_with_null_id() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"tools/list\"}\n";
        let mut output = Vec::new();
        McpServer::new()
            .run(&mut input.as_slice(), &mut output)
            .expect("run succeeds");

        let response: Value = serde_json::from_slice(&output).expect("response is JSON");
        assert_eq!(response["error"]["code"], ERROR_INVALID_REQUEST);
        assert!(response["id"].is_null());
    }

    #[test]
    fn tool_results_use_text_content_for_structured_domain_values() {
        let root = std::env::temp_dir().join(format!("threeterm-mcp-content-{}", new_request_id()));
        Bundle::create(&root).expect("bundle creates");
        let response = McpServer::new().handle_request(&JsonRpcRequest {
            id: Value::Number(1.into()),
            is_notification: false,
            method: "tools/call".to_string(),
            params: json!({
                "name": "threeterm.command.identity/1",
                "arguments": {"bundle_path": root.to_string_lossy()}
            }),
        });

        let result = response.result.expect("identity is a tool result");
        assert_eq!(result["content"][0]["type"], "text");
        let content: Value = serde_json::from_str(
            result["content"][0]["text"]
                .as_str()
                .expect("text content is a string"),
        )
        .expect("text content contains JSON");
        assert_eq!(content, result["structuredContent"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn run_streams_bounded_progress_and_cancels_an_active_boolean_pattern() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("threeterm-mcp-progress-{}", new_request_id()));
        Bundle::create(&root).expect("bundle creates");
        let worker_path = root.join("worker.sh");
        std::fs::write(
            &worker_path,
            r##"#!/bin/sh
printf '%s\n' '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"occt"}'
read request
rid=$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
printf '%s\n' '{"kind":"progress","schema_version":"threeterm.protocol/1","request_id":"'$rid'","stage":"boolean_pattern:1/324","percent":1}'
read cancel
printf '%s\n' '{"kind":"cancelled","schema_version":"threeterm.protocol/1","request_id":"'$rid'","reason":"cancelled by client"}'
"##,
        )
        .expect("worker script writes");
        let mut permissions = std::fs::metadata(&worker_path)
            .expect("worker metadata reads")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&worker_path, permissions).expect("worker is executable");

        let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
        let log_before = std::fs::read(root.join("transactions.log")).expect("log reads");
        let call = json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": "call-1",
            "method": "tools/call",
            "params": {
                "name": "threeterm.command.boolean-pattern/1",
                "_meta": {"progressToken": "progress-1"},
                "arguments": {
                    "bundle_path": root.to_string_lossy(),
                    "feature_id": "pattern",
                    "base_feature_id": "missing-base",
                    "origin": [0.0, 0.0, 0.0],
                    "spacing": [1.0, 1.0],
                    "columns": 18,
                    "rows": 18,
                    "diameter": 1.0
                }
            }
        });
        let cancel = json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": "notifications/cancelled",
            "params": {"requestId": "call-1", "reason": "stop"}
        });
        assert_eq!(
            progress_token(&parse_request(&call).expect("call parses")),
            Some(json!("progress-1"))
        );
        let mut input = serde_json::to_vec(&call).expect("call serializes");
        input.push(b'\n');
        input.extend(serde_json::to_vec(&cancel).expect("cancel serializes"));
        input.push(b'\n');
        let mut output = Vec::new();
        McpServer::new()
            .with_boolean_pattern_worker(
                OcctWorker::with_binary_path(worker_path).with_expected_worker_id("occt"),
            )
            .run(&mut input.as_slice(), &mut output)
            .expect("MCP run succeeds");

        let responses: Vec<Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("MCP output is JSON"))
            .collect();
        assert!(
            responses
                .iter()
                .any(|response| response["method"] == "notifications/progress"
                    && response["params"]["progressToken"] == "progress-1"),
            "active workers must stream progress: {responses:?}"
        );
        let result = responses
            .iter()
            .find(|response| response["id"] == "call-1")
            .expect("call has a terminal response");
        assert_eq!(result["result"]["isError"], true);
        assert_eq!(
            result["result"]["structuredContent"]["code"],
            "worker_failure"
        );
        assert!(
            result["result"]["structuredContent"]["arg"]
                .as_str()
                .expect("diagnostic arg is text")
                .contains("boolean_pattern:1/324")
        );
        assert_eq!(
            std::fs::read(root.join("manifest.json")).expect("manifest reads after cancellation"),
            manifest_before
        );
        assert_eq!(
            std::fs::read(root.join("transactions.log")).expect("log reads after cancellation"),
            log_before
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

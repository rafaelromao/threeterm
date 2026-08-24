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

use std::io::{BufRead, Write};

use serde::Serialize;
use serde_json::{Map, Value, json};
use threeterm_cli::dispatch::{DispatchError, EXIT_OK, dispatch_registered_command};
use threeterm_host::Host;
use threeterm_protocol::frame::MAX_FRAME_BUFFER;
use threeterm_protocol::schema::{CommandSchema, iter};
use threeterm_protocol::schema_validator::validate;

pub const JSONRPC_VERSION: &str = "2.0";

pub const ERROR_INVALID_REQUEST: i32 = -32600;
pub const ERROR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERROR_INVALID_PARAMS: i32 = -32602;
pub const ERROR_INTERNAL: i32 = -32603;
pub const ERROR_PARSE: i32 = -32700;

/// Tool descriptor exposed by `tools/list`. The wire shape follows the MCP
/// tool advertisement convention with `inputSchema` and `outputSchema`
/// populated from the protocol registry row.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(rename = "outputSchema")]
    pub output_schema: Value,
}

impl ToolDescriptor {
    fn from_schema(schema: &CommandSchema) -> Self {
        Self {
            name: schema.schema_version.to_string(),
            description: "ThreeTerm versioned domain command (see threeterm_protocol::schema).",
            input_schema: schema.request_schema.clone(),
            output_schema: schema.response_schema.clone(),
        }
    }
}

/// One JSON-RPC 2.0 request. The `params` field is kept as a generic
/// `Value` so the dispatcher can validate the inner `arguments` object
/// against the registered request schema.
#[derive(Debug, Clone)]
pub struct JsonRpcRequest {
    pub id: Value,
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
#[derive(Debug, Default)]
pub struct McpServer {}

impl McpServer {
    pub fn new() -> Self {
        Self::default()
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
        JsonRpcResponse::success(
            request.id.clone(),
            json!({
                "protocolVersion": "2024-11-05",
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

        if let Err(reason) = validate(&schema_entry.request_schema, &arguments) {
            return JsonRpcResponse::error(
                request.id.clone(),
                ERROR_INVALID_PARAMS,
                format!("tools/call arguments failed request-schema validation: {reason}"),
            );
        }

        let result = dispatch_registered_command(&Host::new(), schema_entry.id, arguments);

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
                let envelope = json!({
                    "content": [{
                        "type": "json",
                        "data": value.clone(),
                    }],
                    "structuredContent": value,
                });
                JsonRpcResponse::success(request.id.clone(), envelope)
            }
            Err(error) => match error {
                DispatchError::UnsupportedTool { .. } => JsonRpcResponse::error(
                    request.id.clone(),
                    ERROR_METHOD_NOT_FOUND,
                    format!("{error}"),
                ),
                DispatchError::Host(_)
                | DispatchError::Validation(_)
                | DispatchError::UnknownCommand(_) => JsonRpcResponse::error(
                    request.id.clone(),
                    ERROR_INTERNAL,
                    format!("host dispatch failed: {error}"),
                ),
            },
        }
    }

    /// Drive the newline-framed JSON-RPC 2.0 loop over `reader`/`writer`.
    /// Returns the number of requests handled. The function returns when
    /// `reader` hits EOF or a malformed frame aborts the current chunk; in
    /// the latter case the buffered bytes are dropped so the supervisor
    /// can resync on the next chunk (closed issue #49 contract).
    pub fn run<R: BufRead, W: Write>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<usize, std::io::Error> {
        let mut buffer = Vec::with_capacity(4096);
        let mut handled = 0usize;
        loop {
            buffer.clear();
            let read = reader.read_until(b'\n', &mut buffer)?;
            if read == 0 {
                return Ok(handled);
            }
            if buffer.len() > MAX_FRAME_BUFFER {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "frame buffer exceeded maximum size: {} > {}",
                        buffer.len(),
                        MAX_FRAME_BUFFER
                    ),
                ));
            }
            let raw_lines = split_newlines(&buffer);
            for line in raw_lines {
                if line.is_empty() {
                    continue;
                }
                let parsed: Result<Value, _> = serde_json::from_slice(line);
                let value = match parsed {
                    Ok(value) => value,
                    Err(error) => {
                        let response = JsonRpcResponse::error(
                            Value::Null,
                            ERROR_PARSE,
                            format!("frame is not valid JSON: {error}"),
                        );
                        write_envelope(writer, &response)?;
                        continue;
                    }
                };
                let request = match parse_request(&value) {
                    Ok(request) => request,
                    Err(error) => {
                        let response = JsonRpcResponse::error(
                            extract_id(&value),
                            ERROR_INVALID_REQUEST,
                            error,
                        );
                        write_envelope(writer, &response)?;
                        continue;
                    }
                };
                let response = self.handle_request(&request);
                write_envelope(writer, &response)?;
                handled += 1;
            }
        }
    }
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
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "request is missing method field".to_string())?
        .to_string();
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    Ok(JsonRpcRequest { id, method, params })
}

fn extract_id(value: &Value) -> Value {
    value
        .as_object()
        .and_then(|object| object.get("id"))
        .cloned()
        .unwrap_or(Value::Null)
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
        assert!(bracket.output_schema.is_object());
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
    fn handle_request_rejects_unknown_method_with_method_not_found_code() {
        let server = McpServer::new();
        let request = JsonRpcRequest {
            id: Value::Number(1.into()),
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
        let value = build_envelope(Value::Number(1.into()), "tools/list", Value::Null);
        let request = parse_request(&value).expect("build_envelope parses");
        assert_eq!(request.method, "tools/list");
        assert_eq!(request.id, Value::Number(1.into()));
    }
}

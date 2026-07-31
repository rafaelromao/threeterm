//! Versioned domain command schema registry.
//!
//! The registry is a single static table keyed by `CommandId`. Each entry
//! carries one versioned JSON Schema for the request and one for the
//! response. The TUI, CLI, and MCP adapters all read from this same table;
//! only the framing differs (closed issue #33).
//!
//! The registry is read-only and pure. Adding a new command is a single
//! static entry in `COMMAND_REGISTRY`; the dispatcher, serializer, and
//! schema hash pick it up automatically.

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

/// Stable, presentation-neutral identifier for a registered command.
///
/// Wrapped in a newtype so the registry can be keyed by `CommandId` rather
/// than by topology indexes or in-process kernel object identity (closed
/// issue #23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CommandId(pub &'static str);

/// One row of the static command registry. The `request_schema` and
/// `response_schema` fields are JSON Schema documents stored as generic
/// `serde_json::Value`s so the schema can evolve without a schema-of-schema
/// dependency.
#[derive(Debug, Clone, Serialize)]
pub struct CommandSchema {
    pub id: CommandId,
    pub name: &'static str,
    pub schema_version: &'static str,
    pub request_schema: Value,
    pub response_schema: Value,
}

/// Canonical request schema document for the `list` command.
pub static LIST_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
});

/// Canonical response schema document for the `list` command.
pub static LIST_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["schema_version", "commands"],
        "properties": {
            "schema_version": {
                "type": "string",
                "description": "Version of the listing envelope itself."
            },
            "commands": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": [
                        "id",
                        "name",
                        "schema_version",
                        "request_schema",
                        "response_schema"
                    ],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "schema_version": { "type": "string" },
                        "request_schema": { "type": "object" },
                        "response_schema": { "type": "object" }
                    }
                }
            }
        }
    })
});

/// The single static command registry. The slice (#233) seeds one entry;
/// later slices extend this table.
pub static COMMAND_REGISTRY: LazyLock<Vec<CommandSchema>> = LazyLock::new(|| {
    vec![CommandSchema {
        id: CommandId("list"),
        name: "list",
        schema_version: "threeterm.command.list/1",
        request_schema: LIST_REQUEST_SCHEMA.clone(),
        response_schema: LIST_RESPONSE_SCHEMA.clone(),
    }]
});

/// Reserve a stable id for the `list` command so adapters can reference it
/// without depending on the entry's table position.
pub const LIST_COMMAND_ID: CommandId = CommandId("list");

/// SHA-256 hex digest of the canonical JSON encoding of `COMMAND_REGISTRY`.
///
/// The canonical encoding sorts object keys recursively and uses no
/// whitespace, so the byte order is deterministic across builds and
/// platforms. The returned string is 64 lowercase hex characters.
pub fn registry_hash() -> String {
    let mut hasher = Sha256::new();

    for entry in COMMAND_REGISTRY.iter() {
        let serialized = serde_json::to_vec(entry).expect("entry serializes");
        hasher.update(serialized);
        // Separate entries with a newline so two adjacent entries do not
        // produce ambiguous concatenated bytes.
        hasher.update(b"\n");
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

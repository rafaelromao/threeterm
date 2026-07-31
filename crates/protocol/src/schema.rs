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

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Stable, presentation-neutral identifier for a registered command.
///
/// Wrapped in a newtype so the registry can be keyed by `CommandId` rather
/// than by topology indexes or in-process kernel object identity (closed
/// issue #23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CommandId(pub &'static str);

/// One row of the static command registry. The `request_schema` and
/// `response_schema` fields are JSON Schema documents stored as generic
/// `serde_json::Value`s so the schema can evolve without a schema-of-schema
/// dependency. Each schema has its own version independent of the
/// command-level `schema_version` (closed issue #51: one versioned domain
/// command schema per request and per response).
#[derive(Debug, Clone, Serialize)]
pub struct CommandSchema {
    pub id: CommandId,
    pub name: &'static str,
    pub schema_version: &'static str,
    pub request_schema_version: &'static str,
    pub request_schema: Value,
    pub response_schema_version: &'static str,
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
        "type": "array",
        "description": "Array of every registered command with its schema version.",
        "items": {
            "type": "object",
            "required": [
                "id",
                "name",
                "schema_version",
                "request_schema_version",
                "request_schema",
                "response_schema_version",
                "response_schema"
            ],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "schema_version": { "type": "string" },
                "request_schema_version": { "type": "string" },
                "request_schema": { "type": "object" },
                "response_schema_version": { "type": "string" },
                "response_schema": { "type": "object" }
            }
        }
    })
});

/// Canonical request schema document for the `save` command.
pub static SAVE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "description": "Save a one-feature transaction to a sealed bundle.",
        "required": ["bundle_path", "feature_id", "kind"],
        "additionalProperties": false,
        "properties": {
            "bundle_path": { "type": "string" },
            "feature_id": { "type": "string" },
            "kind": { "type": "string" }
        }
    })
});

/// Canonical response schema document for the `save` and `load` commands.
pub static SNAPSHOT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "description": "Canonical feature-graph hash and revision hash with the response's own schema version.",
        "required": ["feature_graph_hash", "revision_hash", "schema_version"],
        "properties": {
            "feature_graph_hash": {
                "type": "string",
                "description": "Lowercase-hex SHA-256 of the canonical feature graph."
            },
            "revision_hash": {
                "type": "string",
                "description": "Lowercase-hex SHA-256 of feature_graph_hash || terminal_log_digest_hex."
            },
            "schema_version": { "type": "string" }
        }
    })
});

/// Canonical request schema document for the `load` command.
pub static LOAD_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "description": "Integrity-verify a bundle and emit its canonical snapshot.",
        "required": ["bundle_path"],
        "additionalProperties": false,
        "properties": {
            "bundle_path": { "type": "string" }
        }
    })
});

/// The single static command registry, keyed by `CommandId`. The slice
/// (#233) seeds one entry; later slices extend this table.
pub static COMMAND_REGISTRY: LazyLock<BTreeMap<CommandId, CommandSchema>> =
    LazyLock::new(|| {
        let mut map = BTreeMap::new();
        map.insert(
            LIST_COMMAND_ID,
            CommandSchema {
                id: LIST_COMMAND_ID,
                name: "list",
                schema_version: "threeterm.command.list/1",
                request_schema_version: "threeterm.command.list.request/1",
                request_schema: LIST_REQUEST_SCHEMA.clone(),
                response_schema_version: "threeterm.command.list.response/1",
                response_schema: LIST_RESPONSE_SCHEMA.clone(),
            },
        );
        map.insert(
            SAVE_COMMAND_ID,
            CommandSchema {
                id: SAVE_COMMAND_ID,
                name: "save",
                schema_version: "threeterm.command.save/1",
                request_schema_version: "threeterm.command.save.request/1",
                request_schema: SAVE_REQUEST_SCHEMA.clone(),
                response_schema_version: "threeterm.command.save.response/1",
                response_schema: SNAPSHOT_RESPONSE_SCHEMA.clone(),
            },
        );
        map.insert(
            LOAD_COMMAND_ID,
            CommandSchema {
                id: LOAD_COMMAND_ID,
                name: "load",
                schema_version: "threeterm.command.load/1",
                request_schema_version: "threeterm.command.load.request/1",
                request_schema: LOAD_REQUEST_SCHEMA.clone(),
                response_schema_version: "threeterm.command.load.response/1",
                response_schema: SNAPSHOT_RESPONSE_SCHEMA.clone(),
            },
        );
        map
    });

/// Reserve a stable id for the `list` command so adapters can reference it
/// without depending on the entry's table position.
pub const LIST_COMMAND_ID: CommandId = CommandId("list");

/// Stable id for the `save` command.
pub const SAVE_COMMAND_ID: CommandId = CommandId("save");

/// Stable id for the `load` command.
pub const LOAD_COMMAND_ID: CommandId = CommandId("load");

/// Stable lookup against the registry. Returns `Some` when the id is
/// registered, `None` otherwise. Adapters use this to resolve a parsed
/// command id into the canonical schema row.
pub fn find(command: CommandId) -> Option<&'static CommandSchema> {
    COMMAND_REGISTRY.get(&command)
}

/// Iterate the registered commands in stable insertion order.
///
/// The slice (#233) seeds a single entry; later slices extend the table.
/// The dispatcher's `--machine list` output collects this iterator so the
/// call sites stay agnostic of the underlying table type.
pub fn iter() -> impl Iterator<Item = &'static CommandSchema> {
    COMMAND_REGISTRY.values()
}

/// SHA-256 hex digest of the canonical JSON encoding of `COMMAND_REGISTRY`.
///
/// The canonical encoding serializes each entry in declaration order via
/// `serde_json::to_vec` (which sorts `Value::Object` keys through the
/// default `BTreeMap` backing) and emits a trailing newline between
/// entries. The returned string is 64 lowercase hex characters.
pub fn registry_hash() -> String {
    let mut hasher = Sha256::new();

    for entry in iter() {
        let serialized = serde_json::to_vec(entry).expect("entry serializes");
        hasher.update(serialized);
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

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize)]
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

fn transform_schema() -> Value {
    json!({
        "type": "object",
        "required": ["translation_micrometers", "rotation_degrees"],
        "properties": {
            "translation_micrometers": {
                "type": "array",
                "items": { "type": "integer" }
            },
            "rotation_degrees": {
                "type": "array",
                "items": { "type": "integer" }
            }
        },
        "additionalProperties": false
    })
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

/// Canonical request schema document for the `new-project` command.
pub static NEW_PROJECT_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["destination"],
        "properties": { "destination": { "type": "string", "minLength": 1 } },
        "additionalProperties": false
    })
});

/// Canonical response schema document for the `new-project` command.
pub static NEW_PROJECT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["generation_id", "manifest"],
        "properties": {
            "generation_id": { "type": "string" },
            "manifest": { "type": "object" }
        },
        "additionalProperties": false
    })
});

pub static DEFINE_COMPONENT_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["definition_id", "features"],
        "properties": {
            "definition_id": { "type": "string" },
            "features": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "kind", "parameters", "references"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": { "type": "string" },
                        "parameters": { "type": "object" },
                        "references": { "type": "array" }
                    },
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
});

pub static PLACE_INSTANCE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["instance_id", "definition_id", "transform"],
        "properties": {
            "instance_id": { "type": "string" },
            "definition_id": { "type": "string" },
            "transform": transform_schema()
        },
        "additionalProperties": false
    })
});

pub static TRANSFORM_INSTANCE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["instance_id", "transform"],
        "properties": {
            "instance_id": { "type": "string" },
            "transform": transform_schema()
        },
        "additionalProperties": false
    })
});

pub static INDEPENDENT_COPY_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["source_instance_id", "copy_suffix"],
        "properties": {
            "source_instance_id": { "type": "string" },
            "copy_suffix": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static EDIT_PARAMETER_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "definition_id",
            "feature_id",
            "parameter_name",
            "parameter_value"
        ],
        "properties": {
            "definition_id": { "type": "string" },
            "feature_id": { "type": "string" },
            "parameter_name": { "type": "string" },
            "parameter_value": {}
        },
        "additionalProperties": false
    })
});

pub static COMPONENT_COMMAND_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["generation_id", "revision_id", "reattachment", "affected_ids"],
        "properties": {
            "generation_id": { "type": "string" },
            "revision_id": { "type": "string" },
            "reattachment": { "type": "string" },
            "affected_ids": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "additionalProperties": false
    })
});

/// The static command registry, keyed by `CommandId`.
pub static COMMAND_REGISTRY: LazyLock<BTreeMap<CommandId, CommandSchema>> = LazyLock::new(|| {
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
        NEW_PROJECT_COMMAND_ID,
        CommandSchema {
            id: NEW_PROJECT_COMMAND_ID,
            name: "new-project",
            schema_version: "threeterm.command.new-project/1",
            request_schema_version: "threeterm.command.new-project.request/1",
            request_schema: NEW_PROJECT_REQUEST_SCHEMA.clone(),
            response_schema_version: "threeterm.command.new-project.response/1",
            response_schema: NEW_PROJECT_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        DEFINE_COMPONENT_COMMAND_ID,
        CommandSchema {
            id: DEFINE_COMPONENT_COMMAND_ID,
            name: "define-component",
            schema_version: "threeterm.command.define-component/1",
            request_schema_version: "threeterm.command.define-component.request/1",
            request_schema: DEFINE_COMPONENT_REQUEST_SCHEMA.clone(),
            response_schema_version: "threeterm.command.define-component.response/1",
            response_schema: COMPONENT_COMMAND_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        PLACE_INSTANCE_COMMAND_ID,
        CommandSchema {
            id: PLACE_INSTANCE_COMMAND_ID,
            name: "place-instance",
            schema_version: "threeterm.command.place-instance/1",
            request_schema_version: "threeterm.command.place-instance.request/1",
            request_schema: PLACE_INSTANCE_REQUEST_SCHEMA.clone(),
            response_schema_version: "threeterm.command.place-instance.response/1",
            response_schema: COMPONENT_COMMAND_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        TRANSFORM_INSTANCE_COMMAND_ID,
        CommandSchema {
            id: TRANSFORM_INSTANCE_COMMAND_ID,
            name: "transform-instance",
            schema_version: "threeterm.command.transform-instance/1",
            request_schema_version: "threeterm.command.transform-instance.request/1",
            request_schema: TRANSFORM_INSTANCE_REQUEST_SCHEMA.clone(),
            response_schema_version: "threeterm.command.transform-instance.response/1",
            response_schema: COMPONENT_COMMAND_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        INDEPENDENT_COPY_COMMAND_ID,
        CommandSchema {
            id: INDEPENDENT_COPY_COMMAND_ID,
            name: "independent-copy",
            schema_version: "threeterm.command.independent-copy/1",
            request_schema_version: "threeterm.command.independent-copy.request/1",
            request_schema: INDEPENDENT_COPY_REQUEST_SCHEMA.clone(),
            response_schema_version: "threeterm.command.independent-copy.response/1",
            response_schema: COMPONENT_COMMAND_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        EDIT_PARAMETER_COMMAND_ID,
        CommandSchema {
            id: EDIT_PARAMETER_COMMAND_ID,
            name: "edit-parameter",
            schema_version: "threeterm.command.edit-parameter/1",
            request_schema_version: "threeterm.command.edit-parameter.request/1",
            request_schema: EDIT_PARAMETER_REQUEST_SCHEMA.clone(),
            response_schema_version: "threeterm.command.edit-parameter.response/1",
            response_schema: COMPONENT_COMMAND_RESPONSE_SCHEMA.clone(),
        },
    );
    map
});

pub const LIST_COMMAND_ID: CommandId = CommandId("list");
pub const NEW_PROJECT_COMMAND_ID: CommandId = CommandId("new-project");
pub const DEFINE_COMPONENT_COMMAND_ID: CommandId = CommandId("define-component");
pub const PLACE_INSTANCE_COMMAND_ID: CommandId = CommandId("place-instance");
pub const TRANSFORM_INSTANCE_COMMAND_ID: CommandId = CommandId("transform-instance");
pub const INDEPENDENT_COPY_COMMAND_ID: CommandId = CommandId("independent-copy");
pub const EDIT_PARAMETER_COMMAND_ID: CommandId = CommandId("edit-parameter");

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

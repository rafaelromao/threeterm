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

pub static SAVE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "kind"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "kind": { "type": "string", "minLength": 1 }
        },
        "additionalProperties": false
    })
});

pub static LOAD_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 }
        },
        "additionalProperties": false
    })
});

pub static EXTRUDE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "profile", "height"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "profile": {
                "type": "array",
                "minItems": 3,
                "items": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 2,
                    "items": { "type": "number" }
                }
            },
            "height": { "type": "number", "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

pub static BOOLEAN_FUSE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "base_feature_id", "tool_feature_id"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "tool_feature_id": { "type": "string", "minLength": 1 }
        },
        "additionalProperties": false
    })
});

pub static FILLET_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "base_feature_id", "radius"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "radius": { "type": "number", "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

pub static CHAMFER_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "base_feature_id", "distance"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "distance": { "type": "number", "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

pub static HOLE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "position",
            "direction",
            "diameter"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "position": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "direction": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "diameter": { "type": "number", "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

pub static REVOLVE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "profile",
            "axis_point",
            "axis_direction",
            "angle"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "profile": {
                "type": "array",
                "minItems": 3,
                "items": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 2,
                    "items": { "type": "number" }
                }
            },
            "axis_point": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "axis_direction": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "angle": { "type": "number", "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

pub static EXTRUDE_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static BOOLEAN_FUSE_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static FILLET_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static CHAMFER_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static HOLE_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static REVOLVE_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static MIRROR_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "plane_point",
            "plane_normal"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "plane_point": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "plane_normal": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            }
        },
        "additionalProperties": false
    })
});

pub static LINEAR_PATTERN_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "direction",
            "count",
            "spacing"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "direction": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "count": { "type": "integer", "minimum": 1 },
            "spacing": { "type": "number", "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

// `maximum: 6.283185307179586` is exactly 2π; the float literal
// trips `clippy::approx_constant` so the constant is hoisted behind
// an explicit allow.
#[allow(clippy::approx_constant)]
const CIRCULAR_PATTERN_ANGLE_STEP_MAX: f64 = 6.283185307179586;

pub static CIRCULAR_PATTERN_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "axis_point",
            "axis_normal",
            "angle_step",
            "count"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "axis_point": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "axis_normal": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            },
            "angle_step": { "type": "number", "exclusiveMinimum": 0, "maximum": CIRCULAR_PATTERN_ANGLE_STEP_MAX },
            "count": { "type": "integer", "minimum": 1 }
        },
        "additionalProperties": false
    })
});

pub static MIRROR_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static LINEAR_PATTERN_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static CIRCULAR_PATTERN_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static SHELL_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "thickness"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "thickness": { "type": "number", "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

pub static SHELL_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static DRAFT_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "base_feature_id",
            "angle",
            "pull_direction"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "base_feature_id": { "type": "string", "minLength": 1 },
            "angle": { "type": "number", "exclusiveMinimum": 0 },
            "pull_direction": {
                "type": "array",
                "minItems": 3,
                "maxItems": 3,
                "items": { "type": "number" }
            }
        },
        "additionalProperties": false
    })
});

pub static DRAFT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static LOFT_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "bundle_path",
            "feature_id",
            "profiles"
        ],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "profiles": {
                "type": "array",
                "minItems": 2,
                "items": {
                    "type": "array",
                    "minItems": 3,
                    "items": {
                        "type": "array",
                        "minItems": 3,
                        "maxItems": 3,
                        "items": { "type": "number" }
                    }
                }
            },
            "is_solid": { "type": "boolean" },
            "ruled": { "type": "boolean" }
        },
        "additionalProperties": false
    })
});

pub static LOFT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status",
            "operation",
            "feature_id",
            "feature_graph_hash",
            "revision_hash",
            "brep_path",
            "brep_sha256",
            "schema_version"
        ],
        "properties": {
            "status": { "type": "string", "minLength": 1 },
            "operation": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_path": { "type": "string", "minLength": 1 },
            "brep_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "brep_bytes": { "type": "integer", "minimum": 0 },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static SNAPSHOT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
            "type": "object",
            "required": ["feature_graph_hash", "revision_hash", "schema_version"],
            "properties": {
                "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
                "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
                "schema_version": { "type": "string" }
            },
            "additionalProperties": false
    })
});

/// Canonical request schema document for the `bracket` command. The numeric
/// dimensions are stored in the canonical transaction log but no OCCT
/// geometry is computed in this slice — that is the responsibility of a
/// future worker slice. The four dimensions must each be strictly positive
/// (`minimum > 0`); zero, negative, NaN, or infinite values describe a
/// degenerate solid and are rejected by the schema validator before they
/// reach the host.
pub static BRACKET_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "bracket_id", "length", "width", "height", "thickness"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "bracket_id": { "type": "string", "minLength": 1 },
            "length": { "type": "number", "minimum": 0, "exclusiveMinimum": 0 },
            "width": { "type": "number", "minimum": 0, "exclusiveMinimum": 0 },
            "height": { "type": "number", "minimum": 0, "exclusiveMinimum": 0 },
            "thickness": { "type": "number", "minimum": 0, "exclusiveMinimum": 0 }
        },
        "additionalProperties": false
    })
});

/// Canonical response schema document for the `bracket` command. The
/// `schema_version` field is pinned to the bracket response-schema version
/// constant so the dispatcher emits the same wire contract as `save` and
/// `load`.
pub static BRACKET_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["feature_graph_hash", "revision_hash", "schema_version"],
        "properties": {
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static LOAD_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["feature_graph_hash", "revision_hash", "recovered_from_previous", "schema_version"],
        "properties": {
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "recovered_from_previous": { "type": "boolean" },
            "schema_version": { "type": "string" }
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
        SAVE_COMMAND_ID,
        CommandSchema {
            id: SAVE_COMMAND_ID,
            name: "save",
            schema_version: "threeterm.command.save/1",
            request_schema_version: "threeterm.command.save.request/1",
            request_schema: SAVE_REQUEST_SCHEMA.clone(),
            response_schema_version: SAVE_RESPONSE_SCHEMA_VERSION,
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
            response_schema_version: LOAD_RESPONSE_SCHEMA_VERSION,
            response_schema: LOAD_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        BRACKET_COMMAND_ID,
        CommandSchema {
            id: BRACKET_COMMAND_ID,
            name: "bracket",
            schema_version: "threeterm.command.bracket/1",
            request_schema_version: "threeterm.command.bracket.request/1",
            request_schema: BRACKET_REQUEST_SCHEMA.clone(),
            response_schema_version: BRACKET_RESPONSE_SCHEMA_VERSION,
            response_schema: BRACKET_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        EXTRUDE_COMMAND_ID,
        CommandSchema {
            id: EXTRUDE_COMMAND_ID,
            name: "extrude",
            schema_version: "threeterm.command.extrude/1",
            request_schema_version: "threeterm.command.extrude.request/1",
            request_schema: EXTRUDE_REQUEST_SCHEMA.clone(),
            response_schema_version: EXTRUDE_RESPONSE_SCHEMA_VERSION,
            response_schema: EXTRUDE_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        BOOLEAN_FUSE_COMMAND_ID,
        CommandSchema {
            id: BOOLEAN_FUSE_COMMAND_ID,
            name: "boolean-fuse",
            schema_version: "threeterm.command.boolean-fuse/1",
            request_schema_version: "threeterm.command.boolean-fuse.request/1",
            request_schema: BOOLEAN_FUSE_REQUEST_SCHEMA.clone(),
            response_schema_version: BOOLEAN_FUSE_RESPONSE_SCHEMA_VERSION,
            response_schema: BOOLEAN_FUSE_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        FILLET_COMMAND_ID,
        CommandSchema {
            id: FILLET_COMMAND_ID,
            name: "fillet",
            schema_version: "threeterm.command.fillet/1",
            request_schema_version: "threeterm.command.fillet.request/1",
            request_schema: FILLET_REQUEST_SCHEMA.clone(),
            response_schema_version: FILLET_RESPONSE_SCHEMA_VERSION,
            response_schema: FILLET_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        CHAMFER_COMMAND_ID,
        CommandSchema {
            id: CHAMFER_COMMAND_ID,
            name: "chamfer",
            schema_version: "threeterm.command.chamfer/1",
            request_schema_version: "threeterm.command.chamfer.request/1",
            request_schema: CHAMFER_REQUEST_SCHEMA.clone(),
            response_schema_version: CHAMFER_RESPONSE_SCHEMA_VERSION,
            response_schema: CHAMFER_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        HOLE_COMMAND_ID,
        CommandSchema {
            id: HOLE_COMMAND_ID,
            name: "hole",
            schema_version: "threeterm.command.hole/1",
            request_schema_version: "threeterm.command.hole.request/1",
            request_schema: HOLE_REQUEST_SCHEMA.clone(),
            response_schema_version: HOLE_RESPONSE_SCHEMA_VERSION,
            response_schema: HOLE_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        REVOLVE_COMMAND_ID,
        CommandSchema {
            id: REVOLVE_COMMAND_ID,
            name: "revolve",
            schema_version: "threeterm.command.revolve/1",
            request_schema_version: "threeterm.command.revolve.request/1",
            request_schema: REVOLVE_REQUEST_SCHEMA.clone(),
            response_schema_version: REVOLVE_RESPONSE_SCHEMA_VERSION,
            response_schema: REVOLVE_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        MIRROR_COMMAND_ID,
        CommandSchema {
            id: MIRROR_COMMAND_ID,
            name: "mirror",
            schema_version: "threeterm.command.mirror/1",
            request_schema_version: "threeterm.command.mirror.request/1",
            request_schema: MIRROR_REQUEST_SCHEMA.clone(),
            response_schema_version: MIRROR_RESPONSE_SCHEMA_VERSION,
            response_schema: MIRROR_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        LINEAR_PATTERN_COMMAND_ID,
        CommandSchema {
            id: LINEAR_PATTERN_COMMAND_ID,
            name: "linear-pattern",
            schema_version: "threeterm.command.linear-pattern/1",
            request_schema_version: "threeterm.command.linear-pattern.request/1",
            request_schema: LINEAR_PATTERN_REQUEST_SCHEMA.clone(),
            response_schema_version: LINEAR_PATTERN_RESPONSE_SCHEMA_VERSION,
            response_schema: LINEAR_PATTERN_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        CIRCULAR_PATTERN_COMMAND_ID,
        CommandSchema {
            id: CIRCULAR_PATTERN_COMMAND_ID,
            name: "circular-pattern",
            schema_version: "threeterm.command.circular-pattern/1",
            request_schema_version: "threeterm.command.circular-pattern.request/1",
            request_schema: CIRCULAR_PATTERN_REQUEST_SCHEMA.clone(),
            response_schema_version: CIRCULAR_PATTERN_RESPONSE_SCHEMA_VERSION,
            response_schema: CIRCULAR_PATTERN_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        SHELL_COMMAND_ID,
        CommandSchema {
            id: SHELL_COMMAND_ID,
            name: "shell",
            schema_version: "threeterm.command.shell/1",
            request_schema_version: "threeterm.command.shell.request/1",
            request_schema: SHELL_REQUEST_SCHEMA.clone(),
            response_schema_version: SHELL_RESPONSE_SCHEMA_VERSION,
            response_schema: SHELL_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        DRAFT_COMMAND_ID,
        CommandSchema {
            id: DRAFT_COMMAND_ID,
            name: "draft",
            schema_version: "threeterm.command.draft/1",
            request_schema_version: "threeterm.command.draft.request/1",
            request_schema: DRAFT_REQUEST_SCHEMA.clone(),
            response_schema_version: DRAFT_RESPONSE_SCHEMA_VERSION,
            response_schema: DRAFT_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        LOFT_COMMAND_ID,
        CommandSchema {
            id: LOFT_COMMAND_ID,
            name: "loft",
            schema_version: "threeterm.command.loft/1",
            request_schema_version: "threeterm.command.loft.request/1",
            request_schema: LOFT_REQUEST_SCHEMA.clone(),
            response_schema_version: LOFT_RESPONSE_SCHEMA_VERSION,
            response_schema: LOFT_RESPONSE_SCHEMA.clone(),
        },
    );
    map
});

pub const LIST_COMMAND_ID: CommandId = CommandId("list");
pub const NEW_PROJECT_COMMAND_ID: CommandId = CommandId("new-project");
pub const SAVE_COMMAND_ID: CommandId = CommandId("save");
pub const LOAD_COMMAND_ID: CommandId = CommandId("load");
pub const BRACKET_COMMAND_ID: CommandId = CommandId("bracket");
pub const EXTRUDE_COMMAND_ID: CommandId = CommandId("extrude");
pub const BOOLEAN_FUSE_COMMAND_ID: CommandId = CommandId("boolean-fuse");
pub const FILLET_COMMAND_ID: CommandId = CommandId("fillet");
pub const CHAMFER_COMMAND_ID: CommandId = CommandId("chamfer");
pub const HOLE_COMMAND_ID: CommandId = CommandId("hole");
pub const REVOLVE_COMMAND_ID: CommandId = CommandId("revolve");
pub const MIRROR_COMMAND_ID: CommandId = CommandId("mirror");
pub const LINEAR_PATTERN_COMMAND_ID: CommandId = CommandId("linear-pattern");
pub const CIRCULAR_PATTERN_COMMAND_ID: CommandId = CommandId("circular-pattern");
pub const SHELL_COMMAND_ID: CommandId = CommandId("shell");
pub const DRAFT_COMMAND_ID: CommandId = CommandId("draft");
pub const LOFT_COMMAND_ID: CommandId = CommandId("loft");
pub const SAVE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.save.response/1";
pub const LOAD_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.load.response/2";
pub const BRACKET_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.bracket.response/1";
pub const EXTRUDE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.extrude.response/1";
pub const BOOLEAN_FUSE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.boolean-fuse.response/1";
pub const FILLET_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.fillet.response/1";
pub const CHAMFER_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.chamfer.response/1";
pub const HOLE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.hole.response/1";
pub const REVOLVE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.revolve.response/1";
pub const MIRROR_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.mirror.response/1";
pub const LINEAR_PATTERN_RESPONSE_SCHEMA_VERSION: &str =
    "threeterm.command.linear-pattern.response/1";
pub const CIRCULAR_PATTERN_RESPONSE_SCHEMA_VERSION: &str =
    "threeterm.command.circular-pattern.response/1";
pub const SHELL_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.shell.response/1";
pub const DRAFT_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.draft.response/1";
pub const LOFT_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.loft.response/1";

/// registered, `None` otherwise. Adapters use this to resolve a parsed
/// command id into the canonical schema row.
pub fn find(command: CommandId) -> Option<&'static CommandSchema> {
    COMMAND_REGISTRY.get(&command)
}

/// Resolve a registered command from its presentation-neutral name.
pub fn find_by_name(name: &str) -> Option<&'static CommandSchema> {
    iter().find(|entry| entry.name == name)
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

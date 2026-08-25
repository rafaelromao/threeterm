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

pub static EXPORT_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "formats", "output_dir", "tessellation_deflection", "override_warnings", "accept_stale_geometry"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "body_ids": { "type": "array", "items": { "type": "string", "minLength": 1 } },
            "formats": { "type": "array", "minItems": 1, "items": { "type": "string", "enum": ["stl", "3mf", "step"] } },
            "output_dir": { "type": "string", "minLength": 1 },
            "tessellation_deflection": { "type": "number", "exclusiveMinimum": 0 },
            "override_warnings": { "type": "boolean" },
            "accept_stale_geometry": { "type": "boolean" }
        }, "additionalProperties": false
    })
});
pub static EXPORT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object", "required": ["status", "feature_id", "artifacts", "accepted_stale_last_valid_geometry", "stale_last_valid_geometry", "schema_version"],
        "properties": {
            "status": { "type": "string" },
            "feature_id": { "type": "string" },
            "artifacts": { "type": "array" },
            "accepted_stale_last_valid_geometry": { "type": "boolean" },
            "stale_last_valid_geometry": {
                "type": "object",
                "required": ["feature_id", "active_revision", "stale_features"],
                "properties": {
                    "feature_id": { "type": "string", "minLength": 1 },
                    "active_revision": { "type": "string", "minLength": 1 },
                    "stale_features": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["feature_id", "status", "last_valid_geometry_fingerprint"],
                            "properties": {
                                "feature_id": { "type": "string", "minLength": 1 },
                                "status": { "type": "string", "enum": ["broken", "blocked-by-failure"] },
                                "last_valid_geometry_fingerprint": { "type": "string", "minLength": 1 }
                            },
                            "additionalProperties": false
                        }
                    }
                },
                "additionalProperties": false
            },
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

pub static HISTORICAL_EDIT_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "parameter", "value"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "parameter": { "type": "string", "minLength": 1 },
            "value": { "type": "number" }
        },
        "additionalProperties": false
    })
});

pub static HISTORY_COMMIT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "status", "operation", "active_revision", "dirty_features",
            "evaluated_features", "blocked_features", "diagnostics",
            "named_revisions", "features", "feature_graph_hash", "revision_hash", "schema_version"
        ],
        "properties": {
            "status": { "type": "string" },
            "operation": { "type": "string" },
            "active_revision": { "type": "string", "minLength": 1 },
            "dirty_features": { "type": "array", "items": { "type": "string" } },
            "evaluated_features": { "type": "array", "items": { "type": "string" } },
            "blocked_features": { "type": "array", "items": { "type": "string" } },
            "diagnostics": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["code", "feature_id", "detail"],
                    "properties": {
                        "code": { "type": "string", "minLength": 1 },
                        "feature_id": { "type": "string", "minLength": 1 },
                        "detail": { "type": "string", "minLength": 1 }
                    },
                    "additionalProperties": false
                }
            },
            "named_revisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["name", "revision_id", "provenance"],
                    "properties": {
                        "name": { "type": "string", "minLength": 1 },
                        "revision_id": { "type": "string", "minLength": 1 },
                        "provenance": { "type": "string", "minLength": 1 }
                    },
                    "additionalProperties": false
                }
            },
            "features": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "status", "geometry_fingerprint", "last_valid_geometry_fingerprint", "stale_last_valid_geometry"],
                    "properties": {
                        "id": { "type": "string", "minLength": 1 },
                        "status": { "type": "string", "minLength": 1 },
                        "geometry_fingerprint": { "type": "string" },
                        "last_valid_geometry_fingerprint": { "type": "string" },
                        "stale_last_valid_geometry": { "type": "boolean" },
                        "diagnostic": { "type": "object" }
                    },
                    "additionalProperties": false
                }
            },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static NAMED_REVISION_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "name"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "name": { "type": "string", "minLength": 1 }
        },
        "additionalProperties": false
    })
});

pub static RESTORE_REVISION_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id", "name"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 },
            "name": { "type": "string", "minLength": 1 }
        },
        "additionalProperties": false
    })
});

pub static TIMELINE_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path", "feature_id"],
        "properties": {
            "bundle_path": { "type": "string", "minLength": 1 },
            "feature_id": { "type": "string", "minLength": 1 }
        },
        "additionalProperties": false
    })
});

pub static TIMELINE_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "feature_id", "active_revision", "revisions", "named_revisions",
            "feature_graph_hash", "revision_hash", "schema_version"
        ],
        "properties": {
            "feature_id": { "type": "string", "minLength": 1 },
            "active_revision": { "type": "string", "minLength": 1 },
            "revisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["ordinal", "revision_id", "operation", "status", "stale_last_valid_geometry_fingerprint", "named_revision_names"],
                    "properties": {
                        "ordinal": { "type": "integer", "minimum": 1 },
                        "revision_id": { "type": "string", "minLength": 1 },
                        "operation": { "type": "string", "minLength": 1 },
                        "status": { "type": "string", "minLength": 1 },
                        "stale_last_valid_geometry_fingerprint": { "type": "string" },
                        "named_revision_names": { "type": "array", "items": { "type": "string", "minLength": 1 } }
                    },
                    "additionalProperties": false
                }
            },
            "named_revisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["name", "revision_id", "provenance"],
                    "properties": {
                        "name": { "type": "string", "minLength": 1 },
                        "revision_id": { "type": "string", "minLength": 1 },
                        "provenance": { "type": "string", "minLength": 1 }
                    },
                    "additionalProperties": false
                }
            },
            "feature_graph_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "revision_hash": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "schema_version": { "type": "string" }
        },
        "additionalProperties": false
    })
});

pub static REPLAY_VERIFY_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["bundle_path"],
        "properties": { "bundle_path": { "type": "string", "minLength": 1 } },
        "additionalProperties": false
    })
});

pub static REPLAY_VERIFY_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["deterministic", "fingerprint", "mismatch", "schema_version"],
        "properties": {
            "deterministic": { "type": "boolean" },
            "fingerprint": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "mismatch": { "type": "string" },
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

/// Versioned semantic lifecycle contract for editing an L-bracket parameter.
/// The phase is explicit so session adapters cannot confuse a transient
/// preview with a canonical commit.
pub static BRACKET_EDIT_REQUEST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": [
            "phase", "bundle_path", "draft_id", "bracket_id",
            "length", "width", "height", "thickness"
        ],
        "properties": {
            "phase": { "type": "string", "enum": ["open", "update", "preview", "commit", "discard"] },
            "bundle_path": { "type": "string", "minLength": 1 },
            "draft_id": { "type": "string", "minLength": 1 },
            "bracket_id": { "type": "string", "minLength": 1 },
            "length": { "type": "number", "exclusiveMinimum": 0 },
            "width": { "type": "number", "exclusiveMinimum": 0 },
            "height": { "type": "number", "exclusiveMinimum": 0 },
            "thickness": { "type": "number", "exclusiveMinimum": 0 },
            "source_revision": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "draft_sequence": { "type": "integer", "minimum": 0 },
            "input_fingerprint": { "type": "string", "pattern": "^[0-9a-f]{64}$" }
        },
        "additionalProperties": false
    })
});

pub static BRACKET_EDIT_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "required": ["status", "phase", "draft_id", "source_revision", "schema_version"],
        "properties": {
            "status": { "type": "string", "enum": ["ok", "rejected", "unknown"] },
            "phase": { "type": "string", "minLength": 1 },
            "draft_id": { "type": "string", "minLength": 1 },
            "source_revision": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "current_revision": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "preview_revision": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "input_fingerprint": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "draft_sequence": { "type": "integer", "minimum": 0 },
            "diagnostic": { "type": "object" },
            "schema_version": { "type": "string", "minLength": 1 }
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

fn component_request_schema(fields: &[&str]) -> Value {
    let mut properties = serde_json::Map::new();
    for field in fields {
        let schema = match *field {
            "transform" => {
                json!({"type":"array", "minItems":3, "maxItems":3, "items":{"type":"number"}})
            }
            "selected_feature_ids" => {
                json!({"type":"array", "minItems":1, "uniqueItems":true, "items":{"type":"string", "minLength":1}})
            }
            "length" | "width" | "height" | "thickness" | "value" => {
                json!({"type":"number", "exclusiveMinimum":0})
            }
            _ => json!({"type":"string", "minLength":1}),
        };
        properties.insert((*field).to_string(), schema);
    }
    json!({"type":"object", "required":fields, "properties":properties, "additionalProperties":false})
}

pub static COMPONENT_STATE_RESPONSE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type":"object", "required":["definitions","instances","schema_version"],
        "properties":{"definitions":{"type":"object"},"instances":{"type":"object"},"schema_version":{"type":"string"}},
        "additionalProperties":false
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
    for (id, name, fields) in [
        (
            DEFINE_COMPONENT_COMMAND_ID,
            "define-component",
            &[
                "bundle_path",
                "definition_id",
                "feature_id",
                "length",
                "width",
                "height",
                "thickness",
            ][..],
        ),
        (
            CREATE_COMPONENT_INSTANCE_COMMAND_ID,
            "create-component-instance",
            &["bundle_path", "instance_id", "definition_id", "transform"][..],
        ),
        (
            TRANSFORM_COMPONENT_INSTANCE_COMMAND_ID,
            "transform-component-instance",
            &["bundle_path", "instance_id", "transform"][..],
        ),
        (
            MAKE_COMPONENT_INDEPENDENT_COMMAND_ID,
            "make-component-independent",
            &[
                "bundle_path",
                "source_instance_id",
                "definition_id",
                "instance_id",
                "feature_id",
            ][..],
        ),
        (
            EDIT_COMPONENT_PARAMETER_COMMAND_ID,
            "edit-component-parameter",
            &["bundle_path", "definition_id", "parameter", "value"][..],
        ),
    ] {
        let command = format!("threeterm.command.{name}/1");
        let request = format!("threeterm.command.{name}.request/1");
        let response = format!("threeterm.command.{name}.response/1");
        map.insert(
            id,
            CommandSchema {
                id,
                name,
                schema_version: Box::leak(command.into_boxed_str()),
                request_schema_version: Box::leak(request.into_boxed_str()),
                request_schema: component_request_schema(fields),
                response_schema_version: Box::leak(response.into_boxed_str()),
                response_schema: SNAPSHOT_RESPONSE_SCHEMA.clone(),
            },
        );
    }
    map.insert(
        COMPONENT_STATE_COMMAND_ID,
        CommandSchema {
            id: COMPONENT_STATE_COMMAND_ID,
            name: "component-state",
            schema_version: "threeterm.command.component-state/1",
            request_schema_version: "threeterm.command.component-state.request/1",
            request_schema: component_request_schema(&["bundle_path"]),
            response_schema_version: "threeterm.command.component-state.response/1",
            response_schema: COMPONENT_STATE_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        BRACKET_EDIT_COMMAND_ID,
        CommandSchema {
            id: BRACKET_EDIT_COMMAND_ID,
            name: "bracket-edit",
            schema_version: "threeterm.command.bracket-edit/1",
            request_schema_version: "threeterm.command.bracket-edit.request/1",
            request_schema: BRACKET_EDIT_REQUEST_SCHEMA.clone(),
            response_schema_version: BRACKET_EDIT_RESPONSE_SCHEMA_VERSION,
            response_schema: BRACKET_EDIT_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        CAPTURE_COMPONENT_COMMAND_ID,
        CommandSchema {
            id: CAPTURE_COMPONENT_COMMAND_ID,
            name: "capture-component",
            schema_version: "threeterm.command.capture-component/1",
            request_schema_version: "threeterm.command.capture-component.request/1",
            request_schema: component_request_schema(&[
                "bundle_path",
                "definition_id",
                "selected_feature_ids",
            ]),
            response_schema_version: "threeterm.command.capture-component.response/1",
            response_schema: SNAPSHOT_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        HISTORICAL_EDIT_COMMAND_ID,
        CommandSchema {
            id: HISTORICAL_EDIT_COMMAND_ID,
            name: "historical-edit",
            schema_version: "threeterm.command.historical-edit/1",
            request_schema_version: "threeterm.command.historical-edit.request/1",
            request_schema: HISTORICAL_EDIT_REQUEST_SCHEMA.clone(),
            response_schema_version: HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION,
            response_schema: HISTORY_COMMIT_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        CREATE_REVISION_COMMAND_ID,
        CommandSchema {
            id: CREATE_REVISION_COMMAND_ID,
            name: "create-revision",
            schema_version: "threeterm.command.create-revision/1",
            request_schema_version: "threeterm.command.create-revision.request/1",
            request_schema: NAMED_REVISION_REQUEST_SCHEMA.clone(),
            response_schema_version: HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION,
            response_schema: HISTORY_COMMIT_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        RESTORE_REVISION_COMMAND_ID,
        CommandSchema {
            id: RESTORE_REVISION_COMMAND_ID,
            name: "restore-revision",
            schema_version: "threeterm.command.restore-revision/1",
            request_schema_version: RESTORE_REVISION_REQUEST_SCHEMA_VERSION,
            request_schema: RESTORE_REVISION_REQUEST_SCHEMA.clone(),
            response_schema_version: HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION,
            response_schema: HISTORY_COMMIT_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        TIMELINE_COMMAND_ID,
        CommandSchema {
            id: TIMELINE_COMMAND_ID,
            name: "timeline",
            schema_version: "threeterm.command.timeline/1",
            request_schema_version: TIMELINE_REQUEST_SCHEMA_VERSION,
            request_schema: TIMELINE_REQUEST_SCHEMA.clone(),
            response_schema_version: TIMELINE_RESPONSE_SCHEMA_VERSION,
            response_schema: TIMELINE_RESPONSE_SCHEMA.clone(),
        },
    );
    map.insert(
        REPLAY_VERIFY_COMMAND_ID,
        CommandSchema {
            id: REPLAY_VERIFY_COMMAND_ID,
            name: "replay-verify",
            schema_version: "threeterm.command.replay-verify/1",
            request_schema_version: "threeterm.command.replay-verify.request/1",
            request_schema: REPLAY_VERIFY_REQUEST_SCHEMA.clone(),
            response_schema_version: REPLAY_VERIFY_RESPONSE_SCHEMA_VERSION,
            response_schema: REPLAY_VERIFY_RESPONSE_SCHEMA.clone(),
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
    map.insert(
        EXPORT_COMMAND_ID,
        CommandSchema {
            id: EXPORT_COMMAND_ID,
            name: "export",
            schema_version: "threeterm.command.export/1",
            request_schema_version: "threeterm.command.export.request/2",
            request_schema: EXPORT_REQUEST_SCHEMA.clone(),
            response_schema_version: EXPORT_RESPONSE_SCHEMA_VERSION,
            response_schema: EXPORT_RESPONSE_SCHEMA.clone(),
        },
    );
    map
});

pub const LIST_COMMAND_ID: CommandId = CommandId("list");
pub const NEW_PROJECT_COMMAND_ID: CommandId = CommandId("new-project");
pub const SAVE_COMMAND_ID: CommandId = CommandId("save");
pub const LOAD_COMMAND_ID: CommandId = CommandId("load");
pub const BRACKET_COMMAND_ID: CommandId = CommandId("bracket");
pub const BRACKET_EDIT_COMMAND_ID: CommandId = CommandId("bracket-edit");
pub const DEFINE_COMPONENT_COMMAND_ID: CommandId = CommandId("define-component");
pub const CREATE_COMPONENT_INSTANCE_COMMAND_ID: CommandId = CommandId("create-component-instance");
pub const TRANSFORM_COMPONENT_INSTANCE_COMMAND_ID: CommandId =
    CommandId("transform-component-instance");
pub const MAKE_COMPONENT_INDEPENDENT_COMMAND_ID: CommandId =
    CommandId("make-component-independent");
pub const EDIT_COMPONENT_PARAMETER_COMMAND_ID: CommandId = CommandId("edit-component-parameter");
pub const COMPONENT_STATE_COMMAND_ID: CommandId = CommandId("component-state");
pub const CAPTURE_COMPONENT_COMMAND_ID: CommandId = CommandId("capture-component");
pub const HISTORICAL_EDIT_COMMAND_ID: CommandId = CommandId("historical-edit");
pub const CREATE_REVISION_COMMAND_ID: CommandId = CommandId("create-revision");
pub const RESTORE_REVISION_COMMAND_ID: CommandId = CommandId("restore-revision");
pub const REPLAY_VERIFY_COMMAND_ID: CommandId = CommandId("replay-verify");
pub const TIMELINE_COMMAND_ID: CommandId = CommandId("timeline");
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
pub const EXPORT_COMMAND_ID: CommandId = CommandId("export");
pub const SAVE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.save.response/1";
pub const LOAD_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.load.response/2";
pub const BRACKET_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.bracket.response/1";
pub const BRACKET_EDIT_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.bracket-edit.response/1";
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
pub const EXPORT_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.export.response/2";
pub const HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.history.response/2";
pub const REPLAY_VERIFY_RESPONSE_SCHEMA_VERSION: &str =
    "threeterm.command.replay-verify.response/1";
pub const TIMELINE_REQUEST_SCHEMA_VERSION: &str = "threeterm.command.timeline.request/1";
pub const TIMELINE_RESPONSE_SCHEMA_VERSION: &str = "threeterm.command.timeline.response/2";
pub const RESTORE_REVISION_REQUEST_SCHEMA_VERSION: &str =
    "threeterm.command.restore-revision.request/2";

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

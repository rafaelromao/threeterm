use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "threeterm.workers.slvs/1";
pub const OPERATION: &str = "sketch_solve";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SketchEntity {
    Point {
        id: String,
        x: f64,
        y: f64,
    },
    LineSegment {
        id: String,
        start: String,
        end: String,
    },
    Circle {
        id: String,
        center: String,
        radius: f64,
    },
    Arc {
        id: String,
        center: String,
        start: String,
        end: String,
    },
}

impl SketchEntity {
    pub fn id(&self) -> &str {
        match self {
            Self::Point { id, .. }
            | Self::LineSegment { id, .. }
            | Self::Circle { id, .. }
            | Self::Arc { id, .. } => id,
        }
    }

    fn references(&self) -> impl Iterator<Item = &str> {
        let refs: Vec<&str> = match self {
            Self::Point { .. } => Vec::new(),
            Self::LineSegment { start, end, .. } => vec![start, end],
            Self::Circle { center, .. } => vec![center],
            Self::Arc {
                center, start, end, ..
            } => vec![center, start, end],
        };
        refs.into_iter()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchConstraint {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchSolveRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: String,
    pub feature_id: String,
    #[serde(default)]
    pub source_revision: String,
    pub entities: Vec<SketchEntity>,
    pub constraints: Vec<SketchConstraint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolvedCoordinate {
    pub entity_id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchDiagnostic {
    pub code: String,
    pub detail: String,
    #[serde(default)]
    pub constraint_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchSolveResponse {
    pub schema_version: String,
    pub request_id: String,
    pub operation: String,
    pub feature_id: String,
    pub status: String,
    pub dof: i32,
    pub entity_ids: Vec<String>,
    pub related_constraint_ids: Vec<String>,
    pub diagnostics: Vec<SketchDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solved_coordinates: Option<Vec<SolvedCoordinate>>,
}

impl SketchSolveRequest {
    pub fn new(
        request_id: impl Into<String>,
        feature_id: impl Into<String>,
        entities: Vec<SketchEntity>,
        constraints: Vec<SketchConstraint>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: OPERATION.to_string(),
            feature_id: feature_id.into(),
            source_revision: String::new(),
            entities,
            constraints,
        }
    }

    pub fn with_source_revision(mut self, revision: impl Into<String>) -> Self {
        self.source_revision = revision.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!("schema_version must be {SCHEMA_VERSION}"));
        }
        if self.operation != OPERATION {
            return Err(format!("operation must be {OPERATION}"));
        }
        for (label, value) in [
            ("request_id", &self.request_id),
            ("feature_id", &self.feature_id),
        ] {
            if value.is_empty() || value.len() > 128 || !value.chars().all(valid_id_char) {
                return Err(format!("{label} must be a bounded stable identifier"));
            }
        }
        if self.entities.is_empty() {
            return Err("sketch must contain at least one entity".to_string());
        }
        let mut ids = BTreeSet::new();
        for entity in &self.entities {
            if entity.id().is_empty() || !ids.insert(entity.id().to_string()) {
                return Err("sketch entity IDs must be non-empty and unique".to_string());
            }
            match entity {
                SketchEntity::Point { x, y, .. } if x.is_finite() && y.is_finite() => {}
                SketchEntity::Point { .. } => {
                    return Err("point coordinates must be finite".to_string());
                }
                SketchEntity::Circle { radius, .. } if radius.is_finite() && *radius > 0.0 => {}
                SketchEntity::Circle { .. } => {
                    return Err("circle radius must be positive and finite".to_string());
                }
                SketchEntity::LineSegment { .. } | SketchEntity::Arc { .. } => {}
            }
        }
        for entity in &self.entities {
            if entity
                .references()
                .any(|reference| !ids.contains(reference))
            {
                return Err(format!(
                    "entity {} references an unknown point",
                    entity.id()
                ));
            }
        }
        for constraint in &self.constraints {
            if constraint.id.is_empty() || !ids.insert(constraint.id.clone()) {
                return Err("entity and constraint IDs must be globally unique".to_string());
            }
            if constraint
                .entities
                .iter()
                .any(|id| !self.entities.iter().any(|entity| entity.id() == id))
            {
                return Err(format!(
                    "constraint {} references an unknown entity",
                    constraint.id
                ));
            }
            if constraint.value.is_some_and(|value| !value.is_finite()) {
                return Err(format!("constraint {} value must be finite", constraint.id));
            }
        }
        Ok(())
    }
}

impl SketchSolveResponse {
    pub fn is_success(&self) -> bool {
        self.status == "solved"
    }

    pub fn validate_for(&self, request: &SketchSolveRequest) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION
            || self.request_id != request.request_id
            || self.operation != OPERATION
            || self.feature_id != request.feature_id
        {
            return Err("worker response identity does not match the request".to_string());
        }
        if self.dof < 0
            || !matches!(
                self.status.as_str(),
                "solved"
                    | "underconstrained"
                    | "redundant"
                    | "inconsistent"
                    | "nonconvergent"
                    | "invalid_request"
            )
        {
            return Err("worker response has an unknown status or negative dof".to_string());
        }
        if self.status == "solved" && self.solved_coordinates.is_none() {
            return Err("solved worker response is missing coordinates".to_string());
        }
        if self.status != "solved" && self.solved_coordinates.is_some() {
            return Err("failed worker response must not include coordinates".to_string());
        }
        if self.entity_ids
            != request
                .entities
                .iter()
                .map(SketchEntity::id)
                .collect::<Vec<_>>()
        {
            return Err("worker response entity_ids are not in request order".to_string());
        }
        Ok(())
    }
}

fn valid_id_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.')
}

//! Typed envelope exchanged with the `threeterm-slvs-worker` binary.
//!
//! The worker protocol is `threeterm.workers.slvs/1`. The host is the sole
//! owner of stable caller IDs; libslvs numeric handles are private to the
//! worker.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::SCHEMA_VERSION;

fn is_schema_version(value: &str) -> bool {
    value == SCHEMA_VERSION
}

fn is_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum SketchParamValue {
    Bool(bool),
    Number(f64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchParam {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
}

impl SketchParam {
    pub fn point_2d(x: f64, y: f64) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            ..Self::empty()
        }
    }

    pub fn point_2d_fixed(x: f64, y: f64, fixed: bool) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            fixed: Some(fixed),
            ..Self::empty()
        }
    }

    pub fn line_segment(start: impl Into<String>, end: impl Into<String>) -> Self {
        Self {
            start: Some(start.into()),
            end: Some(end.into()),
            ..Self::empty()
        }
    }

    fn empty() -> Self {
        Self {
            x: None,
            y: None,
            fixed: None,
            start: None,
            end: None,
            center: None,
            radius: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchEntity {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub params: SketchParam,
}

impl SketchEntity {
    pub fn point_2d(id: impl Into<String>, x: f64, y: f64) -> Self {
        Self {
            id: id.into(),
            kind: "point_2d".to_string(),
            params: SketchParam::point_2d(x, y),
        }
    }

    pub fn fixed_point_2d(id: impl Into<String>, x: f64, y: f64) -> Self {
        Self {
            id: id.into(),
            kind: "point_2d".to_string(),
            params: SketchParam::point_2d_fixed(x, y, true),
        }
    }

    pub fn line_segment_2d(id: impl Into<String>, start: impl Into<String>, end: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: "line_segment_2d".to_string(),
            params: SketchParam::line_segment(start, end),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchConstraint {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

impl SketchConstraint {
    pub fn coincident(id: impl Into<String>, a: impl Into<String>, b: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: "coincident".to_string(),
            entities: vec![a.into(), b.into()],
            value: None,
        }
    }

    pub fn distance(id: impl Into<String>, a: impl Into<String>, b: impl Into<String>, value: f64) -> Self {
        Self {
            id: id.into(),
            kind: "distance".to_string(),
            entities: vec![a.into(), b.into()],
            value: Some(value),
        }
    }

    pub fn horizontal(id: impl Into<String>, entity: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: "horizontal".to_string(),
            entities: vec![entity.into()],
            value: None,
        }
    }

    pub fn vertical(id: impl Into<String>, entity: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: "vertical".to_string(),
            entities: vec![entity.into()],
            value: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchRequest {
    pub schema_version: String,
    pub request_id: String,
    pub entities: Vec<SketchEntity>,
    pub constraints: Vec<SketchConstraint>,
}

impl SketchRequest {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            entities: Vec::new(),
            constraints: Vec::new(),
        }
    }

    pub fn with_entity(mut self, entity: SketchEntity) -> Self {
        self.entities.push(entity);
        self
    }

    pub fn with_constraint(mut self, constraint: SketchConstraint) -> Self {
        self.constraints.push(constraint);
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for entity in &self.entities {
            if entity.id.is_empty() || entity.id.len() > 64 {
                return Err(format!("entity id {:?} is not a valid identifier", entity.id));
            }
            if !entity.id.chars().all(|c| {
                c.is_ascii_alphanumeric() || c == '_' || c == '-'
            }) {
                return Err(format!("entity id {:?} contains invalid characters", entity.id));
            }
            if !seen.insert(entity.id.clone()) {
                return Err(format!("duplicate id {:?}", entity.id));
            }
        }
        for constraint in &self.constraints {
            if constraint.id.is_empty() || constraint.id.len() > 64 {
                return Err(format!(
                    "constraint id {:?} is not a valid identifier",
                    constraint.id
                ));
            }
            if !seen.insert(constraint.id.clone()) {
                return Err(format!("duplicate id {:?}", constraint.id));
            }
            for ref_id in &constraint.entities {
                if !seen.contains(ref_id) {
                    return Err(format!(
                        "constraint {:?} references unknown entity {:?}",
                        constraint.id, ref_id
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Worker response envelope. `status` is one of:
/// * `"ok"` — solver returned `SLVS_RESULT_OKAY`.
/// * `"inconsistent"` — solver returned `SLVS_RESULT_INCONSISTENT`.
/// * `"nonconvergent"` — solver returned `SLVS_RESULT_DIDNT_CONVERGE`.
/// * `"rank_deficient"` — solver returned `SLVS_RESULT_TOO_MANY_UNKNOWNS`.
/// * `"redundant_okay"` — solver returned `SLVS_RESULT_REDUNDANT_OKAY`.
/// * `"internal_error"` — solver returned an unexpected result code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolveResult {
    pub schema_version: String,
    pub request_id: String,
    pub status: String,
    pub dof: i64,
    pub resolved_entity_ids: Vec<String>,
    pub failed_constraint_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<BTreeMap<String, [f64; 2]>>,
}

impl SolveResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok" || self.status == "redundant_okay"
    }

    pub fn is_fully_constrained(&self) -> bool {
        self.is_success() && self.dof == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_unknown_schema_version() {
        let request = SketchRequest {
            schema_version: "threeterm.workers.slvs/0".to_string(),
            request_id: "req-1".to_string(),
            entities: Vec::new(),
            constraints: Vec::new(),
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_unknown_entity_reference() {
        let request = SketchRequest::new("req-1")
            .with_entity(SketchEntity::point_2d("p1", 0.0, 0.0))
            .with_constraint(SketchConstraint::coincident("k1", "p1", "p2"));
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_accepts_canonical_rectangle() {
        let request = SketchRequest::new("req-1")
            .with_entity(SketchEntity::fixed_point_2d("p1", 0.0, 0.0))
            .with_entity(SketchEntity::point_2d("p2", 10.0, 0.0))
            .with_entity(SketchEntity::point_2d("p3", 10.0, 5.0))
            .with_entity(SketchEntity::point_2d("p4", 0.0, 5.0))
            .with_entity(SketchEntity::line_segment_2d("l1", "p1", "p2"))
            .with_entity(SketchEntity::line_segment_2d("l2", "p2", "p3"))
            .with_entity(SketchEntity::line_segment_2d("l3", "p3", "p4"))
            .with_entity(SketchEntity::line_segment_2d("l4", "p4", "p1"))
            .with_constraint(SketchConstraint::coincident("c12", "p1", "p2"))
            .with_constraint(SketchConstraint::coincident("c23", "p2", "p3"))
            .with_constraint(SketchConstraint::coincident("c34", "p3", "p4"))
            .with_constraint(SketchConstraint::coincident("c41", "p4", "p1"))
            .with_constraint(SketchConstraint::horizontal("h1", "l1"))
            .with_constraint(SketchConstraint::vertical("v2", "l2"))
            .with_constraint(SketchConstraint::distance("dw", "p1", "p3", 10.0))
            .with_constraint(SketchConstraint::distance("dh", "p1", "p4", 5.0));
        request.validate().expect("rectangle envelope is valid");
    }

    #[test]
    fn solve_result_status_classification() {
        let mut result = SolveResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            status: "ok".to_string(),
            dof: 0,
            resolved_entity_ids: Vec::new(),
            failed_constraint_ids: Vec::new(),
            coordinates: None,
        };
        assert!(result.is_success());
        assert!(result.is_fully_constrained());

        result.status = "inconsistent".to_string();
        assert!(!result.is_success());

        result.status = "ok".to_string();
        result.dof = 1;
        assert!(result.is_success());
        assert!(!result.is_fully_constrained());
    }
}
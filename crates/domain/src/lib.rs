use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod graph {
    pub use super::{Feature, FeatureGraph, FeatureId, ProjectGeneration, Revision};
}

pub mod component {
    pub use super::{
        ComponentCommand, ComponentDefinition, ComponentGraph, ComponentInstance,
        EdgeGeometricEvidence, EdgeProvenance, EdgeReattachmentOutcome, LBracketDescriptor,
        PostEditEdgeCandidate, ReferenceOutcome, SelectedEdgeReference, SemanticReference,
        resolve_edge_reference, resolve_semantic_reference,
    };
}

pub mod history;

pub mod sketch {
    pub use super::{
        PlanarFaceCandidate, PlanarFaceEvidence, PlanarFaceProvenance,
        PlanarFaceReattachmentOutcome, PlanarFaceReference, SketchConstraint, SketchDiagnostic,
        SketchEntity, SketchPayload, SketchPlacement, SolvedCoordinate,
        resolve_planar_face_reference,
    };
}

pub fn schema_version() -> &'static str {
    "threeterm.domain/1"
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureId(String);

impl FeatureId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainError::EmptyId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub kind: String,
}

impl Feature {
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Result<Self, DomainError> {
        let kind = kind.into();
        if kind.is_empty() {
            return Err(DomainError::EmptyKind);
        }
        Ok(Self {
            id: FeatureId::new(id)?,
            kind,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureGraph {
    features: BTreeMap<FeatureId, String>,
    #[serde(default)]
    sketches: BTreeMap<FeatureId, SketchPayload>,
    #[serde(default)]
    fit_dimensions: BTreeMap<String, FitDimension>,
}

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
pub struct SolvedCoordinate {
    pub entity_id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchDiagnostic {
    pub code: String,
    pub detail: String,
    #[serde(default)]
    pub constraint_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanarFaceEvidence {
    pub topology_kind: String,
    pub origin: [f64; 3],
    pub normal: [f64; 3],
    pub x_axis: [f64; 3],
    pub y_axis: [f64; 3],
    #[serde(default)]
    pub adjacent_feature_ids: Vec<String>,
}

impl PlanarFaceEvidence {
    pub fn validate(&self) -> Result<(), String> {
        if self.topology_kind != "planar_face" {
            return Err("face evidence topology kind must be planar_face".to_string());
        }
        validate_frame(
            self.origin,
            self.x_axis,
            self.y_axis,
            self.normal,
            "face evidence",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanarFaceProvenance {
    pub source_feature_id: String,
    pub source_revision_id: String,
    pub source_face_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanarFaceReference {
    pub semantic_id: String,
    pub provenance: PlanarFaceProvenance,
    pub role: String,
    pub evidence: PlanarFaceEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanarFaceCandidate {
    pub semantic_id: String,
    pub provenance: PlanarFaceProvenance,
    pub role: String,
    pub evidence: PlanarFaceEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanarFaceReattachmentOutcome {
    Resolved { semantic_id: String },
    Ambiguous { candidate_ids: Vec<String> },
    Lost,
    Incompatible { candidate_ids: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchPlacement {
    pub origin: [f64; 3],
    pub x_axis: [f64; 3],
    pub y_axis: [f64; 3],
    pub normal: [f64; 3],
}

impl SketchPlacement {
    pub fn validate(&self) -> Result<(), String> {
        validate_frame(
            self.origin,
            self.x_axis,
            self.y_axis,
            self.normal,
            "sketch placement",
        )
    }

    pub fn transform_point(&self, point: [f64; 2]) -> [f64; 3] {
        [
            self.origin[0] + point[0] * self.x_axis[0] + point[1] * self.y_axis[0],
            self.origin[1] + point[0] * self.x_axis[1] + point[1] * self.y_axis[1],
            self.origin[2] + point[0] * self.x_axis[2] + point[1] * self.y_axis[2],
        ]
    }
}

impl Eq for SketchPlacement {}

fn validate_frame(
    origin: [f64; 3],
    x_axis: [f64; 3],
    y_axis: [f64; 3],
    normal: [f64; 3],
    label: &str,
) -> Result<(), String> {
    if !origin
        .into_iter()
        .chain(x_axis)
        .chain(y_axis)
        .chain(normal)
        .all(f64::is_finite)
    {
        return Err(format!("{label} frame must contain finite values"));
    }
    let norm = |vector: [f64; 3]| {
        vector
            .into_iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt()
    };
    let dot = |left: [f64; 3], right: [f64; 3]| {
        left.into_iter().zip(right).map(|(a, b)| a * b).sum::<f64>()
    };
    if (norm(x_axis) - 1.0).abs() > 1e-6
        || (norm(y_axis) - 1.0).abs() > 1e-6
        || (norm(normal) - 1.0).abs() > 1e-6
        || dot(x_axis, y_axis).abs() > 1e-6
        || dot(x_axis, normal).abs() > 1e-6
        || dot(y_axis, normal).abs() > 1e-6
    {
        return Err(format!("{label} frame must be orthonormal"));
    }
    let cross = [
        x_axis[1] * y_axis[2] - x_axis[2] * y_axis[1],
        x_axis[2] * y_axis[0] - x_axis[0] * y_axis[2],
        x_axis[0] * y_axis[1] - x_axis[1] * y_axis[0],
    ];
    if dot(cross, normal) < 1.0 - 1e-6 {
        return Err(format!("{label} frame must be right-handed"));
    }
    Ok(())
}

pub fn resolve_planar_face_reference(
    reference: &PlanarFaceReference,
    candidates: impl IntoIterator<Item = PlanarFaceCandidate>,
) -> PlanarFaceReattachmentOutcome {
    if reference.semantic_id.is_empty()
        || reference.role.is_empty()
        || reference.provenance.source_feature_id.is_empty()
        || reference.provenance.source_revision_id.is_empty()
        || reference.provenance.source_face_id.is_empty()
        || reference.provenance.source_face_id != reference.semantic_id
        || reference.evidence.validate().is_err()
    {
        return PlanarFaceReattachmentOutcome::Incompatible {
            candidate_ids: Vec::new(),
        };
    }
    let candidates: Vec<_> = candidates.into_iter().collect();
    if candidates.iter().any(|candidate| {
        candidate.semantic_id.is_empty()
            || candidate.role.is_empty()
            || candidate.provenance.source_feature_id.is_empty()
            || candidate.provenance.source_revision_id.is_empty()
            || candidate.provenance.source_face_id.is_empty()
            || candidate.evidence.validate().is_err()
    }) {
        return PlanarFaceReattachmentOutcome::Incompatible {
            candidate_ids: candidate_ids(&candidates),
        };
    }
    let lineage: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.provenance == reference.provenance)
        .collect::<Vec<_>>();
    if lineage.is_empty() {
        return PlanarFaceReattachmentOutcome::Lost;
    }
    let role_matches: Vec<_> = lineage
        .iter()
        .filter(|candidate| candidate.role == reference.role)
        .copied()
        .collect();
    if role_matches.is_empty() {
        return PlanarFaceReattachmentOutcome::Incompatible {
            candidate_ids: candidate_ids(&lineage),
        };
    }
    let geometric_matches: Vec<_> = role_matches
        .into_iter()
        .filter(|candidate| frame_matches(&reference.evidence, &candidate.evidence))
        .collect();
    let ids = geometric_matches
        .iter()
        .map(|candidate| candidate.semantic_id.clone())
        .collect::<Vec<_>>();
    let mut ids = ids;
    ids.sort();
    match geometric_matches.as_slice() {
        [candidate] => PlanarFaceReattachmentOutcome::Resolved {
            semantic_id: candidate.semantic_id.clone(),
        },
        [] => PlanarFaceReattachmentOutcome::Incompatible {
            candidate_ids: candidate_ids(&lineage),
        },
        _ => PlanarFaceReattachmentOutcome::Ambiguous { candidate_ids: ids },
    }
}

fn candidate_ids(candidates: &[impl std::borrow::Borrow<PlanarFaceCandidate>]) -> Vec<String> {
    let mut ids = candidates
        .iter()
        .map(|candidate| candidate.borrow().semantic_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

fn frame_matches(left: &PlanarFaceEvidence, right: &PlanarFaceEvidence) -> bool {
    left.origin
        .into_iter()
        .zip(right.origin)
        .all(|(a, b)| (a - b).abs() <= 1e-6)
        && left
            .normal
            .into_iter()
            .zip(right.normal)
            .all(|(a, b)| (a - b).abs() <= 1e-6)
        && left
            .x_axis
            .into_iter()
            .zip(right.x_axis)
            .all(|(a, b)| (a - b).abs() <= 1e-6)
        && left
            .y_axis
            .into_iter()
            .zip(right.y_axis)
            .all(|(a, b)| (a - b).abs() <= 1e-6)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SketchPayload {
    pub feature_id: String,
    pub entities: Vec<SketchEntity>,
    pub constraints: Vec<SketchConstraint>,
    pub status: String,
    pub dof: i32,
    pub entity_ids: Vec<String>,
    pub related_constraint_ids: Vec<String>,
    pub diagnostics: Vec<SketchDiagnostic>,
    #[serde(default)]
    pub solved_coordinates: Option<Vec<SolvedCoordinate>>,
    #[serde(default)]
    pub support: Option<PlanarFaceReference>,
    #[serde(default)]
    pub placement: Option<SketchPlacement>,
}

impl Eq for SketchPayload {}

impl SketchPayload {
    pub fn validate(&self) -> Result<(), String> {
        if self.feature_id.is_empty() || self.entities.is_empty() {
            return Err("sketch feature and entities must not be empty".to_string());
        }
        if self.support.is_some() != self.placement.is_some() {
            return Err("sketch support and placement must be provided together".to_string());
        }
        if let Some(support) = &self.support {
            if support.evidence.validate().is_err()
                || support.semantic_id.is_empty()
                || support.role.is_empty()
                || support.provenance.source_feature_id.is_empty()
                || support.provenance.source_revision_id.is_empty()
                || support.provenance.source_face_id != support.semantic_id
            {
                return Err("sketch support reference is invalid".to_string());
            }
            self.placement
                .as_ref()
                .expect("support and placement presence checked")
                .validate()?;
        }
        if self.dof < 0 {
            return Err("sketch dof must not be negative".to_string());
        }
        let mut ids = std::collections::BTreeSet::new();
        for entity in &self.entities {
            let id = match entity {
                SketchEntity::Point { id, x, y } => {
                    if !x.is_finite() || !y.is_finite() {
                        return Err("sketch point coordinates must be finite".to_string());
                    }
                    id
                }
                SketchEntity::LineSegment { id, start, end } => {
                    if start == end {
                        return Err("sketch line endpoints must differ".to_string());
                    }
                    if !self
                        .entities
                        .iter()
                        .any(|candidate| entity_id(candidate) == start)
                        || !self
                            .entities
                            .iter()
                            .any(|candidate| entity_id(candidate) == end)
                    {
                        return Err("sketch line references an unknown point".to_string());
                    }
                    id
                }
                SketchEntity::Circle { id, center, radius } => {
                    if !self
                        .entities
                        .iter()
                        .any(|candidate| entity_id(candidate) == center)
                        || !radius.is_finite()
                        || *radius <= 0.0
                    {
                        return Err("sketch circle must have a valid center and radius".to_string());
                    }
                    id
                }
                SketchEntity::Arc {
                    id,
                    center,
                    start,
                    end,
                } => {
                    if !self
                        .entities
                        .iter()
                        .any(|candidate| entity_id(candidate) == center)
                        || !self
                            .entities
                            .iter()
                            .any(|candidate| entity_id(candidate) == start)
                        || !self
                            .entities
                            .iter()
                            .any(|candidate| entity_id(candidate) == end)
                    {
                        return Err("sketch arc references an unknown point".to_string());
                    }
                    id
                }
            };
            if id.is_empty() || !ids.insert(id.clone()) {
                return Err("sketch entity IDs must be unique".to_string());
            }
        }
        for constraint in &self.constraints {
            if constraint.id.is_empty() || !ids.insert(constraint.id.clone()) {
                return Err("sketch entity and constraint IDs must be globally unique".to_string());
            }
            if constraint.value.is_some_and(|value| !value.is_finite()) {
                return Err("sketch constraint values must be finite".to_string());
            }
            if constraint.entities.iter().any(|reference| {
                !self.entities.iter().any(|entity| match entity {
                    SketchEntity::Point { id, .. }
                    | SketchEntity::LineSegment { id, .. }
                    | SketchEntity::Circle { id, .. }
                    | SketchEntity::Arc { id, .. } => id == reference,
                })
            }) {
                return Err("sketch constraint references an unknown entity".to_string());
            }
        }
        if !matches!(
            self.status.as_str(),
            "solved"
                | "underconstrained"
                | "redundant"
                | "inconsistent"
                | "nonconvergent"
                | "invalid_request"
        ) {
            return Err("sketch status is not normalized".to_string());
        }
        let expected_entity_ids: Vec<_> = self.entities.iter().map(entity_id).cloned().collect();
        if self.entity_ids != expected_entity_ids {
            return Err("sketch entity_ids must match entity order".to_string());
        }
        if self.status == "solved" && self.dof != 0 {
            return Err("solved sketches must have zero degrees of freedom".to_string());
        }
        if self.status == "solved" {
            let coordinates = self
                .solved_coordinates
                .as_ref()
                .ok_or_else(|| "solved sketches require coordinates".to_string())?;
            let point_ids: std::collections::BTreeSet<_> = self
                .entities
                .iter()
                .filter_map(|entity| match entity {
                    SketchEntity::Point { id, .. } => Some(id.as_str()),
                    _ => None,
                })
                .collect();
            let coordinate_ids: std::collections::BTreeSet<_> = coordinates
                .iter()
                .map(|coordinate| coordinate.entity_id.as_str())
                .collect();
            if coordinate_ids != point_ids
                || coordinates.len() != coordinate_ids.len()
                || coordinates
                    .iter()
                    .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
            {
                return Err("solved sketch coordinates must be finite".to_string());
            }
        } else if self.solved_coordinates.is_some() {
            return Err("failed sketches must not carry solved coordinates".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitDimension {
    pub id: String,
    pub source_feature_id: String,
    pub target_feature_id: String,
    pub source_dimension_id: String,
    pub target_dimension_id: String,
    pub dimension: String,
    pub source_value: f64,
    pub target_value: f64,
    pub clearance: f64,
}

impl Eq for FitDimension {}

impl FitDimension {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.source_feature_id.is_empty()
            || self.target_feature_id.is_empty()
            || self.source_dimension_id.is_empty()
            || self.target_dimension_id.is_empty()
            || self.dimension.is_empty()
        {
            return Err("fit dimension IDs and dimension must not be empty".to_string());
        }
        if self.source_feature_id == self.target_feature_id {
            return Err("fit dimension source and target must differ".to_string());
        }
        if ![self.source_value, self.target_value, self.clearance]
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
        {
            return Err("fit dimension values must be strictly positive and finite".to_string());
        }
        let expected_target = self.source_value - 2.0 * self.clearance;
        if expected_target <= 0.0 || (expected_target - self.target_value).abs() > 1e-9 {
            return Err(
                "fit dimension target must equal source dimension minus twice the clearance"
                    .to_string(),
            );
        }
        Ok(())
    }
}

fn entity_id(entity: &SketchEntity) -> &String {
    match entity {
        SketchEntity::Point { id, .. }
        | SketchEntity::LineSegment { id, .. }
        | SketchEntity::Circle { id, .. }
        | SketchEntity::Arc { id, .. } => id,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LBracketDescriptor {
    pub feature_id: String,
    pub length: f64,
    pub width: f64,
    pub height: f64,
    pub thickness: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDefinition {
    pub id: String,
    #[serde(default)]
    pub selected_feature_ids: Vec<String>,
    pub descriptor: LBracketDescriptor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentInstance {
    pub id: String,
    pub definition_id: String,
    pub transform: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ComponentCommand {
    Define {
        definition: ComponentDefinition,
    },
    Capture {
        definition_id: String,
        selected_feature_ids: Vec<String>,
        descriptor: LBracketDescriptor,
    },
    CreateInstance {
        instance: ComponentInstance,
    },
    TransformInstance {
        instance_id: String,
        transform: [f64; 3],
    },
    MakeIndependent {
        source_instance_id: String,
        definition_id: String,
        instance_id: String,
        feature_id: String,
    },
    EditParameter {
        definition_id: String,
        parameter: String,
        value: f64,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentGraph {
    pub definitions: BTreeMap<String, ComponentDefinition>,
    pub instances: BTreeMap<String, ComponentInstance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticReference {
    pub id: String,
    pub expected_kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceOutcome {
    Resolved(String),
    Ambiguous,
    Lost,
    Incompatible,
}

pub const EDGE_MIDPOINT_TOLERANCE: f64 = 1e-6;
pub const EDGE_LENGTH_TOLERANCE: f64 = 1e-6;
pub const EDGE_TANGENT_TOLERANCE: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeGeometricEvidence {
    pub midpoint: [f64; 3],
    pub tangent: [f64; 3],
    pub length: f64,
}

impl EdgeGeometricEvidence {
    pub fn validate(&self) -> Result<(), String> {
        if !self
            .midpoint
            .into_iter()
            .chain(self.tangent)
            .all(f64::is_finite)
        {
            return Err("edge geometric evidence must be finite".to_string());
        }
        if !self.length.is_finite() || self.length <= 0.0 {
            return Err("edge geometric evidence length must be positive and finite".to_string());
        }
        let tangent_length = self
            .tangent
            .into_iter()
            .map(|value| value * value)
            .sum::<f64>();
        if tangent_length <= f64::EPSILON {
            return Err("edge geometric evidence tangent must not be zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeProvenance {
    pub source_feature_id: String,
    pub source_revision_id: String,
    pub source_edge_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedEdgeReference {
    pub semantic_id: String,
    pub provenance: EdgeProvenance,
    pub role: String,
    pub evidence: EdgeGeometricEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostEditEdgeCandidate {
    pub semantic_id: String,
    pub provenance: EdgeProvenance,
    pub role: String,
    pub evidence: EdgeGeometricEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum EdgeReattachmentOutcome {
    Resolved { semantic_id: String },
    Ambiguous { candidate_ids: Vec<String> },
    Lost,
    Incompatible { candidate_ids: Vec<String> },
}

/// Reattach one selected edge without consulting topology order or position.
/// Provenance narrows the lineage first; role and geometric evidence then
/// decide whether that lineage is safe to use.
pub fn resolve_edge_reference(
    reference: &SelectedEdgeReference,
    candidates: impl IntoIterator<Item = PostEditEdgeCandidate>,
) -> EdgeReattachmentOutcome {
    if reference.semantic_id.is_empty()
        || reference.role.is_empty()
        || reference.provenance.source_feature_id.is_empty()
        || reference.provenance.source_revision_id.is_empty()
        || reference.provenance.source_edge_id.is_empty()
        || reference.provenance.source_edge_id != reference.semantic_id
        || reference.evidence.validate().is_err()
    {
        return EdgeReattachmentOutcome::Incompatible {
            candidate_ids: Vec::new(),
        };
    }

    let candidates: Vec<_> = candidates.into_iter().collect();
    if candidates.iter().any(|candidate| {
        candidate.semantic_id.is_empty()
            || candidate.role.is_empty()
            || candidate.provenance.source_feature_id.is_empty()
            || candidate.provenance.source_revision_id.is_empty()
            || candidate.provenance.source_edge_id.is_empty()
            || candidate.evidence.validate().is_err()
    }) {
        return EdgeReattachmentOutcome::Incompatible {
            candidate_ids: sorted_candidate_ids(&candidates),
        };
    }

    let lineage: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.provenance == reference.provenance)
        .collect();
    if lineage.is_empty() {
        return EdgeReattachmentOutcome::Lost;
    }

    let role_matches: Vec<_> = lineage
        .iter()
        .filter(|candidate| candidate.role == reference.role)
        .copied()
        .collect();
    if role_matches.is_empty() {
        return EdgeReattachmentOutcome::Incompatible {
            candidate_ids: sorted_candidate_ids(&lineage),
        };
    }

    let geometric_matches: Vec<_> = role_matches
        .into_iter()
        .filter(|candidate| geometric_match(&reference.evidence, &candidate.evidence))
        .collect();
    let candidate_ids = sorted_candidate_ids(&geometric_matches);
    match geometric_matches.as_slice() {
        [candidate] => EdgeReattachmentOutcome::Resolved {
            semantic_id: candidate.semantic_id.clone(),
        },
        [] => EdgeReattachmentOutcome::Incompatible {
            candidate_ids: sorted_candidate_ids(&lineage),
        },
        _ => EdgeReattachmentOutcome::Ambiguous { candidate_ids },
    }
}

/// Resolve a selected edge against descendants produced by a real split.
/// A split edge is represented by its actual fragments, so the fragments do
/// not individually match the source length and midpoint. They remain
/// ambiguous when their shared lineage, role, direction, and contiguous
/// geometry reconstruct the selected source edge.
pub fn resolve_split_edge_reference(
    reference: &SelectedEdgeReference,
    candidates: impl IntoIterator<Item = PostEditEdgeCandidate>,
) -> EdgeReattachmentOutcome {
    let candidates: Vec<_> = candidates.into_iter().collect();
    let outcome = resolve_edge_reference(reference, candidates.clone());
    if !matches!(outcome, EdgeReattachmentOutcome::Incompatible { .. }) {
        return outcome;
    }
    if reference.semantic_id.is_empty()
        || reference.role.is_empty()
        || reference.provenance.source_feature_id.is_empty()
        || reference.provenance.source_revision_id.is_empty()
        || reference.provenance.source_edge_id.is_empty()
        || reference.provenance.source_edge_id != reference.semantic_id
        || reference.evidence.validate().is_err()
    {
        return outcome;
    }
    let lineage: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            candidate.provenance == reference.provenance && candidate.role == reference.role
        })
        .collect();
    if lineage.len() >= 2 && split_fragments_reconstruct_reference(reference, &lineage) {
        return EdgeReattachmentOutcome::Ambiguous {
            candidate_ids: sorted_candidate_ids(&lineage),
        };
    }
    outcome
}

fn sorted_candidate_ids(
    candidates: &[impl std::borrow::Borrow<PostEditEdgeCandidate>],
) -> Vec<String> {
    let mut ids: Vec<_> = candidates
        .iter()
        .map(|candidate| candidate.borrow().semantic_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn geometric_match(reference: &EdgeGeometricEvidence, candidate: &EdgeGeometricEvidence) -> bool {
    let midpoint_delta = reference
        .midpoint
        .into_iter()
        .zip(candidate.midpoint)
        .map(|(left, right)| (left - right) * (left - right))
        .sum::<f64>()
        .sqrt();
    let reference_tangent_length = reference
        .tangent
        .into_iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    let candidate_tangent_length = candidate
        .tangent
        .into_iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    let tangent_dot = reference
        .tangent
        .into_iter()
        .zip(candidate.tangent)
        .map(|(left, right)| left * right)
        .sum::<f64>()
        / (reference_tangent_length * candidate_tangent_length);
    midpoint_delta <= EDGE_MIDPOINT_TOLERANCE
        && (reference.length - candidate.length).abs() <= EDGE_LENGTH_TOLERANCE
        && 1.0 - tangent_dot.abs() <= EDGE_TANGENT_TOLERANCE
}

fn split_fragments_reconstruct_reference(
    reference: &SelectedEdgeReference,
    candidates: &[&PostEditEdgeCandidate],
) -> bool {
    let direction_length = reference
        .evidence
        .tangent
        .into_iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if direction_length <= f64::EPSILON {
        return false;
    }
    let direction = reference
        .evidence
        .tangent
        .map(|value| value / direction_length);
    let half_length = reference.evidence.length / 2.0;
    let mut intervals: Vec<_> = candidates
        .iter()
        .filter_map(|candidate| {
            let tangent_length = candidate
                .evidence
                .tangent
                .into_iter()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt();
            if tangent_length <= f64::EPSILON {
                return None;
            }
            let tangent = candidate
                .evidence
                .tangent
                .map(|value| value / tangent_length);
            let tangent_dot = direction
                .into_iter()
                .zip(tangent)
                .map(|(left, right)| left * right)
                .sum::<f64>();
            let offset = candidate
                .evidence
                .midpoint
                .into_iter()
                .zip(reference.evidence.midpoint)
                .map(|(candidate, reference)| candidate - reference)
                .collect::<Vec<_>>();
            let along = offset
                .iter()
                .zip(direction)
                .map(|(offset, direction)| offset * direction)
                .sum::<f64>();
            let perpendicular_squared = offset
                .iter()
                .zip(direction)
                .map(|(offset, direction)| {
                    let perpendicular = offset - along * direction;
                    perpendicular * perpendicular
                })
                .sum::<f64>();
            if 1.0 - tangent_dot.abs() > EDGE_TANGENT_TOLERANCE
                || perpendicular_squared.sqrt() > EDGE_MIDPOINT_TOLERANCE
            {
                return None;
            }
            Some((
                along - candidate.evidence.length / 2.0,
                along + candidate.evidence.length / 2.0,
            ))
        })
        .collect();
    if intervals.len() < 2 {
        return false;
    }
    intervals.sort_by(|left, right| left.0.total_cmp(&right.0));
    if intervals[0].0 > -half_length + EDGE_LENGTH_TOLERANCE {
        return false;
    }
    let mut covered_end = intervals[0].1;
    for (start, end) in intervals.into_iter().skip(1) {
        if start > covered_end + EDGE_LENGTH_TOLERANCE {
            return false;
        }
        covered_end = covered_end.max(end);
    }
    covered_end >= half_length - EDGE_LENGTH_TOLERANCE
}

/// Resolve only stable semantic identities. The caller supplies semantic
/// candidates; topology positions or indexes are never considered.
pub fn resolve_semantic_reference(
    reference: &SemanticReference,
    candidates: impl IntoIterator<Item = (String, &'static str)>,
) -> ReferenceOutcome {
    let matches: Vec<_> = candidates
        .into_iter()
        .filter(|(id, _)| id == &reference.id)
        .collect();
    match matches.as_slice() {
        [] => ReferenceOutcome::Lost,
        [(_, kind)] if *kind == reference.expected_kind => {
            ReferenceOutcome::Resolved(reference.id.clone())
        }
        [..] if matches.len() == 1 => ReferenceOutcome::Incompatible,
        _ => ReferenceOutcome::Ambiguous,
    }
}

impl ComponentGraph {
    pub fn apply(&mut self, command: &ComponentCommand) -> Result<(), String> {
        match command {
            ComponentCommand::Define { definition } => {
                if definition.id.is_empty() || definition.descriptor.feature_id.is_empty() {
                    return Err("component IDs must not be empty".to_string());
                }
                validate_selected_feature_ids(&definition.selected_feature_ids)?;
                if ![
                    definition.descriptor.length,
                    definition.descriptor.width,
                    definition.descriptor.height,
                    definition.descriptor.thickness,
                ]
                .iter()
                .all(|value| value.is_finite() && *value > 0.0)
                {
                    return Err(
                        "component dimensions must be strictly positive finite numbers".to_string(),
                    );
                }
                if self.id_is_in_use(&definition.id) {
                    return Err("component ID already exists".to_string());
                }
                if self.feature_id_is_in_use(&definition.descriptor.feature_id) {
                    return Err("component feature ID already exists".to_string());
                }
                self.definitions
                    .insert(definition.id.clone(), definition.clone());
            }
            ComponentCommand::Capture {
                definition_id,
                selected_feature_ids,
                descriptor,
            } => {
                self.apply(&ComponentCommand::Define {
                    definition: ComponentDefinition {
                        id: definition_id.clone(),
                        selected_feature_ids: selected_feature_ids.clone(),
                        descriptor: descriptor.clone(),
                    },
                })?;
            }
            ComponentCommand::CreateInstance { instance } => {
                self.require_definition(&instance.definition_id)?;
                if self.id_is_in_use(&instance.id) {
                    return Err("component ID already exists".to_string());
                }
                self.instances.insert(instance.id.clone(), instance.clone());
            }
            ComponentCommand::TransformInstance {
                instance_id,
                transform,
            } => {
                if !transform.iter().all(|value| value.is_finite()) {
                    return Err("component transform must contain finite numbers".to_string());
                }
                self.require_instance(instance_id)?.transform = *transform;
            }
            ComponentCommand::MakeIndependent {
                source_instance_id,
                definition_id,
                instance_id,
                feature_id,
            } => {
                let source = self.require_instance(source_instance_id)?.clone();
                let mut definition = self.require_definition(&source.definition_id)?.clone();
                if definition_id == instance_id
                    || self.id_is_in_use(definition_id)
                    || self.id_is_in_use(instance_id)
                    || self.feature_id_is_in_use(feature_id)
                {
                    return Err("independent component IDs already exist".to_string());
                }
                definition.id = definition_id.clone();
                definition.descriptor.feature_id = feature_id.clone();
                self.definitions.insert(definition_id.clone(), definition);
                self.instances.insert(
                    instance_id.clone(),
                    ComponentInstance {
                        id: instance_id.clone(),
                        definition_id: definition_id.clone(),
                        transform: source.transform,
                    },
                );
            }
            ComponentCommand::EditParameter {
                definition_id,
                parameter,
                value,
            } => {
                if !value.is_finite() || *value <= 0.0 {
                    return Err(
                        "component parameter value must be a strictly positive finite number"
                            .to_string(),
                    );
                }
                let descriptor = &mut self.require_definition(definition_id)?.descriptor;
                match parameter.as_str() {
                    "length" => descriptor.length = *value,
                    "width" => descriptor.width = *value,
                    "height" => descriptor.height = *value,
                    "thickness" => descriptor.thickness = *value,
                    _ => return Err("unknown component parameter".to_string()),
                }
            }
        }
        Ok(())
    }

    fn require_definition(&mut self, id: &str) -> Result<&mut ComponentDefinition, String> {
        let outcome = resolve_semantic_reference(
            &SemanticReference {
                id: id.to_string(),
                expected_kind: "definition",
            },
            self.definitions
                .keys()
                .cloned()
                .map(|id| (id, "definition"))
                .chain(self.instances.keys().cloned().map(|id| (id, "instance"))),
        );
        match outcome {
            ReferenceOutcome::Resolved(_) => self
                .definitions
                .get_mut(id)
                .ok_or_else(|| "component definition reference is lost".to_string()),
            ReferenceOutcome::Ambiguous => {
                Err("component definition reference is ambiguous".to_string())
            }
            ReferenceOutcome::Lost => Err("component definition reference is lost".to_string()),
            ReferenceOutcome::Incompatible => {
                Err("component definition reference is incompatible".to_string())
            }
        }
    }

    fn require_instance(&mut self, id: &str) -> Result<&mut ComponentInstance, String> {
        let outcome = resolve_semantic_reference(
            &SemanticReference {
                id: id.to_string(),
                expected_kind: "instance",
            },
            self.definitions
                .keys()
                .cloned()
                .map(|id| (id, "definition"))
                .chain(self.instances.keys().cloned().map(|id| (id, "instance"))),
        );
        match outcome {
            ReferenceOutcome::Resolved(_) => self
                .instances
                .get_mut(id)
                .ok_or_else(|| "component instance reference is lost".to_string()),
            ReferenceOutcome::Ambiguous => {
                Err("component instance reference is ambiguous".to_string())
            }
            ReferenceOutcome::Lost => Err("component instance reference is lost".to_string()),
            ReferenceOutcome::Incompatible => {
                Err("component instance reference is incompatible".to_string())
            }
        }
    }

    fn id_is_in_use(&self, id: &str) -> bool {
        self.definitions.contains_key(id) || self.instances.contains_key(id)
    }

    fn feature_id_is_in_use(&self, feature_id: &str) -> bool {
        self.definitions
            .values()
            .any(|definition| definition.descriptor.feature_id == feature_id)
    }
}

fn validate_selected_feature_ids(feature_ids: &[String]) -> Result<(), String> {
    if feature_ids.iter().any(|feature_id| feature_id.is_empty())
        || feature_ids.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(
            "selected component feature IDs must be non-empty, unique, and sorted".to_string(),
        );
    }
    Ok(())
}

impl FeatureGraph {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Iterate canonical features in deterministic stable-ID order.
    pub fn features(&self) -> impl Iterator<Item = Feature> + '_ {
        self.features.iter().map(|(id, kind)| Feature {
            id: id.clone(),
            kind: kind.clone(),
        })
    }

    pub fn add_feature(&mut self, feature: Feature) -> bool {
        let previous = self.features.insert(feature.id, feature.kind.clone());
        previous.as_deref() != Some(feature.kind.as_str())
    }

    pub fn contains_feature(&self, feature_id: &str) -> bool {
        self.features.keys().any(|id| id.as_str() == feature_id)
    }

    pub fn set_feature(&mut self, feature: Feature) -> bool {
        self.add_feature(feature)
    }

    pub fn remove_feature(&mut self, feature_id: &str) -> bool {
        let Some(id) = self
            .features
            .keys()
            .find(|id| id.as_str() == feature_id)
            .cloned()
        else {
            return false;
        };
        self.features.remove(&id);
        self.sketches.remove(&id);
        true
    }

    pub fn add_sketch(&mut self, feature: Feature, sketch: SketchPayload) -> Result<bool, String> {
        if sketch.feature_id != feature.id.as_str() {
            return Err("sketch feature ID does not match its graph feature".to_string());
        }
        sketch.validate()?;
        let sketch_changed = self.sketches.get(&feature.id) != Some(&sketch);
        let changed = self.add_feature(feature.clone()) || sketch_changed;
        self.sketches.insert(feature.id, sketch);
        Ok(changed)
    }

    pub fn sketch(&self, feature_id: &str) -> Option<&SketchPayload> {
        self.sketches
            .iter()
            .find_map(|(id, sketch)| (id.as_str() == feature_id).then_some(sketch))
    }

    pub fn add_fit_dimension(&mut self, fit: FitDimension) -> Result<bool, String> {
        fit.validate()?;
        match self.fit_dimensions.get(&fit.id) {
            Some(existing) if existing == &fit => Ok(false),
            Some(_) => Err("fit dimension ID already exists with different values".to_string()),
            None => {
                self.fit_dimensions.insert(fit.id.clone(), fit);
                Ok(true)
            }
        }
    }

    pub fn fit_dimensions(&self) -> impl Iterator<Item = &FitDimension> {
        self.fit_dimensions.values()
    }

    pub fn graph_hash_hex(&self) -> String {
        let mut bytes = serde_json::to_vec(&self.features).expect("feature graph serializes");
        if !self.sketches.is_empty() {
            bytes.push(b'\n');
            bytes.extend_from_slice(
                &serde_json::to_vec(&self.sketches).expect("sketch graph serializes"),
            );
        }
        if !self.fit_dimensions.is_empty() {
            bytes.push(b'\n');
            bytes.extend_from_slice(
                &serde_json::to_vec(&self.fit_dimensions).expect("fit dimensions serialize"),
            );
        }
        hash_hex(&bytes)
    }

    pub fn revision_hash_hex(&self, terminal_log_digest_hex: &str) -> String {
        let mut bytes = self.graph_hash_hex().into_bytes();
        bytes.extend_from_slice(terminal_log_digest_hex.as_bytes());
        hash_hex(&bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub id: String,
    pub features: Vec<FeatureId>,
}

impl Revision {
    pub fn empty() -> Self {
        Self {
            id: "revision-0".to_string(),
            features: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGeneration {
    pub id: String,
    pub revisions: Vec<Revision>,
}

impl ProjectGeneration {
    pub fn fresh() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after unix epoch")
            .as_nanos();
        Self::with_id(format!("generation-{nanos}"))
    }

    pub fn with_id(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            revisions: vec![Revision::empty()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    EmptyId,
    EmptyKind,
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => formatter.write_str("feature id must not be empty"),
            Self::EmptyKind => formatter.write_str("feature kind must not be empty"),
        }
    }
}

impl std::error::Error for DomainError {}

fn hash_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.domain/1");
    }

    #[test]
    fn fresh_generation_contains_one_empty_revision() {
        let generation = ProjectGeneration::with_id("generation-test");
        assert_eq!(generation.id, "generation-test");
        assert_eq!(generation.revisions, vec![Revision::empty()]);
    }

    #[test]
    fn feature_graph_hash_is_deterministic_and_duplicate_add_is_idempotent() {
        let feature = Feature::new("box-1", "box").expect("feature is valid");
        let mut first = FeatureGraph::empty();
        assert!(first.add_feature(feature.clone()));
        let hash = first.graph_hash_hex();

        assert_eq!(hash.len(), 64);
        assert!(!first.add_feature(feature));
        assert_eq!(first.graph_hash_hex(), hash);

        let mut second = FeatureGraph::empty();
        assert!(second.add_feature(Feature::new("box-1", "box").expect("feature is valid")));
        assert_eq!(second.graph_hash_hex(), hash);
    }

    #[test]
    fn empty_graph_and_revision_hashes_are_pinned() {
        let graph = FeatureGraph::empty();
        assert_eq!(
            graph.graph_hash_hex(),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert_eq!(
            graph.revision_hash_hex(&"0".repeat(64)),
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7"
        );
    }

    #[test]
    fn revision_hash_changes_with_graph_or_log_digest() {
        let empty = FeatureGraph::empty();
        let empty_revision = empty.revision_hash_hex(&"0".repeat(64));
        let other_log_revision = empty.revision_hash_hex(&"1".repeat(64));
        assert_ne!(empty_revision, other_log_revision);

        let mut one_feature = FeatureGraph::empty();
        one_feature.add_feature(Feature::new("box-1", "box").expect("feature is valid"));
        assert_ne!(
            empty_revision,
            one_feature.revision_hash_hex(&"0".repeat(64))
        );
        assert_eq!(empty_revision.len(), 64);
    }

    #[test]
    fn fit_dimension_is_validated_and_included_in_graph_identity() {
        let fit = FitDimension {
            id: "fit:box:lid:width".to_string(),
            source_feature_id: "box-sketch".to_string(),
            target_feature_id: "lid-sketch".to_string(),
            source_dimension_id: "box-width".to_string(),
            target_dimension_id: "lid-width".to_string(),
            dimension: "width".to_string(),
            source_value: 10.0,
            target_value: 9.6,
            clearance: 0.2,
        };
        let mut graph = FeatureGraph::empty();
        let before = graph.graph_hash_hex();
        assert!(graph.add_fit_dimension(fit.clone()).expect("fit is valid"));
        assert_eq!(graph.fit_dimensions().collect::<Vec<_>>(), vec![&fit]);
        assert_ne!(graph.graph_hash_hex(), before);

        let invalid = FitDimension {
            target_value: 9.5,
            ..fit
        };
        assert!(graph.add_fit_dimension(invalid).is_err());
    }

    #[test]
    fn feature_id_rejects_empty_values() {
        assert_eq!(FeatureId::new(""), Err(DomainError::EmptyId));
    }

    #[test]
    fn feature_graph_supports_explicit_set_and_remove_operations() {
        let mut graph = FeatureGraph::empty();
        graph.add_feature(Feature::new("box", "cube").expect("feature is valid"));
        assert!(graph.contains_feature("box"));
        assert!(graph.set_feature(Feature::new("box", "sphere").expect("feature is valid")));
        assert_eq!(
            graph.features().next().expect("feature remains").kind,
            "sphere"
        );
        assert!(graph.remove_feature("box"));
        assert!(!graph.contains_feature("box"));
        assert!(!graph.remove_feature("box"));
    }

    #[test]
    fn semantic_reference_resolution_never_uses_topology_indexes() {
        let reference = SemanticReference {
            id: "stable-id".to_string(),
            expected_kind: "definition",
        };
        assert_eq!(
            resolve_semantic_reference(&reference, [("stable-id".to_string(), "definition")]),
            ReferenceOutcome::Resolved("stable-id".to_string())
        );
        assert_eq!(
            resolve_semantic_reference(&reference, [("stable-id".to_string(), "instance")]),
            ReferenceOutcome::Incompatible
        );
        assert_eq!(
            resolve_semantic_reference(
                &reference,
                [
                    ("stable-id".to_string(), "definition"),
                    ("stable-id".to_string(), "definition")
                ]
            ),
            ReferenceOutcome::Ambiguous
        );
        assert_eq!(
            resolve_semantic_reference(&reference, [("different".to_string(), "definition")]),
            ReferenceOutcome::Lost
        );
    }

    fn selected_edge() -> SelectedEdgeReference {
        SelectedEdgeReference {
            semantic_id: "edge-source".to_string(),
            provenance: EdgeProvenance {
                source_feature_id: "feature-before".to_string(),
                source_revision_id: "revision-before".to_string(),
                source_edge_id: "edge-source".to_string(),
            },
            role: "outer-perimeter".to_string(),
            evidence: EdgeGeometricEvidence {
                midpoint: [10.0, 2.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                length: 20.0,
            },
        }
    }

    fn candidate(id: &str) -> PostEditEdgeCandidate {
        PostEditEdgeCandidate {
            semantic_id: id.to_string(),
            provenance: selected_edge().provenance,
            role: "outer-perimeter".to_string(),
            evidence: selected_edge().evidence,
        }
    }

    #[test]
    fn edge_reference_resolves_by_provenance_role_and_geometry_not_candidate_order() {
        let reference = selected_edge();
        let mut candidates = vec![candidate("edge-new")];
        assert_eq!(
            resolve_edge_reference(&reference, candidates.clone()),
            EdgeReattachmentOutcome::Resolved {
                semantic_id: "edge-new".to_string()
            }
        );
        candidates.reverse();
        assert_eq!(
            resolve_edge_reference(&reference, candidates),
            EdgeReattachmentOutcome::Resolved {
                semantic_id: "edge-new".to_string()
            }
        );
    }

    #[test]
    fn edge_reference_reports_all_explicit_failure_outcomes() {
        let reference = selected_edge();

        let mut ambiguous = candidate("edge-a");
        ambiguous.evidence.midpoint[0] += 0.5;
        assert_eq!(
            resolve_edge_reference(&reference, [candidate("edge-a"), candidate("edge-b")]),
            EdgeReattachmentOutcome::Ambiguous {
                candidate_ids: vec!["edge-a".to_string(), "edge-b".to_string()]
            }
        );
        assert_eq!(
            resolve_edge_reference(
                &reference,
                [PostEditEdgeCandidate {
                    provenance: EdgeProvenance {
                        source_feature_id: "other-feature".to_string(),
                        ..candidate("edge-lost").provenance
                    },
                    ..candidate("edge-lost")
                }]
            ),
            EdgeReattachmentOutcome::Lost
        );
        let mut wrong_role = candidate("edge-wrong-role");
        wrong_role.role = "inner-loop".to_string();
        assert_eq!(
            resolve_edge_reference(&reference, [wrong_role]),
            EdgeReattachmentOutcome::Incompatible {
                candidate_ids: vec!["edge-wrong-role".to_string()]
            }
        );
        ambiguous.evidence.midpoint[0] = 11.0;
        assert_eq!(
            resolve_edge_reference(&reference, [ambiguous]),
            EdgeReattachmentOutcome::Incompatible {
                candidate_ids: vec!["edge-a".to_string()]
            }
        );
    }

    #[test]
    fn edge_reference_rejects_invalid_evidence_without_falling_back() {
        let mut invalid = candidate("edge-invalid");
        invalid.evidence.tangent = [0.0; 3];
        assert_eq!(
            resolve_edge_reference(&selected_edge(), [invalid]),
            EdgeReattachmentOutcome::Incompatible {
                candidate_ids: vec!["edge-invalid".to_string()]
            }
        );
    }

    #[test]
    fn split_edge_fragments_remain_ambiguous_when_they_reconstruct_the_source() {
        let reference = selected_edge();
        let mut first = candidate("edge-left");
        first.evidence.midpoint = [5.0, 2.0, 0.0];
        first.evidence.length = 10.0;
        let mut second = candidate("edge-right");
        second.evidence.midpoint = [15.0, 2.0, 0.0];
        second.evidence.length = 10.0;

        assert_eq!(
            resolve_split_edge_reference(&reference, [first, second]),
            EdgeReattachmentOutcome::Ambiguous {
                candidate_ids: vec!["edge-left".to_string(), "edge-right".to_string()]
            }
        );
    }

    #[test]
    fn component_ids_are_unique_across_definitions_and_instances() {
        let mut graph = ComponentGraph::default();
        graph
            .apply(&ComponentCommand::Define {
                definition: ComponentDefinition {
                    id: "bracket".to_string(),
                    selected_feature_ids: Vec::new(),
                    descriptor: LBracketDescriptor {
                        feature_id: "bracket-feature".to_string(),
                        length: 60.0,
                        width: 30.0,
                        height: 40.0,
                        thickness: 3.0,
                    },
                },
            })
            .expect("definition is valid");
        assert_eq!(
            graph.apply(&ComponentCommand::CreateInstance {
                instance: ComponentInstance {
                    id: "bracket".to_string(),
                    definition_id: "bracket".to_string(),
                    transform: [0.0, 0.0, 0.0],
                },
            }),
            Err("component ID already exists".to_string())
        );
    }

    #[test]
    fn planar_face_placement_maps_local_coordinates_and_resolves_by_evidence() {
        let evidence = PlanarFaceEvidence {
            topology_kind: "planar_face".to_string(),
            origin: [4.0, 5.0, 6.0],
            normal: [0.0, 1.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 0.0, -1.0],
            adjacent_feature_ids: Vec::new(),
        };
        let reference = PlanarFaceReference {
            semantic_id: "bracket/vertical-face".to_string(),
            provenance: PlanarFaceProvenance {
                source_feature_id: "bracket".to_string(),
                source_revision_id: "revision-1".to_string(),
                source_face_id: "bracket/vertical-face".to_string(),
            },
            role: "sketch-support".to_string(),
            evidence: evidence.clone(),
        };
        let placement = SketchPlacement {
            origin: evidence.origin,
            x_axis: evidence.x_axis,
            y_axis: evidence.y_axis,
            normal: evidence.normal,
        };

        placement.validate().expect("face frame is right-handed");
        assert_eq!(placement.transform_point([2.0, 3.0]), [6.0, 5.0, 3.0]);
        assert_eq!(
            resolve_planar_face_reference(
                &reference,
                [PlanarFaceCandidate {
                    semantic_id: reference.semantic_id.clone(),
                    provenance: reference.provenance.clone(),
                    role: reference.role.clone(),
                    evidence,
                }],
            ),
            PlanarFaceReattachmentOutcome::Resolved {
                semantic_id: "bracket/vertical-face".to_string(),
            }
        );
    }
}

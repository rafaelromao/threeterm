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
        LBracketDescriptor, ReferenceOutcome, SemanticReference, resolve_semantic_reference,
    };
}

pub mod history;

pub mod sketch {
    pub use super::{
        SketchConstraint, SketchDiagnostic, SketchEntity, SketchPayload, SolvedCoordinate,
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
}

impl Eq for SketchPayload {}

impl SketchPayload {
    pub fn validate(&self) -> Result<(), String> {
        if self.feature_id.is_empty() || self.entities.is_empty() {
            return Err("sketch feature and entities must not be empty".to_string());
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
                SketchEntity::LineSegment { id, .. }
                | SketchEntity::Circle { id, .. }
                | SketchEntity::Arc { id, .. } => id,
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
        if self.status == "solved" {
            let coordinates = self
                .solved_coordinates
                .as_ref()
                .ok_or_else(|| "solved sketches require coordinates".to_string())?;
            if coordinates
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
pub struct LBracketDescriptor {
    pub feature_id: String,
    pub length: f64,
    pub width: f64,
    pub height: f64,
    pub thickness: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentDefinition {
    pub id: String,
    #[serde(default)]
    pub selected_feature_ids: Vec<String>,
    pub descriptor: LBracketDescriptor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentInstance {
    pub id: String,
    pub definition_id: String,
    pub transform: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
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

    pub fn add_sketch(&mut self, feature: Feature, sketch: SketchPayload) -> Result<bool, String> {
        if sketch.feature_id != feature.id.as_str() {
            return Err("sketch feature ID does not match its graph feature".to_string());
        }
        sketch.validate()?;
        let changed = self.add_feature(feature.clone());
        self.sketches.insert(feature.id, sketch);
        Ok(changed)
    }

    pub fn sketch(&self, feature_id: &str) -> Option<&SketchPayload> {
        self.sketches
            .iter()
            .find_map(|(id, sketch)| (id.as_str() == feature_id).then_some(sketch))
    }

    pub fn graph_hash_hex(&self) -> String {
        let mut bytes = serde_json::to_vec(&self.features).expect("feature graph serializes");
        if !self.sketches.is_empty() {
            bytes.push(b'\n');
            bytes.extend_from_slice(
                &serde_json::to_vec(&self.sketches).expect("sketch graph serializes"),
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
    fn feature_id_rejects_empty_values() {
        assert_eq!(FeatureId::new(""), Err(DomainError::EmptyId));
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
}

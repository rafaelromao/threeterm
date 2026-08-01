use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod graph {
    pub use super::{
        CommandIntent, CommandTransaction, ComponentDefinition, ComponentDefinitionId,
        ComponentGraph, ComponentInstance, ComponentInstanceId, FeatureDescriptor, FeatureId,
        ProjectGeneration, ReattachmentOutcome, Revision, SemanticReference, Transform,
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
        non_empty_id(value.into()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ComponentDefinitionId(String);

impl ComponentDefinitionId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        non_empty_id(value.into()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ComponentInstanceId(String);

impl ComponentInstanceId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        non_empty_id(value.into()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn non_empty_id(value: String) -> Result<String, DomainError> {
    if value.is_empty() {
        Err(DomainError::EmptyId)
    } else {
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticReference {
    pub schema_version: String,
    pub source_feature_id: FeatureId,
    pub source_output_role: String,
    pub expected_feature_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureDescriptor {
    pub id: FeatureId,
    pub kind: String,
    pub parameters: BTreeMap<String, Value>,
    #[serde(default)]
    pub references: Vec<SemanticReference>,
}

impl FeatureDescriptor {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("feature descriptor serializes")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    pub translation_micrometers: [i64; 3],
    pub rotation_degrees: [i64; 3],
}

impl Transform {
    pub const fn identity() -> Self {
        Self {
            translation_micrometers: [0, 0, 0],
            rotation_degrees: [0, 0, 0],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDefinition {
    pub id: ComponentDefinitionId,
    pub features: Vec<FeatureDescriptor>,
}

impl ComponentDefinition {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("component definition serializes")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentInstance {
    pub id: ComponentInstanceId,
    pub definition_id: ComponentDefinitionId,
    pub transform: Transform,
}

impl ComponentInstance {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("component instance serializes")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentGraph {
    pub definitions: Vec<ComponentDefinition>,
    pub instances: Vec<ComponentInstance>,
}

impl ComponentGraph {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("component graph serializes")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReattachmentOutcome {
    Resolved,
    Ambiguous,
    Lost,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CommandIntent {
    DefineComponent {
        definition_id: ComponentDefinitionId,
        features: Vec<FeatureDescriptor>,
    },
    PlaceInstance {
        instance_id: ComponentInstanceId,
        definition_id: ComponentDefinitionId,
        transform: Transform,
    },
    TransformInstance {
        instance_id: ComponentInstanceId,
        transform: Transform,
    },
    IndependentCopy {
        source_instance_id: ComponentInstanceId,
        copy_suffix: String,
    },
    EditParameter {
        definition_id: ComponentDefinitionId,
        feature_id: FeatureId,
        parameter_name: String,
        parameter_value: Value,
    },
}

impl CommandIntent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::DefineComponent { .. } => "define-component",
            Self::PlaceInstance { .. } => "place-instance",
            Self::TransformInstance { .. } => "transform-instance",
            Self::IndependentCopy { .. } => "independent-copy",
            Self::EditParameter { .. } => "edit-parameter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandTransaction {
    pub schema_version: String,
    pub sequence: usize,
    pub parent_revision_id: String,
    pub revision_id: String,
    pub intent: CommandIntent,
    pub reattachment: ReattachmentOutcome,
    pub affected_ids: Vec<String>,
}

impl CommandTransaction {
    pub fn canonical_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("command transaction serializes");
        line.push('\n');
        line
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub id: String,
    pub features: Vec<FeatureId>,
    #[serde(default)]
    pub component_graph: ComponentGraph,
}

impl Revision {
    pub fn empty() -> Self {
        Self {
            id: "revision-0".to_string(),
            features: Vec::new(),
            component_graph: ComponentGraph::default(),
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

    pub fn current_revision(&self) -> &Revision {
        self.revisions.last().expect("generation has a revision")
    }

    pub fn apply(&mut self, intent: CommandIntent) -> Result<CommandTransaction, DomainError> {
        let sequence = self.revisions.len();
        let parent = self.current_revision();
        let (component_graph, reattachment, affected_ids) =
            apply_intent(&parent.component_graph, &intent)?;
        let transaction = CommandTransaction {
            schema_version: "threeterm.transaction.component/1".to_string(),
            sequence,
            parent_revision_id: parent.id.clone(),
            revision_id: format!("revision-{sequence}"),
            intent,
            reattachment,
            affected_ids,
        };
        self.revisions.push(Revision {
            id: transaction.revision_id.clone(),
            features: parent.features.clone(),
            component_graph,
        });
        Ok(transaction)
    }

    pub fn replay(&mut self, transaction: &CommandTransaction) -> Result<(), DomainError> {
        let expected_sequence = self.revisions.len();
        let parent = self.current_revision();
        if transaction.schema_version != "threeterm.transaction.component/1"
            || transaction.sequence != expected_sequence
            || transaction.parent_revision_id != parent.id
            || transaction.revision_id != format!("revision-{expected_sequence}")
        {
            return Err(DomainError::InvalidTransaction(
                "transaction revision lineage is incompatible".to_string(),
            ));
        }
        let (component_graph, reattachment, affected_ids) =
            apply_intent(&parent.component_graph, &transaction.intent)?;
        if transaction.reattachment != reattachment || transaction.affected_ids != affected_ids {
            return Err(DomainError::InvalidTransaction(
                "transaction semantic outcome does not match replay".to_string(),
            ));
        }
        self.revisions.push(Revision {
            id: transaction.revision_id.clone(),
            features: parent.features.clone(),
            component_graph,
        });
        Ok(())
    }
}

fn apply_intent(
    graph: &ComponentGraph,
    intent: &CommandIntent,
) -> Result<(ComponentGraph, ReattachmentOutcome, Vec<String>), DomainError> {
    let mut next = graph.clone();
    match intent {
        CommandIntent::DefineComponent {
            definition_id,
            features,
        } => {
            if features.is_empty() {
                return Err(DomainError::InvalidCommand(
                    "component definition requires at least one feature".to_string(),
                ));
            }
            if next
                .definitions
                .iter()
                .any(|definition| definition.id == *definition_id)
            {
                return Err(DomainError::ReferenceAmbiguous(definition_id.0.clone()));
            }
            let unique_ids: BTreeSet<&FeatureId> =
                features.iter().map(|feature| &feature.id).collect();
            if unique_ids.len() != features.len() {
                return Err(DomainError::ReferenceAmbiguous(
                    "component feature IDs are not unique".to_string(),
                ));
            }
            for feature in features {
                for reference in &feature.references {
                    match resolve_reference(reference, features) {
                        ReattachmentOutcome::Resolved => {}
                        ReattachmentOutcome::Ambiguous => {
                            return Err(DomainError::ReferenceAmbiguous(
                                reference.source_feature_id.0.clone(),
                            ));
                        }
                        ReattachmentOutcome::Lost => {
                            return Err(DomainError::ReferenceLost(
                                reference.source_feature_id.0.clone(),
                            ));
                        }
                        ReattachmentOutcome::Incompatible => {
                            return Err(DomainError::ReferenceIncompatible(
                                reference.source_feature_id.0.clone(),
                            ));
                        }
                    }
                }
            }
            next.definitions.push(ComponentDefinition {
                id: definition_id.clone(),
                features: features.clone(),
            });
            Ok((
                next,
                ReattachmentOutcome::Resolved,
                vec![definition_id.0.clone()],
            ))
        }
        CommandIntent::PlaceInstance {
            instance_id,
            definition_id,
            transform,
        } => {
            if next
                .instances
                .iter()
                .any(|instance| instance.id == *instance_id)
            {
                return Err(DomainError::ReferenceAmbiguous(instance_id.0.clone()));
            }
            let definitions: Vec<&ComponentDefinition> = next
                .definitions
                .iter()
                .filter(|definition| definition.id == *definition_id)
                .collect();
            match definitions.len() {
                0 => return Err(DomainError::ReferenceLost(definition_id.0.clone())),
                1 => {}
                _ => return Err(DomainError::ReferenceAmbiguous(definition_id.0.clone())),
            }
            next.instances.push(ComponentInstance {
                id: instance_id.clone(),
                definition_id: definition_id.clone(),
                transform: transform.clone(),
            });
            Ok((
                next,
                ReattachmentOutcome::Resolved,
                vec![instance_id.0.clone(), definition_id.0.clone()],
            ))
        }
        CommandIntent::TransformInstance {
            instance_id,
            transform,
        } => {
            let position = next
                .instances
                .iter()
                .position(|instance| instance.id == *instance_id)
                .ok_or_else(|| DomainError::ReferenceLost(instance_id.0.clone()))?;
            if next
                .instances
                .iter()
                .filter(|instance| instance.id == *instance_id)
                .count()
                > 1
            {
                return Err(DomainError::ReferenceAmbiguous(instance_id.0.clone()));
            }
            next.instances[position].transform = transform.clone();
            Ok((
                next,
                ReattachmentOutcome::Resolved,
                vec![instance_id.0.clone()],
            ))
        }
        CommandIntent::IndependentCopy {
            source_instance_id,
            copy_suffix,
        } => {
            if copy_suffix.is_empty() {
                return Err(DomainError::InvalidCommand(
                    "copy suffix must not be empty".to_string(),
                ));
            }
            let source_instance = next
                .instances
                .iter()
                .find(|instance| instance.id == *source_instance_id)
                .cloned()
                .ok_or_else(|| DomainError::ReferenceLost(source_instance_id.0.clone()))?;
            if next
                .instances
                .iter()
                .filter(|instance| instance.id == *source_instance_id)
                .count()
                > 1
            {
                return Err(DomainError::ReferenceAmbiguous(
                    source_instance_id.0.clone(),
                ));
            }
            let source_definition = next
                .definitions
                .iter()
                .find(|definition| definition.id == source_instance.definition_id)
                .cloned()
                .ok_or_else(|| {
                    DomainError::ReferenceLost(source_instance.definition_id.0.clone())
                })?;
            if next
                .definitions
                .iter()
                .filter(|definition| definition.id == source_instance.definition_id)
                .count()
                > 1
            {
                return Err(DomainError::ReferenceAmbiguous(
                    source_instance.definition_id.0.clone(),
                ));
            }
            if source_definition.features.is_empty() {
                return Err(DomainError::InvalidCommand(
                    "component definition must contain at least one feature to copy".to_string(),
                ));
            }
            if source_definition.features.len() > 1 {
                return Err(DomainError::InvalidCommand(
                    "multi-feature independent copy is not supported in this slice".to_string(),
                ));
            }
            let source_feature = source_definition.features[0].clone();
            let new_definition_id = ComponentDefinitionId::new(format!(
                "{}-{copy_suffix}",
                source_definition.id.as_str()
            ))?;
            let new_feature_id =
                FeatureId::new(format!("{}-{copy_suffix}", source_feature.id.as_str()))?;
            let new_instance_id =
                ComponentInstanceId::new(format!("{}-{copy_suffix}", source_instance.id.as_str()))?;
            if new_definition_id == source_definition.id
                || new_feature_id == source_feature.id
                || new_instance_id == source_instance.id
            {
                return Err(DomainError::InvalidCommand(
                    "copy suffix must change at least one of the generated IDs".to_string(),
                ));
            }
            if next
                .definitions
                .iter()
                .any(|definition| definition.id == new_definition_id)
                || next
                    .instances
                    .iter()
                    .any(|instance| instance.id == new_instance_id)
                || next.definitions.iter().any(|definition| {
                    definition
                        .features
                        .iter()
                        .any(|feature| feature.id == new_feature_id)
                })
            {
                return Err(DomainError::ReferenceAmbiguous(copy_suffix.clone()));
            }
            let remapped_feature = FeatureDescriptor {
                id: new_feature_id.clone(),
                kind: source_feature.kind,
                parameters: source_feature.parameters,
                references: Vec::new(),
            };
            let new_definition = ComponentDefinition {
                id: new_definition_id.clone(),
                features: vec![remapped_feature],
            };
            let new_instance = ComponentInstance {
                id: new_instance_id.clone(),
                definition_id: new_definition_id.clone(),
                transform: source_instance.transform,
            };
            next.definitions.push(new_definition);
            next.instances.push(new_instance);
            Ok((
                next,
                ReattachmentOutcome::Resolved,
                vec![
                    new_definition_id.0.clone(),
                    new_feature_id.0.clone(),
                    new_instance_id.0.clone(),
                ],
            ))
        }
        CommandIntent::EditParameter {
            definition_id,
            feature_id,
            parameter_name,
            parameter_value,
        } => {
            if parameter_name.is_empty() {
                return Err(DomainError::InvalidCommand(
                    "parameter name must not be empty".to_string(),
                ));
            }
            let definition_position = next
                .definitions
                .iter()
                .position(|definition| definition.id == *definition_id)
                .ok_or_else(|| DomainError::ReferenceLost(definition_id.0.clone()))?;
            if next
                .definitions
                .iter()
                .filter(|definition| definition.id == *definition_id)
                .count()
                > 1
            {
                return Err(DomainError::ReferenceAmbiguous(definition_id.0.clone()));
            }
            let feature_position = next.definitions[definition_position]
                .features
                .iter()
                .position(|feature| feature.id == *feature_id);
            let feature_position = match feature_position {
                Some(position) => position,
                None => return Err(DomainError::ReferenceLost(feature_id.0.clone())),
            };
            if next.definitions[definition_position]
                .features
                .iter()
                .filter(|feature| feature.id == *feature_id)
                .count()
                > 1
            {
                return Err(DomainError::ReferenceAmbiguous(feature_id.0.clone()));
            }
            next.definitions[definition_position].features[feature_position]
                .parameters
                .insert(parameter_name.clone(), parameter_value.clone());
            Ok((
                next,
                ReattachmentOutcome::Resolved,
                vec![definition_id.0.clone(), feature_id.0.clone()],
            ))
        }
    }
}

pub fn resolve_reference(
    reference: &SemanticReference,
    candidates: &[FeatureDescriptor],
) -> ReattachmentOutcome {
    if reference.schema_version != "threeterm.reference.semantic/1" {
        return ReattachmentOutcome::Incompatible;
    }
    let matches: Vec<&FeatureDescriptor> = candidates
        .iter()
        .filter(|candidate| candidate.id == reference.source_feature_id)
        .collect();
    match matches.as_slice() {
        [] => ReattachmentOutcome::Lost,
        [_, _, ..] => ReattachmentOutcome::Ambiguous,
        [candidate] if candidate.kind == reference.expected_feature_kind => {
            ReattachmentOutcome::Resolved
        }
        [_] => ReattachmentOutcome::Incompatible,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    EmptyId,
    InvalidCommand(String),
    InvalidTransaction(String),
    ReferenceAmbiguous(String),
    ReferenceLost(String),
    ReferenceIncompatible(String),
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => formatter.write_str("feature id must not be empty"),
            Self::InvalidCommand(detail) => write!(formatter, "invalid command: {detail}"),
            Self::InvalidTransaction(detail) => write!(formatter, "invalid transaction: {detail}"),
            Self::ReferenceAmbiguous(detail) => write!(formatter, "reference ambiguous: {detail}"),
            Self::ReferenceLost(detail) => write!(formatter, "reference lost: {detail}"),
            Self::ReferenceIncompatible(detail) => {
                write!(formatter, "reference incompatible: {detail}")
            }
        }
    }
}

impl std::error::Error for DomainError {}

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
    fn feature_id_rejects_empty_values() {
        assert_eq!(FeatureId::new(""), Err(DomainError::EmptyId));
    }

    #[test]
    fn resolve_reference_outcomes_match_the_four_state_machine() {
        let candidate = FeatureDescriptor {
            id: FeatureId::new("feature-target").unwrap(),
            kind: "l-bracket".to_string(),
            parameters: BTreeMap::new(),
            references: Vec::new(),
        };
        let candidates = vec![candidate.clone(), candidate.clone()];

        let resolved = SemanticReference {
            schema_version: "threeterm.reference.semantic/1".to_string(),
            source_feature_id: FeatureId::new("feature-target").unwrap(),
            source_output_role: "face".to_string(),
            expected_feature_kind: "l-bracket".to_string(),
        };
        assert_eq!(
            resolve_reference(&resolved, std::slice::from_ref(&candidate)),
            ReattachmentOutcome::Resolved
        );

        let lost = SemanticReference {
            schema_version: "threeterm.reference.semantic/1".to_string(),
            source_feature_id: FeatureId::new("feature-missing").unwrap(),
            source_output_role: "face".to_string(),
            expected_feature_kind: "l-bracket".to_string(),
        };
        assert_eq!(
            resolve_reference(&lost, std::slice::from_ref(&candidate)),
            ReattachmentOutcome::Lost
        );

        let incompatible_schema = SemanticReference {
            schema_version: "threeterm.reference.semantic/0".to_string(),
            source_feature_id: FeatureId::new("feature-target").unwrap(),
            source_output_role: "face".to_string(),
            expected_feature_kind: "l-bracket".to_string(),
        };
        assert_eq!(
            resolve_reference(&incompatible_schema, std::slice::from_ref(&candidate)),
            ReattachmentOutcome::Incompatible
        );

        let incompatible_kind = SemanticReference {
            schema_version: "threeterm.reference.semantic/1".to_string(),
            source_feature_id: FeatureId::new("feature-target").unwrap(),
            source_output_role: "face".to_string(),
            expected_feature_kind: "extrude".to_string(),
        };
        assert_eq!(
            resolve_reference(&incompatible_kind, std::slice::from_ref(&candidate)),
            ReattachmentOutcome::Incompatible
        );

        let ambiguous = SemanticReference {
            schema_version: "threeterm.reference.semantic/1".to_string(),
            source_feature_id: FeatureId::new("feature-target").unwrap(),
            source_output_role: "face".to_string(),
            expected_feature_kind: "l-bracket".to_string(),
        };
        assert_eq!(
            resolve_reference(&ambiguous, &candidates),
            ReattachmentOutcome::Ambiguous
        );
    }
}

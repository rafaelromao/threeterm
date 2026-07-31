use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod graph {
    pub use super::{Feature, FeatureGraph, FeatureId, ProjectGeneration, Revision};
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
}

impl FeatureGraph {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn add_feature(&mut self, feature: Feature) -> bool {
        let previous = self.features.insert(feature.id, feature.kind.clone());
        previous.as_deref() != Some(feature.kind.as_str())
    }

    pub fn graph_hash_hex(&self) -> String {
        hash_hex(&serde_json::to_vec(&self.features).expect("feature graph serializes"))
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
}

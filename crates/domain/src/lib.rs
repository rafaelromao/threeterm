//! Canonical ThreeTerm feature graph and domain model.
//!
//! The slice (#235) extends the existing domain surface from #234 with
//! the deterministic `graph_hash_hex` of the canonical JSON encoding
//! and a `revision_hex` that combines the graph hash with the
//! transactional log's terminal digest.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub mod feature_graph;

pub use feature_graph::{EMPTY_LOG_DIGEST_HEX, revision_hex};

pub fn schema_version() -> &'static str {
    "threeterm.domain/1"
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => formatter.write_str("feature id must not be empty"),
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
}

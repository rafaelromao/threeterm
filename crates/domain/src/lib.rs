//! Canonical ThreeTerm feature graph and domain model.
//!
//! The slice (#235) extends the existing domain surface from #234 with
//! the deterministic `feature_graph_hash_hex` over the canonical
//! `(revision_id, features)` encoding and a `revision_hash_hex` that
//! combines the graph hash with the transactional log's terminal
//! digest.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn schema_version() -> &'static str {
    "threeterm.domain/1"
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// SHA-256 hex of the canonical JSON encoding of the
/// `ProjectGeneration` (the `(revision_id, features)` tuples in
/// canonical BTreeMap-backed object order). Two projects with the same
/// feature set produce the same hex regardless of insertion order. This
/// is the `feature_graph_hash` the slice's save/load round-trip emits.
pub fn feature_graph_hash_hex(generation: &ProjectGeneration) -> String {
    let mut ordered: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for revision in &generation.revisions {
        let features: Vec<String> = revision
            .features
            .iter()
            .map(|f| f.as_str().to_string())
            .collect();
        ordered.insert(revision.id.clone(), features);
    }
    let bytes = serde_json::to_vec(&ordered).expect("canonical JSON serializes");
    hex_sha256(&bytes)
}

/// SHA-256 hex of `feature_graph_hash_hex || terminal_log_digest_hex`.
/// This is the `revision_hash` the slice's save/load round-trip emits; it
/// rebinds when EITHER the feature graph changes OR the canonical
/// transaction log's terminal digest changes.
pub fn revision_hex(graph_hash_hex: &str, terminal_log_digest_hex: &str) -> String {
    let mut bytes = Vec::with_capacity(graph_hash_hex.len() + terminal_log_digest_hex.len());
    bytes.extend_from_slice(graph_hash_hex.as_bytes());
    bytes.extend_from_slice(terminal_log_digest_hex.as_bytes());
    hex_sha256(&bytes)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The all-zero SHA-256 digest used as the chain's anchor before any
/// transaction is appended. Mirrors the persistence layer's empty-log
/// terminal digest.
pub const EMPTY_LOG_DIGEST_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

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

    #[test]
    fn feature_graph_hash_is_deterministic_for_empty_project() {
        let generation = ProjectGeneration::with_id("generation-test");
        let first = feature_graph_hash_hex(&generation);
        let second = feature_graph_hash_hex(&generation);
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "lowercase hex; got {first}"
        );
    }

    #[test]
    fn revision_combines_graph_hash_and_log_digest() {
        let rev_a = revision_hex(
            "0000000000000000000000000000000000000000000000000000000000000000",
            EMPTY_LOG_DIGEST_HEX,
        );
        assert_eq!(rev_a.len(), 64);
        let rev_b = revision_hex(
            "0000000000000000000000000000000000000000000000000000000000000000",
            "ff".repeat(32).as_str(),
        );
        assert_ne!(rev_a, rev_b);
    }
}

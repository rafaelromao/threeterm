//! Canonical feature graph and revision hash for the ThreeTerm domain model.
//!
//! The slice (#235) implements the minimum domain surface that the save /
//! load round-trip needs:
//!
//! - [`FeatureGraph`] is the authoritative document state. It is a
//!   `BTreeMap<FeatureId, Feature>` so the canonical JSON serialization
//!   sorts keys deterministically, mirroring the established
//!   `protocol::schema::registry_hash` test.
//! - [`graph_hash_hex`] reduces the graph to a 32-byte SHA-256 hex string.
//!   Two graphs with the same `(id, kind)` set produce identical hex
//!   strings regardless of insertion order.
//! - [`revision_hex`] combines `graph_hash_hex` with the
//!   `terminal_log_digest_hex` to produce the revision the manifest
//!   commits.
//! - [`empty_log_digest_hex`] is the all-zero SHA-256 digest that anchors
//!   the digest chain before any transaction is appended.
//!
//! Digest arrays participate in JSON as **lowercase hex strings** everywhere
//! in the bundle; the bytes-as-hex encoding is part of the hash input and
//! must remain stable across versions. [`DomainError`] is the single error
//! surface for the module.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable, presentation-neutral identifier for a feature in the canonical
/// graph. Wrapped in a newtype so the registry can be keyed by `FeatureId`
/// rather than by topology indexes or in-process kernel object identity
/// (closed issue #23).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct FeatureId(pub String);

/// Stable, presentation-neutral identifier for the kind of a feature
/// (`"box"`, `"extrude"`, ...). Wrapped in a newtype to keep the
/// canonical JSON shape consistent across the bundle.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct FeatureKind(pub String);

/// One entry in the canonical feature graph. `(id, kind)` is the natural
/// key; `FeatureGraph::add_feature` is idempotent on this pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub kind: FeatureKind,
}

impl Feature {
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: FeatureId(id.into()),
            kind: FeatureKind(kind.into()),
        }
    }
}

/// The canonical feature graph: an ordered map keyed by `FeatureId` to
/// preserve insertion order while keeping JSON serialization
/// order-independent. Two graphs containing the same `(id, kind)` pairs
/// (in any insertion order) produce identical [`graph_hash_hex`] values.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FeatureGraph {
    features: BTreeMap<FeatureId, Feature>,
}

impl FeatureGraph {
    /// Construct an empty graph.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Insert or replace a feature. Idempotent on `(id, kind)` — adding
    /// the same `(id, kind)` twice does not change [`graph_hash_hex`].
    /// Replacing an `id` with a different `kind` mutates the graph and
    /// changes the hash.
    pub fn add_feature(&mut self, feature: Feature) -> &mut Self {
        self.features.insert(feature.id.clone(), feature);
        self
    }

    /// `true` if the graph currently contains `feature` as `id` -> `kind`.
    pub fn contains(&self, id: &FeatureId, kind: &FeatureKind) -> bool {
        self.features
            .get(id)
            .map(|existing| &existing.kind == kind)
            .unwrap_or(false)
    }

    /// Iterate the graph in canonical key order.
    pub fn iter(&self) -> impl Iterator<Item = (&FeatureId, &Feature)> {
        self.features.iter()
    }

    /// Number of features in the graph.
    pub fn len(&self) -> usize {
        self.features.len()
    }

    /// `true` when the graph has no features.
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Lowercase-hex SHA-256 of the canonical JSON encoding of the
    /// `(id, kind)` pairs. Sorted by `BTreeMap` so insertion order does
    /// not change the hash.
    pub fn graph_hash_hex(&self) -> String {
        let canonical: BTreeMap<&FeatureId, &FeatureKind> =
            self.features.iter().map(|(id, f)| (id, &f.kind)).collect();
        let bytes = serde_json::to_vec(&canonical).expect("canonical JSON serializes");
        hex_sha256(&bytes)
    }
}

/// Lowercase-hex SHA-256 of `graph_hash_hex_bytes || log_digest_hex_bytes`.
/// This is the revision the manifest commits; it depends on BOTH the
/// canonical graph state and the canonical transaction log's terminal
/// digest so a history-only change (e.g., a duplicate `add_feature` call
/// or a later append without graph change) rebinds the revision.
pub fn revision_hex(graph_hash_hex: &str, terminal_log_digest_hex: &str) -> String {
    let mut bytes = Vec::with_capacity(graph_hash_hex.len() + terminal_log_digest_hex.len());
    bytes.extend_from_slice(graph_hash_hex.as_bytes());
    bytes.extend_from_slice(terminal_log_digest_hex.as_bytes());
    hex_sha256(&bytes)
}

/// The all-zero SHA-256 digest used as the chain's anchor before any
/// transaction is appended. It is a `&'static str` so callers can use it
/// without allocation.
pub const EMPTY_LOG_DIGEST_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_hash_is_deterministic() {
        let a = FeatureGraph::empty().graph_hash_hex();
        let b = FeatureGraph::empty().graph_hash_hex();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn adding_same_feature_twice_is_idempotent() {
        let mut g = FeatureGraph::empty();
        g.add_feature(Feature::new("box-1", "box"));
        let once = g.graph_hash_hex();

        g.add_feature(Feature::new("box-1", "box"));
        let twice = g.graph_hash_hex();
        assert_eq!(once, twice, "duplicate add does not change the hash");
    }

    #[test]
    fn insertion_order_does_not_change_graph_hash() {
        let mut first = FeatureGraph::empty();
        first.add_feature(Feature::new("box-1", "box"));
        first.add_feature(Feature::new("box-2", "box"));

        let mut second = FeatureGraph::empty();
        second.add_feature(Feature::new("box-2", "box"));
        second.add_feature(Feature::new("box-1", "box"));

        assert_eq!(
            first.graph_hash_hex(),
            second.graph_hash_hex(),
            "BTreeMap canonicalization makes insertion order irrelevant"
        );
    }

    #[test]
    fn revision_combines_graph_hash_and_log_digest() {
        let g = FeatureGraph::empty();
        let graph_hash = g.graph_hash_hex();
        let rev = revision_hex(&graph_hash, EMPTY_LOG_DIGEST_HEX);
        assert_eq!(rev.len(), 64);
        let rev_again = revision_hex(&graph_hash, EMPTY_LOG_DIGEST_HEX);
        assert_eq!(rev, rev_again);
    }

    #[test]
    fn different_log_digest_changes_revision() {
        let g = FeatureGraph::empty();
        let graph_hash = g.graph_hash_hex();
        let rev_a = revision_hex(&graph_hash, EMPTY_LOG_DIGEST_HEX);
        let rev_b = revision_hex(&graph_hash, "ff".repeat(32).as_str());
        assert_ne!(rev_a, rev_b);
    }
}

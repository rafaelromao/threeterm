//! Canonical ThreeTerm feature graph and domain model.
//!
//! The `#235` slice exposes the minimum surface the save / load round-trip
//! needs: a deterministic `graph_hash_hex` of the canonical JSON encoding
//! of the feature set, and a `revision_hex` that combines the graph hash
//! with the transactional log's terminal digest.

pub mod graph;

pub use graph::{
    EMPTY_LOG_DIGEST_HEX, Feature, FeatureGraph, FeatureId, FeatureKind,
    revision_hex,
};

pub fn schema_version() -> &'static str {
    "threeterm.domain/1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.domain/1");
    }

    #[test]
    fn pinned_empty_graph_hash_is_stable() {
        // Pinned constant: the SHA-256 of the canonical JSON `{}`.
        let expected = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";
        let actual = graph::FeatureGraph::empty().graph_hash_hex();
        assert_eq!(actual, expected, "empty-graph hash is pinned");
    }
}

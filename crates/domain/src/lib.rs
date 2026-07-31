use std::fmt;

use serde::{Deserialize, Serialize};

pub mod graph {
    pub use super::{CommandIntent, CommandTransaction, FeatureId, ProjectGeneration, Revision};
}

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
    /// Build a `ProjectGeneration` whose `id` is the canonical log digest
    /// of the empty initial state.
    ///
    /// The digest is a fixed SHA-256 of the empty NDJSON transaction log,
    /// so two consecutive calls produce byte-equal IDs. The persistence
    /// layer (`threeterm-persistence::bundle::log_identity_hex`) is the
    /// single source of truth for the encoding; the constant is pinned
    /// here so the domain type can stand alone.
    pub fn fresh() -> Self {
        Self::with_id(EMPTY_LOG_IDENTITY)
    }

    pub fn with_id(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            revisions: vec![Revision::empty()],
        }
    }
}

/// Canonical log digest of the empty initial transaction log.
///
/// SHA-256 of the empty byte sequence:
/// `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.
/// The persistence layer computes the same value via
/// `compute_log_identity(b"")`; this constant is pinned here so the
/// domain type can produce a fresh `ProjectGeneration` without depending
/// on a hashing crate.
pub const EMPTY_LOG_IDENTITY: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The MVP command surface for accepted command transactions.
///
/// The slice ships the foundation set: `add-feature`, `set-parameter`,
/// `remove-feature`. The CAD-specific operations (sketch, extrude, hole,
/// fillet, ...) live in later slices and piggyback on the same
/// identity-bearing transaction record.
///
/// Each intent serializes to one canonical NDJSON line so the canonical
/// transaction log remains a stable byte sequence for hashing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CommandIntent {
    AddFeature {
        feature_id: FeatureId,
        feature_kind: String,
        parameters: serde_json::Value,
    },
    SetParameter {
        feature_id: FeatureId,
        parameter: String,
        value: serde_json::Value,
    },
    RemoveFeature {
        feature_id: FeatureId,
    },
}

impl CommandIntent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AddFeature { .. } => "add-feature",
            Self::SetParameter { .. } => "set-parameter",
            Self::RemoveFeature { .. } => "remove-feature",
        }
    }

    /// Canonical NDJSON line for the intent.
    ///
    /// Object keys are sorted by `serde_json::Value::Object`'s default
    /// `BTreeMap` backing so two equivalent intents serialize to the same
    /// bytes. The line ends with a `\n` terminator so it is valid NDJSON.
    pub fn canonical_line(&self) -> String {
        let value = serde_json::to_value(self).expect("intent serializes");
        let mut bytes = serde_json::to_vec(&value).expect("intent serializes");
        bytes.push(b'\n');
        String::from_utf8(bytes).expect("canonical line is utf-8")
    }
}

/// One accepted command transaction, sealed as a UTF-8 NDJSON line.
///
/// The persistence layer appends `canonical_line()` to
/// `canonical/transactions.ndjson` and recomputes the canonical log
/// identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandTransaction {
    pub intent: CommandIntent,
}

impl CommandTransaction {
    pub fn new(intent: CommandIntent) -> Self {
        Self { intent }
    }

    pub fn canonical_line(&self) -> String {
        self.intent.canonical_line()
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
    fn fresh_generation_id_is_deterministic() {
        let first = ProjectGeneration::fresh();
        let second = ProjectGeneration::fresh();
        assert_eq!(
            first.id, second.id,
            "fresh ProjectGeneration must produce byte-equal ids across calls"
        );
        assert_eq!(
            first.id, EMPTY_LOG_IDENTITY,
            "fresh ProjectGeneration id must be the canonical empty-log digest"
        );
    }

    #[test]
    fn feature_id_rejects_empty_values() {
        assert_eq!(FeatureId::new(""), Err(DomainError::EmptyId));
    }

    #[test]
    fn command_intent_names_match_the_mvp_operation_set() {
        let add = CommandIntent::AddFeature {
            feature_id: FeatureId::new("sketch-1").unwrap(),
            feature_kind: "sketch".to_string(),
            parameters: serde_json::json!({}),
        };
        let set = CommandIntent::SetParameter {
            feature_id: FeatureId::new("sketch-1").unwrap(),
            parameter: "width".to_string(),
            value: serde_json::json!(1.0),
        };
        let remove = CommandIntent::RemoveFeature {
            feature_id: FeatureId::new("sketch-1").unwrap(),
        };
        assert_eq!(add.kind(), "add-feature");
        assert_eq!(set.kind(), "set-parameter");
        assert_eq!(remove.kind(), "remove-feature");
    }

    #[test]
    fn command_intent_canonical_line_ends_with_newline_and_is_deterministic() {
        let intent = CommandIntent::AddFeature {
            feature_id: FeatureId::new("sketch-1").unwrap(),
            feature_kind: "sketch".to_string(),
            parameters: serde_json::json!({"plane": "xy"}),
        };
        let first = intent.canonical_line();
        let second = intent.canonical_line();
        assert_eq!(first, second);
        assert!(first.ends_with('\n'));
        assert_eq!(first.matches('\n').count(), 1);
    }

    #[test]
    fn command_transaction_canonical_line_matches_intent_line() {
        let intent = CommandIntent::SetParameter {
            feature_id: FeatureId::new("sketch-1").unwrap(),
            parameter: "width".to_string(),
            value: serde_json::json!(2.5),
        };
        let transaction = CommandTransaction::new(intent.clone());
        assert_eq!(transaction.canonical_line(), intent.canonical_line());
    }
}

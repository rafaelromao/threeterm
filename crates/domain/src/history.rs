use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const HISTORY_EVENT_SCHEMA: &str = "threeterm.history.event/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryStatus {
    CurrentValid,
    Broken,
    BlockedByFailure,
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryDiagnostic {
    pub code: String,
    pub feature_id: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryFeature {
    pub id: String,
    pub dependencies: Vec<String>,
    pub input_value: f64,
    pub geometry_fingerprint: Option<String>,
    pub last_valid_geometry_fingerprint: Option<String>,
    pub status: HistoryStatus,
    pub diagnostic: Option<HistoryDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistorySnapshot {
    pub revision_id: String,
    pub features: BTreeMap<String, HistoryFeature>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedRevision {
    pub name: String,
    pub snapshot: HistorySnapshot,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryState {
    pub active: HistorySnapshot,
    pub named_revisions: BTreeMap<String, NamedRevision>,
    pub event_ordinal: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEvent {
    pub schema_version: String,
    pub ordinal: u64,
    pub operation: HistoryOperation,
    pub active: HistorySnapshot,
    pub named_revisions: BTreeMap<String, NamedRevision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
pub enum HistoryOperation {
    InitializeLBracket {
        bracket_id: String,
        length: f64,
        width: f64,
        height: f64,
        thickness: f64,
    },
    HistoricalEdit {
        feature_id: String,
        parameter: String,
        value: f64,
        dirty_features: Vec<String>,
        evaluated_features: Vec<String>,
        blocked_features: Vec<String>,
        diagnostics: Vec<HistoryDiagnostic>,
        preserved_name: Option<String>,
    },
    CreateNamedRevision {
        name: String,
    },
    RestoreNamedRevision {
        name: String,
        displaced_name: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    InvalidEvent(String),
    FeatureNotFound(String),
    DuplicateName(String),
    EmptyName,
    NamedRevisionNotFound(String),
    InvalidValue,
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEvent(detail) => write!(formatter, "invalid history event: {detail}"),
            Self::FeatureNotFound(id) => write!(formatter, "history feature not found: {id}"),
            Self::DuplicateName(name) => write!(formatter, "named revision already exists: {name}"),
            Self::EmptyName => formatter.write_str("named revision name must not be empty"),
            Self::NamedRevisionNotFound(name) => {
                write!(formatter, "named revision not found: {name}")
            }
            Self::InvalidValue => formatter.write_str("historical edit value must be finite"),
        }
    }
}

impl std::error::Error for HistoryError {}

impl Default for HistoryState {
    fn default() -> Self {
        Self {
            active: HistorySnapshot {
                revision_id: "history-revision-0".to_string(),
                features: BTreeMap::new(),
            },
            named_revisions: BTreeMap::new(),
            event_ordinal: 0,
        }
    }
}

impl HistoryState {
    pub fn apply_event(&mut self, event: &HistoryEvent) -> Result<(), HistoryError> {
        if event.schema_version != HISTORY_EVENT_SCHEMA {
            return Err(HistoryError::InvalidEvent(format!(
                "unsupported schema {}",
                event.schema_version
            )));
        }
        if event.ordinal != self.event_ordinal + 1 {
            return Err(HistoryError::InvalidEvent(format!(
                "expected ordinal {}, got {}",
                self.event_ordinal + 1,
                event.ordinal
            )));
        }
        validate_snapshot(&event.active)?;
        validate_named_revisions(&event.named_revisions)?;
        self.active = event.active.clone();
        self.named_revisions = event.named_revisions.clone();
        self.event_ordinal = event.ordinal;
        Ok(())
    }

    pub fn initialize_l_bracket(
        &self,
        bracket_id: &str,
        length: f64,
        width: f64,
        height: f64,
        thickness: f64,
    ) -> Result<HistoryEvent, HistoryError> {
        if bracket_id.is_empty()
            || ![length, width, height, thickness]
                .iter()
                .all(|value| value.is_finite() && *value > 0.0)
        {
            return Err(HistoryError::InvalidValue);
        }
        let bracket = l_bracket_snapshot(
            bracket_id,
            length,
            width,
            height,
            thickness,
            self.event_ordinal + 1,
        );
        let mut active = self.active.clone();
        active.revision_id = format!("history-revision-{}", self.event_ordinal + 1);
        active.features.extend(bracket.features);
        Ok(self.event(
            HistoryOperation::InitializeLBracket {
                bracket_id: bracket_id.to_string(),
                length,
                width,
                height,
                thickness,
            },
            active,
            self.named_revisions.clone(),
        ))
    }

    pub fn historical_edit(
        &self,
        feature_id: &str,
        parameter: &str,
        value: f64,
    ) -> Result<(HistoryEvent, HistoryEvaluation), HistoryError> {
        if !value.is_finite() {
            return Err(HistoryError::InvalidValue);
        }
        self.active
            .features
            .get(feature_id)
            .ok_or_else(|| HistoryError::FeatureNotFound(feature_id.to_string()))?;
        let ordinal = self.event_ordinal + 1;
        let preserved_name = format!("recovered-before-historical-edit-{ordinal}");
        let mut named_revisions = self.named_revisions.clone();
        named_revisions.insert(
            preserved_name.clone(),
            NamedRevision {
                name: preserved_name.clone(),
                snapshot: self.active.clone(),
                provenance: format!("historical-edit:{feature_id}"),
            },
        );

        let dirty_features = reachable_features(&self.active.features, feature_id)?;
        let order = topological_order(&self.active.features, &dirty_features)?;
        let mut active = self.active.clone();
        active.revision_id = format!("history-revision-{ordinal}");
        let mut evaluated_features = Vec::new();
        let mut blocked_features = Vec::new();
        let mut diagnostics = Vec::new();
        let mut failed = false;
        for id in order {
            let dependencies = active
                .features
                .get(&id)
                .ok_or_else(|| HistoryError::FeatureNotFound(id.clone()))?
                .dependencies
                .clone();
            if failed
                || dependencies.iter().any(|dependency| {
                    active
                        .features
                        .get(dependency)
                        .is_some_and(|item| item.status != HistoryStatus::CurrentValid)
                })
            {
                let feature = active
                    .features
                    .get_mut(&id)
                    .ok_or_else(|| HistoryError::FeatureNotFound(id.clone()))?;
                feature.status = HistoryStatus::BlockedByFailure;
                feature.diagnostic = None;
                blocked_features.push(id);
                continue;
            }
            if id == feature_id {
                active
                    .features
                    .get_mut(&id)
                    .ok_or_else(|| HistoryError::FeatureNotFound(id.clone()))?
                    .input_value = value;
            }
            let input_value = active
                .features
                .get(&id)
                .ok_or_else(|| HistoryError::FeatureNotFound(id.clone()))?
                .input_value;
            if input_value == 0.0 {
                let diagnostic = HistoryDiagnostic {
                    code: "historical_geometry_invalid".to_string(),
                    feature_id: id.clone(),
                    detail: format!("{parameter} must produce positive material"),
                };
                let feature = active
                    .features
                    .get_mut(&id)
                    .ok_or_else(|| HistoryError::FeatureNotFound(id.clone()))?;
                feature.last_valid_geometry_fingerprint = feature.geometry_fingerprint.clone();
                feature.status = HistoryStatus::Broken;
                feature.diagnostic = Some(diagnostic.clone());
                diagnostics.push(diagnostic);
                failed = true;
            } else {
                let fingerprint = geometry_fingerprint(
                    active
                        .features
                        .get(&id)
                        .ok_or_else(|| HistoryError::FeatureNotFound(id.clone()))?,
                    &active.features,
                );
                let feature = active
                    .features
                    .get_mut(&id)
                    .ok_or_else(|| HistoryError::FeatureNotFound(id.clone()))?;
                feature.geometry_fingerprint = Some(fingerprint);
                feature.last_valid_geometry_fingerprint = None;
                feature.status = HistoryStatus::CurrentValid;
                feature.diagnostic = None;
                evaluated_features.push(id);
            }
        }
        let evaluation = HistoryEvaluation {
            dirty_features: dirty_features.clone(),
            evaluated_features,
            blocked_features,
            diagnostics: diagnostics.clone(),
        };
        let operation = HistoryOperation::HistoricalEdit {
            feature_id: feature_id.to_string(),
            parameter: parameter.to_string(),
            value,
            dirty_features,
            evaluated_features: evaluation.evaluated_features.clone(),
            blocked_features: evaluation.blocked_features.clone(),
            diagnostics,
            preserved_name: Some(preserved_name),
        };
        Ok((self.event(operation, active, named_revisions), evaluation))
    }

    pub fn create_named_revision(&self, name: &str) -> Result<HistoryEvent, HistoryError> {
        if name.is_empty() {
            return Err(HistoryError::EmptyName);
        }
        if self.named_revisions.contains_key(name) {
            return Err(HistoryError::DuplicateName(name.to_string()));
        }
        let mut named_revisions = self.named_revisions.clone();
        named_revisions.insert(
            name.to_string(),
            NamedRevision {
                name: name.to_string(),
                snapshot: self.active.clone(),
                provenance: "explicit-create".to_string(),
            },
        );
        Ok(self.event(
            HistoryOperation::CreateNamedRevision {
                name: name.to_string(),
            },
            self.active.clone(),
            named_revisions,
        ))
    }

    pub fn restore_named_revision(&self, name: &str) -> Result<HistoryEvent, HistoryError> {
        let named = self
            .named_revisions
            .get(name)
            .ok_or_else(|| HistoryError::NamedRevisionNotFound(name.to_string()))?;
        let ordinal = self.event_ordinal + 1;
        let displaced_name = format!("recovered-before-restore-{ordinal}");
        let mut named_revisions = self.named_revisions.clone();
        named_revisions.insert(
            displaced_name.clone(),
            NamedRevision {
                name: displaced_name.clone(),
                snapshot: self.active.clone(),
                provenance: format!("restore:{name}"),
            },
        );
        Ok(self.event(
            HistoryOperation::RestoreNamedRevision {
                name: name.to_string(),
                displaced_name: Some(displaced_name),
            },
            named.snapshot.clone(),
            named_revisions,
        ))
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_bytes(&serde_json::to_vec(self).expect("history state serializes"))
    }

    fn event(
        &self,
        operation: HistoryOperation,
        active: HistorySnapshot,
        named_revisions: BTreeMap<String, NamedRevision>,
    ) -> HistoryEvent {
        HistoryEvent {
            schema_version: HISTORY_EVENT_SCHEMA.to_string(),
            ordinal: self.event_ordinal + 1,
            operation,
            active,
            named_revisions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEvaluation {
    pub dirty_features: Vec<String>,
    pub evaluated_features: Vec<String>,
    pub blocked_features: Vec<String>,
    pub diagnostics: Vec<HistoryDiagnostic>,
}

fn l_bracket_snapshot(
    bracket_id: &str,
    length: f64,
    width: f64,
    height: f64,
    thickness: f64,
    ordinal: u64,
) -> HistorySnapshot {
    let mut features = BTreeMap::new();
    for (id, dependencies, value) in [
        (format!("{bracket_id}-base"), Vec::new(), length),
        (
            format!("{bracket_id}-bend"),
            vec![format!("{bracket_id}-base")],
            width,
        ),
        (
            format!("{bracket_id}-finish"),
            vec![format!("{bracket_id}-bend")],
            height,
        ),
        (
            format!("{bracket_id}-independent-base"),
            Vec::new(),
            thickness,
        ),
        (
            format!("{bracket_id}-independent-finish"),
            vec![format!("{bracket_id}-independent-base")],
            height,
        ),
    ] {
        let mut feature = HistoryFeature {
            id: id.clone(),
            dependencies,
            input_value: value,
            geometry_fingerprint: None,
            last_valid_geometry_fingerprint: None,
            status: HistoryStatus::CurrentValid,
            diagnostic: None,
        };
        feature.geometry_fingerprint = Some(geometry_fingerprint(&feature, &features));
        features.insert(id, feature);
    }
    HistorySnapshot {
        revision_id: format!("history-revision-{ordinal}"),
        features,
    }
}

fn reachable_features(
    features: &BTreeMap<String, HistoryFeature>,
    target: &str,
) -> Result<Vec<String>, HistoryError> {
    if !features.contains_key(target) {
        return Err(HistoryError::FeatureNotFound(target.to_string()));
    }
    let mut dirty = BTreeSet::from([target.to_string()]);
    let mut changed = true;
    while changed {
        changed = false;
        for feature in features.values() {
            if feature
                .dependencies
                .iter()
                .any(|dependency| dirty.contains(dependency))
                && dirty.insert(feature.id.clone())
            {
                changed = true;
            }
        }
    }
    Ok(dirty.into_iter().collect())
}

fn topological_order(
    features: &BTreeMap<String, HistoryFeature>,
    dirty: &[String],
) -> Result<Vec<String>, HistoryError> {
    let dirty = dirty.iter().cloned().collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(dirty.len());
    let mut remaining = dirty;
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .find(|id| {
                features.get(*id).is_some_and(|feature| {
                    feature
                        .dependencies
                        .iter()
                        .all(|dependency| !remaining.contains(dependency))
                })
            })
            .cloned()
            .ok_or_else(|| HistoryError::InvalidEvent("history dependency cycle".to_string()))?;
        remaining.remove(&next);
        order.push(next);
    }
    Ok(order)
}

fn geometry_fingerprint(
    feature: &HistoryFeature,
    features: &BTreeMap<String, HistoryFeature>,
) -> String {
    let predecessor_fingerprints = feature
        .dependencies
        .iter()
        .map(|dependency| {
            features
                .get(dependency)
                .and_then(|item| item.geometry_fingerprint.as_deref())
                .unwrap_or("missing")
        })
        .collect::<Vec<_>>();
    let input = format!(
        "{}|{}|{}|{:?}",
        feature.id,
        feature.input_value,
        feature.dependencies.len(),
        predecessor_fingerprints
    );
    fingerprint_bytes(input.as_bytes())
}

fn validate_snapshot(snapshot: &HistorySnapshot) -> Result<(), HistoryError> {
    for (id, feature) in &snapshot.features {
        if id != &feature.id {
            return Err(HistoryError::InvalidEvent(format!(
                "feature key mismatch: {id}"
            )));
        }
        if !feature.input_value.is_finite() {
            return Err(HistoryError::InvalidValue);
        }
        for dependency in &feature.dependencies {
            if !snapshot.features.contains_key(dependency) {
                return Err(HistoryError::InvalidEvent(format!(
                    "missing dependency {dependency}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_named_revisions(
    revisions: &BTreeMap<String, NamedRevision>,
) -> Result<(), HistoryError> {
    for (name, revision) in revisions {
        if name.is_empty() || name != &revision.name {
            return Err(HistoryError::InvalidEvent(format!(
                "invalid named revision {name}"
            )));
        }
        validate_snapshot(&revision.snapshot)?;
    }
    Ok(())
}

fn fingerprint_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_failure_stops_only_the_affected_branch() {
        let state = HistoryState::default();
        let event = state
            .initialize_l_bracket("l", 10.0, 5.0, 3.0, 1.0)
            .expect("bracket event");
        let mut state = state;
        state.apply_event(&event).expect("initial event applies");

        let (event, evaluation) = state
            .historical_edit("l-base", "length", 0.0)
            .expect("edit event");
        assert_eq!(evaluation.dirty_features, ["l-base", "l-bend", "l-finish"]);
        assert_eq!(evaluation.evaluated_features, Vec::<String>::new());
        assert_eq!(evaluation.blocked_features, ["l-bend", "l-finish"]);
        assert_eq!(
            event.active.features["l-base"].status,
            HistoryStatus::Broken
        );
        assert_eq!(
            event.active.features["l-base"].last_valid_geometry_fingerprint,
            state.active.features["l-base"].geometry_fingerprint
        );
        assert_eq!(
            event.active.features["l-independent-finish"].status,
            HistoryStatus::CurrentValid
        );
    }

    #[test]
    fn replay_fingerprint_is_deterministic() {
        let mut state = HistoryState::default();
        let event = state
            .initialize_l_bracket("l", 10.0, 5.0, 3.0, 1.0)
            .expect("event");
        state.apply_event(&event).expect("applies");
        let first = state.fingerprint();
        let second = serde_json::from_slice::<HistoryState>(
            &serde_json::to_vec(&state).expect("serializes"),
        )
        .expect("round trips")
        .fingerprint();
        assert_eq!(first, second);
    }
}

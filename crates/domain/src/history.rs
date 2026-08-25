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
#[serde(deny_unknown_fields)]
pub struct HistoryDiagnostic {
    pub code: String,
    pub feature_id: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct HistorySnapshot {
    pub revision_id: String,
    pub features: BTreeMap<String, HistoryFeature>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedRevision {
    pub name: String,
    pub snapshot: HistorySnapshot,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryState {
    active: HistorySnapshot,
    named_revisions: BTreeMap<String, NamedRevision>,
    event_ordinal: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEvent {
    pub schema_version: String,
    pub ordinal: u64,
    pub operation: HistoryOperation,
    pub active: HistorySnapshot,
    pub named_revisions: BTreeMap<String, NamedRevision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
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
    FeatureNotInNamedRevision { feature_id: String, name: String },
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
            Self::FeatureNotInNamedRevision { feature_id, name } => write!(
                formatter,
                "feature {feature_id} is not present in named revision {name}"
            ),
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
    pub fn active_snapshot(&self) -> &HistorySnapshot {
        &self.active
    }

    pub fn named_revisions(&self) -> &BTreeMap<String, NamedRevision> {
        &self.named_revisions
    }

    pub fn event_ordinal(&self) -> u64 {
        self.event_ordinal
    }

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
        validate_event(event)?;
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
        if self.named_revisions.contains_key(&preserved_name) {
            return Err(HistoryError::DuplicateName(preserved_name));
        }
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
                if feature.last_valid_geometry_fingerprint.is_none() {
                    feature.last_valid_geometry_fingerprint = feature.geometry_fingerprint.clone();
                }
                feature.geometry_fingerprint = None;
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
            if input_value <= 0.0 {
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
                feature.geometry_fingerprint = None;
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
        if self.named_revisions.contains_key(&displaced_name) {
            return Err(HistoryError::DuplicateName(displaced_name));
        }
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

    pub fn restore_named_revision_for_feature(
        &self,
        feature_id: &str,
        name: &str,
    ) -> Result<HistoryEvent, HistoryError> {
        if !self.active.features.contains_key(feature_id) {
            return Err(HistoryError::FeatureNotFound(feature_id.to_string()));
        }
        let named = self
            .named_revisions
            .get(name)
            .ok_or_else(|| HistoryError::NamedRevisionNotFound(name.to_string()))?;
        if !named.snapshot.features.contains_key(feature_id) {
            return Err(HistoryError::FeatureNotInNamedRevision {
                feature_id: feature_id.to_string(),
                name: name.to_string(),
            });
        }
        self.restore_named_revision(name)
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryTimelineStatus {
    CurrentValid,
    Broken,
    BlockedByFailure,
    Suppressed,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryTimelineEntry {
    pub ordinal: u64,
    pub revision_id: String,
    pub operation: String,
    pub status: HistoryTimelineStatus,
    pub named_revision_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryNamedRevisionSummary {
    pub name: String,
    pub revision_id: String,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryTimeline {
    pub feature_id: String,
    pub active_revision: String,
    pub revisions: Vec<HistoryTimelineEntry>,
    pub named_revisions: Vec<HistoryNamedRevisionSummary>,
}

pub fn project_feature_timeline(
    events: &[HistoryEvent],
    feature_id: &str,
) -> Result<HistoryTimeline, HistoryError> {
    if feature_id.is_empty() {
        return Err(HistoryError::FeatureNotFound(feature_id.to_string()));
    }

    let mut state = HistoryState::default();
    let mut seen = false;
    let mut revisions = Vec::new();
    for event in events {
        let before = state.clone();
        state.apply_event(event)?;
        let after_feature = state.active.features.get(feature_id);
        let before_feature = before.active.features.get(feature_id);
        seen |= after_feature.is_some()
            || before_feature.is_some()
            || state
                .named_revisions
                .values()
                .any(|revision| revision.snapshot.features.contains_key(feature_id));

        let mut named_revision_names = state
            .named_revisions
            .iter()
            .filter(|(name, revision)| {
                !before.named_revisions.contains_key(*name)
                    && revision.snapshot.features.contains_key(feature_id)
            })
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        named_revision_names.sort();

        let active_changed = before_feature != after_feature;
        let operation_targets_feature = match &event.operation {
            HistoryOperation::InitializeLBracket { .. } => active_changed,
            HistoryOperation::HistoricalEdit {
                feature_id: edited_feature,
                dirty_features,
                ..
            } => edited_feature == feature_id || dirty_features.iter().any(|id| id == feature_id),
            HistoryOperation::CreateNamedRevision { .. } => false,
            HistoryOperation::RestoreNamedRevision { name, .. } => {
                before_feature.is_some()
                    || after_feature.is_some()
                    || state
                        .named_revisions
                        .get(name)
                        .is_some_and(|revision| revision.snapshot.features.contains_key(feature_id))
            }
        };

        if active_changed || operation_targets_feature || !named_revision_names.is_empty() {
            revisions.push(HistoryTimelineEntry {
                ordinal: event.ordinal,
                revision_id: event.active.revision_id.clone(),
                operation: history_operation_name(&event.operation).to_string(),
                status: after_feature
                    .map(|feature| history_timeline_status(feature.status))
                    .unwrap_or(HistoryTimelineStatus::Absent),
                named_revision_names,
            });
        }
    }

    if !seen {
        return Err(HistoryError::FeatureNotFound(feature_id.to_string()));
    }

    let named_revisions = state
        .named_revisions
        .values()
        .filter(|revision| revision.snapshot.features.contains_key(feature_id))
        .map(|revision| HistoryNamedRevisionSummary {
            name: revision.name.clone(),
            revision_id: revision.snapshot.revision_id.clone(),
            provenance: revision.provenance.clone(),
        })
        .collect();
    Ok(HistoryTimeline {
        feature_id: feature_id.to_string(),
        active_revision: state.active.revision_id,
        revisions,
        named_revisions,
    })
}

fn history_operation_name(operation: &HistoryOperation) -> &'static str {
    match operation {
        HistoryOperation::InitializeLBracket { .. } => "initialize-l-bracket",
        HistoryOperation::HistoricalEdit { .. } => "historical-edit",
        HistoryOperation::CreateNamedRevision { .. } => "create-named-revision",
        HistoryOperation::RestoreNamedRevision { .. } => "restore-named-revision",
    }
}

fn history_timeline_status(status: HistoryStatus) -> HistoryTimelineStatus {
    match status {
        HistoryStatus::CurrentValid => HistoryTimelineStatus::CurrentValid,
        HistoryStatus::Broken => HistoryTimelineStatus::Broken,
        HistoryStatus::BlockedByFailure => HistoryTimelineStatus::BlockedByFailure,
        HistoryStatus::Suppressed => HistoryTimelineStatus::Suppressed,
    }
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
        match feature.status {
            HistoryStatus::CurrentValid => {
                if feature.geometry_fingerprint.is_none()
                    || feature.last_valid_geometry_fingerprint.is_some()
                    || feature.diagnostic.is_some()
                {
                    return Err(HistoryError::InvalidEvent(format!(
                        "current feature state is inconsistent: {id}"
                    )));
                }
            }
            HistoryStatus::Broken => {
                if feature.geometry_fingerprint.is_some()
                    || feature.last_valid_geometry_fingerprint.is_none()
                    || feature.diagnostic.is_none()
                {
                    return Err(HistoryError::InvalidEvent(format!(
                        "broken feature state is inconsistent: {id}"
                    )));
                }
            }
            HistoryStatus::BlockedByFailure | HistoryStatus::Suppressed => {
                if feature.geometry_fingerprint.is_some() || feature.diagnostic.is_some() {
                    return Err(HistoryError::InvalidEvent(format!(
                        "non-current feature state is inconsistent: {id}"
                    )));
                }
            }
        }
    }
    let all_features = snapshot.features.keys().cloned().collect::<Vec<_>>();
    topological_order(&snapshot.features, &all_features)?;
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

fn validate_event(event: &HistoryEvent) -> Result<(), HistoryError> {
    validate_snapshot(&event.active)?;
    validate_named_revisions(&event.named_revisions)?;
    match &event.operation {
        HistoryOperation::InitializeLBracket { bracket_id, .. } => {
            if bracket_id.is_empty() || event.active.features.is_empty() {
                return Err(HistoryError::InvalidEvent(
                    "initialization must contain a bracket graph".to_string(),
                ));
            }
        }
        HistoryOperation::HistoricalEdit {
            feature_id,
            dirty_features,
            evaluated_features,
            blocked_features,
            diagnostics,
            preserved_name,
            ..
        } => {
            if !event.active.features.contains_key(feature_id)
                || preserved_name
                    .as_ref()
                    .is_none_or(|name| !event.named_revisions.contains_key(name))
            {
                return Err(HistoryError::InvalidEvent(
                    "historical edit references missing state".to_string(),
                ));
            }
            let dirty = dirty_features.iter().collect::<BTreeSet<_>>();
            if evaluated_features.iter().any(|id| !dirty.contains(id))
                || blocked_features.iter().any(|id| !dirty.contains(id))
                || evaluated_features
                    .iter()
                    .any(|id| blocked_features.contains(id))
                || dirty_features
                    .iter()
                    .any(|id| !event.active.features.contains_key(id))
            {
                return Err(HistoryError::InvalidEvent(
                    "historical edit affected-set metadata is inconsistent".to_string(),
                ));
            }
            for diagnostic in diagnostics {
                if event
                    .active
                    .features
                    .get(&diagnostic.feature_id)
                    .is_none_or(|feature| feature.status != HistoryStatus::Broken)
                {
                    return Err(HistoryError::InvalidEvent(
                        "diagnostic does not identify a broken feature".to_string(),
                    ));
                }
            }
        }
        HistoryOperation::CreateNamedRevision { name } => {
            if !event.named_revisions.contains_key(name) {
                return Err(HistoryError::InvalidEvent(
                    "created named revision is missing".to_string(),
                ));
            }
        }
        HistoryOperation::RestoreNamedRevision { name, .. } => {
            let named = event.named_revisions.get(name).ok_or_else(|| {
                HistoryError::InvalidEvent("restored revision is missing".to_string())
            })?;
            if named.snapshot != event.active {
                return Err(HistoryError::InvalidEvent(
                    "restore active snapshot does not match the named snapshot".to_string(),
                ));
            }
        }
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
    fn historical_failure_stops_only_the_affected_dependency_path() {
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

    #[test]
    fn positive_historical_edit_recomputes_the_affected_dependency_path() {
        let mut state = HistoryState::default();
        let event = state
            .initialize_l_bracket("l", 10.0, 5.0, 3.0, 1.0)
            .expect("event");
        state.apply_event(&event).expect("applies");

        let (event, evaluation) = state
            .historical_edit("l-base", "length", 12.0)
            .expect("valid edit");
        assert_eq!(evaluation.dirty_features, ["l-base", "l-bend", "l-finish"]);
        assert_eq!(evaluation.evaluated_features, evaluation.dirty_features);
        assert!(evaluation.blocked_features.is_empty());
        assert!(evaluation.diagnostics.is_empty());
        assert!(
            event
                .active
                .features
                .values()
                .all(|feature| feature.status == HistoryStatus::CurrentValid)
        );
    }

    #[test]
    fn generated_recovery_name_collision_preserves_the_existing_named_revision() {
        let mut state = HistoryState::default();
        let event = state
            .initialize_l_bracket("l", 10.0, 5.0, 3.0, 1.0)
            .expect("event");
        state.apply_event(&event).expect("applies");
        let name = "recovered-before-historical-edit-3";
        let event = state.create_named_revision(name).expect("named revision");
        state.apply_event(&event).expect("applies");
        assert!(matches!(
            state.historical_edit("l-base", "length", 0.0),
            Err(HistoryError::DuplicateName(_))
        ));
    }

    #[test]
    fn feature_timeline_scopes_active_revisions_and_named_markers() {
        let mut state = HistoryState::default();
        let mut events = Vec::new();
        let event = state
            .initialize_l_bracket("first", 10.0, 5.0, 3.0, 1.0)
            .expect("first bracket");
        state.apply_event(&event).expect("event applies");
        events.push(event);
        let event = state
            .create_named_revision("before-second")
            .expect("named revision");
        state.apply_event(&event).expect("event applies");
        events.push(event);
        let event = state
            .initialize_l_bracket("second", 8.0, 4.0, 2.0, 1.0)
            .expect("second bracket");
        state.apply_event(&event).expect("event applies");
        events.push(event);
        let (event, _) = state
            .historical_edit("first-base", "length", 12.0)
            .expect("historical edit");
        state.apply_event(&event).expect("event applies");
        events.push(event);

        let first = project_feature_timeline(&events, "first-base").expect("first timeline");
        assert_eq!(
            first
                .revisions
                .iter()
                .map(|entry| entry.ordinal)
                .collect::<Vec<_>>(),
            vec![1, 2, 4]
        );
        assert_eq!(first.named_revisions[0].name, "before-second");

        let second = project_feature_timeline(&events, "second-base").expect("second timeline");
        assert_eq!(
            second
                .revisions
                .iter()
                .map(|entry| entry.ordinal)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(
            second.revisions[1].named_revision_names,
            ["recovered-before-historical-edit-4"]
        );
        assert!(matches!(
            project_feature_timeline(&events, "missing"),
            Err(HistoryError::FeatureNotFound(_))
        ));
    }

    #[test]
    fn feature_scoped_restore_rejects_a_named_snapshot_without_the_feature() {
        let mut state = HistoryState::default();
        let event = state
            .initialize_l_bracket("first", 10.0, 5.0, 3.0, 1.0)
            .expect("first bracket");
        state.apply_event(&event).expect("event applies");
        let event = state
            .create_named_revision("before-second")
            .expect("named revision");
        state.apply_event(&event).expect("event applies");
        let event = state
            .initialize_l_bracket("second", 8.0, 4.0, 2.0, 1.0)
            .expect("second bracket");
        state.apply_event(&event).expect("event applies");

        assert_eq!(
            state
                .restore_named_revision_for_feature("second-base", "before-second")
                .expect_err("scope mismatch"),
            HistoryError::FeatureNotInNamedRevision {
                feature_id: "second-base".to_string(),
                name: "before-second".to_string(),
            }
        );
        let event = state
            .restore_named_revision_for_feature("first-base", "before-second")
            .expect("matching scope restores");
        assert_eq!(event.active.revision_id, "history-revision-1");
        assert!(!event.active.features.contains_key("second-base"));
    }
}

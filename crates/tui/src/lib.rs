use threeterm_domain::FeatureGraph;
use threeterm_theme::{NonColorMarker, SemanticToken, TransientState, transient_visuals};

pub fn schema_version() -> &'static str {
    "threeterm.tui/1"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    InteractiveReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusState {
    Focused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionMode {
    ModelessReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPhase {
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryState {
    Linear { can_undo: bool, can_redo: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTarget {
    pub id: String,
    pub label: String,
}

impl FeatureTarget {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowKey {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavigationDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationResult {
    Moved,
    Boundary,
    NoFeatureTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GestureAcknowledgement {
    pub sequence: u64,
    pub key: ArrowKey,
    pub result: NavigationResult,
    pub text: String,
    pub marker: NonColorMarker,
    pub color: Option<SemanticToken>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiState {
    pub lifecycle: LifecycleState,
    pub focus: FocusState,
    pub capture: CaptureState,
    pub selected_target: Option<String>,
    pub interaction_mode: InteractionMode,
    pub command_phase: CommandPhase,
    pub history: HistoryState,
    pub canonical_revision: String,
    pub last_acknowledgement: Option<GestureAcknowledgement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiFrame {
    pub selected_target: Option<String>,
    pub acknowledgement: GestureAcknowledgement,
}

impl TuiFrame {
    pub fn render_overlay(&self) -> String {
        format!(
            "[{}] {}",
            self.acknowledgement.marker.as_str(),
            self.acknowledgement.text
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiDiagnosticCode {
    NoFeatureTarget,
    InvalidArrowInput,
}

impl TuiDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoFeatureTarget => "no_feature_target",
            Self::InvalidArrowInput => "invalid_arrow_input",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiDiagnostic {
    pub code: TuiDiagnosticCode,
    pub detail: String,
    pub canonical_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputOutcome {
    pub frame: TuiFrame,
    pub diagnostic: Option<TuiDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedInput {
    pub frame: TuiFrame,
    pub overlay: String,
    pub diagnostic: Option<TuiDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct TuiSession {
    targets: Vec<FeatureTarget>,
    selected_index: Option<usize>,
    canonical_revision: String,
    acknowledgement_sequence: u64,
    last_acknowledgement: Option<GestureAcknowledgement>,
}

impl TuiSession {
    pub fn new(
        targets: impl IntoIterator<Item = FeatureTarget>,
        canonical_revision: impl AsRef<str>,
    ) -> Self {
        Self {
            targets: targets.into_iter().collect(),
            selected_index: None,
            canonical_revision: canonical_revision.as_ref().to_string(),
            acknowledgement_sequence: 0,
            last_acknowledgement: None,
        }
    }

    pub fn from_feature_graph(graph: &FeatureGraph, canonical_revision: impl AsRef<str>) -> Self {
        let targets = graph
            .features()
            .map(|feature| FeatureTarget::new(feature.id.as_str(), feature.kind));
        Self::new(targets, canonical_revision)
    }

    pub fn state(&self) -> TuiState {
        TuiState {
            lifecycle: LifecycleState::InteractiveReady,
            focus: FocusState::Focused,
            capture: CaptureState::None,
            selected_target: self.selected_target().map(str::to_string),
            interaction_mode: InteractionMode::ModelessReady,
            command_phase: CommandPhase::Idle,
            history: HistoryState::Linear {
                can_undo: false,
                can_redo: false,
            },
            canonical_revision: self.canonical_revision.clone(),
            last_acknowledgement: self.last_acknowledgement.clone(),
        }
    }

    pub fn press(&mut self, key: ArrowKey) -> InputOutcome {
        self.acknowledgement_sequence += 1;
        let sequence = self.acknowledgement_sequence;
        let direction = key.direction();

        if self.targets.is_empty() {
            let acknowledgement = self.acknowledgement(
                sequence,
                key,
                NavigationResult::NoFeatureTarget,
                &format!("Acknowledgement {sequence}: no feature target is available"),
                TransientState::Error,
            );
            let diagnostic = TuiDiagnostic {
                code: TuiDiagnosticCode::NoFeatureTarget,
                detail: "arrow navigation requires at least one feature target".to_string(),
                canonical_revision: self.canonical_revision.clone(),
            };
            return self.finish(acknowledgement, Some(diagnostic));
        }

        let last_index = self.targets.len() - 1;
        let (index, result) = match (self.selected_index, direction) {
            (None, NavigationDirection::Previous) => (last_index, NavigationResult::Moved),
            (None, NavigationDirection::Next) => (0, NavigationResult::Moved),
            (Some(index), NavigationDirection::Previous) if index > 0 => {
                (index - 1, NavigationResult::Moved)
            }
            (Some(index), NavigationDirection::Next) if index < last_index => {
                (index + 1, NavigationResult::Moved)
            }
            (Some(index), _) => (index, NavigationResult::Boundary),
        };
        self.selected_index = Some(index);
        let target = &self.targets[index];
        let text = match result {
            NavigationResult::Moved => {
                format!(
                    "Acknowledgement {sequence}: selected feature {} ({})",
                    target.id,
                    index + 1,
                )
            }
            NavigationResult::Boundary => {
                format!(
                    "Acknowledgement {sequence}: selection boundary at feature {} ({})",
                    target.id,
                    index + 1
                )
            }
            NavigationResult::NoFeatureTarget => unreachable!("empty targets returned above"),
        };
        let acknowledgement =
            self.acknowledgement(sequence, key, result, &text, TransientState::Selected);
        self.finish(acknowledgement, None)
    }

    pub fn process_terminal_input(&mut self, bytes: &[u8]) -> Result<RenderedInput, TuiDiagnostic> {
        let key = decode_arrow(bytes).ok_or_else(|| TuiDiagnostic {
            code: TuiDiagnosticCode::InvalidArrowInput,
            detail: "expected one legacy terminal arrow sequence".to_string(),
            canonical_revision: self.canonical_revision.clone(),
        })?;
        let outcome = self.press(key);
        let overlay = outcome.frame.render_overlay();
        Ok(RenderedInput {
            frame: outcome.frame,
            overlay,
            diagnostic: outcome.diagnostic,
        })
    }

    fn finish(
        &mut self,
        acknowledgement: GestureAcknowledgement,
        diagnostic: Option<TuiDiagnostic>,
    ) -> InputOutcome {
        self.last_acknowledgement = Some(acknowledgement.clone());
        InputOutcome {
            frame: TuiFrame {
                selected_target: self.selected_target().map(str::to_string),
                acknowledgement,
            },
            diagnostic,
        }
    }

    fn acknowledgement(
        &self,
        sequence: u64,
        key: ArrowKey,
        result: NavigationResult,
        text: &str,
        state: TransientState,
    ) -> GestureAcknowledgement {
        let visual = transient_visuals()
            .iter()
            .find(|visual| visual.state == state)
            .expect("theme transient state mapping is complete");
        GestureAcknowledgement {
            sequence,
            key,
            result,
            text: text.to_string(),
            marker: visual.marker.expect("theme marker is present"),
            color: visual.color,
        }
    }

    fn selected_target(&self) -> Option<&str> {
        self.selected_index
            .and_then(|index| self.targets.get(index))
            .map(|target| target.id.as_str())
    }
}

impl ArrowKey {
    fn direction(self) -> NavigationDirection {
        match self {
            Self::Up | Self::Left => NavigationDirection::Previous,
            Self::Down | Self::Right => NavigationDirection::Next,
        }
    }
}

fn decode_arrow(bytes: &[u8]) -> Option<ArrowKey> {
    match bytes {
        b"\x1b[A" => Some(ArrowKey::Up),
        b"\x1b[B" => Some(ArrowKey::Down),
        b"\x1b[C" => Some(ArrowKey::Right),
        b"\x1b[D" => Some(ArrowKey::Left),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.tui/1");
    }
}

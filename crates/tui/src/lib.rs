use threeterm_domain::FeatureGraph;
use threeterm_theme::{NonColorMarker, SemanticToken, TransientState, transient_visuals};

pub fn schema_version() -> &'static str {
    "threeterm.tui/1"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Probing,
    InteractiveReady,
    HeadlessOnly,
    Restoring,
    Resizing,
    Closing,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusState {
    Focused,
    FocusLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureState {
    None,
    PointerCapture(PointerCapture),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionMode {
    ModelessReady,
    DragActive { tool: InteractionTool },
    CommandModal,
    HistoryApplying,
    RecoveryReady,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPhase {
    Idle,
    Draft {
        command: String,
        input_fingerprint: String,
    },
    Previewing {
        input_fingerprint: String,
    },
    PreviewReady {
        input_fingerprint: String,
    },
    Committing {
        input_fingerprint: String,
    },
    Cancelling,
    Outcome {
        outcome: CommandOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Committed { revision: String },
    Rejected { detail: String },
    Cancelled { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewResult {
    Ready,
    Rejected { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEvent {
    Open {
        command: String,
    },
    DraftUpdated {
        input_fingerprint: String,
    },
    PreviewRequested,
    PreviewCompleted(PreviewResult),
    CommitRequested,
    CommitAccepted {
        source_revision: String,
        validated_revision: String,
        revision: String,
    },
    CommitRejected {
        detail: String,
    },
    CancelRequested,
    CancellationCompleted {
        detail: String,
    },
    OutcomeDismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEventKind {
    Open,
    DraftUpdated,
    PreviewRequested,
    PreviewReady,
    PreviewRejected,
    CommitRequested,
    CommitAccepted,
    CommitRejected,
    CancelRequested,
    CancellationCompleted,
    OutcomeDismissed,
}

impl CommandEvent {
    fn kind(&self) -> CommandEventKind {
        match self {
            Self::Open { .. } => CommandEventKind::Open,
            Self::DraftUpdated { .. } => CommandEventKind::DraftUpdated,
            Self::PreviewRequested => CommandEventKind::PreviewRequested,
            Self::PreviewCompleted(PreviewResult::Ready) => CommandEventKind::PreviewReady,
            Self::PreviewCompleted(PreviewResult::Rejected { .. }) => {
                CommandEventKind::PreviewRejected
            }
            Self::CommitRequested => CommandEventKind::CommitRequested,
            Self::CommitAccepted { .. } => CommandEventKind::CommitAccepted,
            Self::CommitRejected { .. } => CommandEventKind::CommitRejected,
            Self::CancelRequested => CommandEventKind::CancelRequested,
            Self::CancellationCompleted { .. } => CommandEventKind::CancellationCompleted,
            Self::OutcomeDismissed => CommandEventKind::OutcomeDismissed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryState {
    Linear {
        can_undo: bool,
        can_redo: bool,
    },
    Applying {
        direction: HistoryDirection,
        can_undo: bool,
        can_redo: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryDirection {
    Undo,
    Redo,
    NamedRevision { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryApplyResult {
    Applied {
        revision: String,
        can_undo: bool,
        can_redo: bool,
    },
    Rejected {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryEvent {
    UndoRequested,
    RedoRequested,
    RestoreNamedRevision {
        name: String,
    },
    ApplyCompleted(HistoryApplyResult),
    DivergentCommit {
        revision: String,
        preserved_named_revision: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryEventKind {
    UndoRequested,
    RedoRequested,
    RestoreNamedRevision,
    ApplyCompleted,
    DivergentCommit,
}

impl HistoryEvent {
    fn kind(&self) -> HistoryEventKind {
        match self {
            Self::UndoRequested => HistoryEventKind::UndoRequested,
            Self::RedoRequested => HistoryEventKind::RedoRequested,
            Self::RestoreNamedRevision { .. } => HistoryEventKind::RestoreNamedRevision,
            Self::ApplyCompleted(_) => HistoryEventKind::ApplyCompleted,
            Self::DivergentCommit { .. } => HistoryEventKind::DivergentCommit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateAxis {
    Lifecycle,
    FocusCapture,
    Selection,
    InteractionMode,
    CommandPhase,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleEvent {
    ProbeStarted,
    ProbeFailed { detail: String },
    ResizeStarted,
    ResizeCompleted,
    ResizeFailed { detail: String },
    ProbeSucceeded,
    RuntimeFailure { detail: String },
    RestoreCompleted,
    CloseRequested,
    CleanupCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateEvent {
    Lifecycle(LifecycleEvent),
    FocusCapture(FocusCaptureEvent),
    Selection(SelectionEvent),
    Command(CommandEvent),
    Interaction(InteractionEvent),
    History(HistoryEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateEventKind {
    Lifecycle(LifecycleEventKind),
    FocusCapture(FocusCaptureEventKind),
    Selection(SelectionEventKind),
    Command(CommandEventKind),
    Interaction(InteractionEventKind),
    History(HistoryEventKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEventKind {
    ProbeStarted,
    ProbeFailed,
    ResizeStarted,
    ResizeCompleted,
    ResizeFailed,
    ProbeSucceeded,
    RuntimeFailure,
    RestoreCompleted,
    CloseRequested,
    CleanupCompleted,
}

impl LifecycleEvent {
    fn kind(&self) -> LifecycleEventKind {
        match self {
            Self::ProbeStarted => LifecycleEventKind::ProbeStarted,
            Self::ProbeFailed { .. } => LifecycleEventKind::ProbeFailed,
            Self::ResizeStarted => LifecycleEventKind::ResizeStarted,
            Self::ResizeCompleted => LifecycleEventKind::ResizeCompleted,
            Self::ResizeFailed { .. } => LifecycleEventKind::ResizeFailed,
            Self::ProbeSucceeded => LifecycleEventKind::ProbeSucceeded,
            Self::RuntimeFailure { .. } => LifecycleEventKind::RuntimeFailure,
            Self::RestoreCompleted => LifecycleEventKind::RestoreCompleted,
            Self::CloseRequested => LifecycleEventKind::CloseRequested,
            Self::CleanupCompleted => LifecycleEventKind::CleanupCompleted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionTool {
    Selection,
    Orbit,
    Pan,
    Zoom,
    SketchPlacement,
    PropertyEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerOrigin {
    pub column: u16,
    pub row: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerCapture {
    pub tool: InteractionTool,
    pub origin: PointerOrigin,
    pub candidate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusCaptureEvent {
    FocusLost,
    FocusIn,
    RecoveryCompleted,
    PointerPressed {
        tool: InteractionTool,
        origin: PointerOrigin,
        candidate: Option<String>,
    },
    PointerMoved {
        candidate: Option<String>,
    },
    PointerReleased,
    DragStarted,
    DragFinished,
    CaptureCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusCaptureEventKind {
    FocusLost,
    FocusIn,
    RecoveryCompleted,
    PointerPressed,
    PointerMoved,
    PointerReleased,
    DragStarted,
    DragFinished,
    CaptureCancelled,
}

impl FocusCaptureEvent {
    fn kind(&self) -> FocusCaptureEventKind {
        match self {
            Self::FocusLost => FocusCaptureEventKind::FocusLost,
            Self::FocusIn => FocusCaptureEventKind::FocusIn,
            Self::RecoveryCompleted => FocusCaptureEventKind::RecoveryCompleted,
            Self::PointerPressed { .. } => FocusCaptureEventKind::PointerPressed,
            Self::PointerMoved { .. } => FocusCaptureEventKind::PointerMoved,
            Self::PointerReleased => FocusCaptureEventKind::PointerReleased,
            Self::DragStarted => FocusCaptureEventKind::DragStarted,
            Self::DragFinished => FocusCaptureEventKind::DragFinished,
            Self::CaptureCancelled => FocusCaptureEventKind::CaptureCancelled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionState {
    None,
    Candidate {
        candidates: Vec<String>,
        previous: Option<Vec<String>>,
    },
    Selected {
        stable_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionVerification {
    Exact { stable_ids: Vec<String> },
    Ambiguous { stable_ids: Vec<String> },
    Lost,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionEvent {
    Nominate { candidates: Vec<String> },
    Verify(SelectionVerification),
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionEventKind {
    Nominate,
    VerifyExact,
    VerifyAmbiguous,
    VerifyLost,
    VerifyIncompatible,
    Clear,
}

impl SelectionEvent {
    fn kind(&self) -> SelectionEventKind {
        match self {
            Self::Nominate { .. } => SelectionEventKind::Nominate,
            Self::Verify(SelectionVerification::Exact { .. }) => SelectionEventKind::VerifyExact,
            Self::Verify(SelectionVerification::Ambiguous { .. }) => {
                SelectionEventKind::VerifyAmbiguous
            }
            Self::Verify(SelectionVerification::Lost) => SelectionEventKind::VerifyLost,
            Self::Verify(SelectionVerification::Incompatible) => {
                SelectionEventKind::VerifyIncompatible
            }
            Self::Clear => SelectionEventKind::Clear,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionEvent {
    StartDrag { tool: InteractionTool },
    FinishDrag,
    OpenCommand { command: String },
    CloseCommand,
    StartHistory { direction: HistoryDirection },
    RecoveryCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionEventKind {
    StartDrag,
    FinishDrag,
    OpenCommand,
    CloseCommand,
    StartHistory,
    RecoveryCompleted,
}

impl InteractionEvent {
    fn kind(&self) -> InteractionEventKind {
        match self {
            Self::StartDrag { .. } => InteractionEventKind::StartDrag,
            Self::FinishDrag => InteractionEventKind::FinishDrag,
            Self::OpenCommand { .. } => InteractionEventKind::OpenCommand,
            Self::CloseCommand => InteractionEventKind::CloseCommand,
            Self::StartHistory { .. } => InteractionEventKind::StartHistory,
            Self::RecoveryCompleted => InteractionEventKind::RecoveryCompleted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateAcknowledgement {
    pub sequence: u64,
    pub event: StateEventKind,
    pub text: String,
    pub marker: NonColorMarker,
    pub color: Option<SemanticToken>,
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
    pub selection: SelectionState,
    pub interaction_mode: InteractionMode,
    pub command_phase: CommandPhase,
    pub command_source_revision: Option<String>,
    pub history: HistoryState,
    pub recoverable_revisions: Vec<String>,
    pub presentation_generation: u64,
    pub canonical_revision: String,
    pub last_acknowledgement: Option<GestureAcknowledgement>,
    pub last_transition_acknowledgement: Option<StateAcknowledgement>,
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
    InvalidTransition,
    AmbiguousSelection,
    CommandRejected,
    HistoryRejected,
    LifecycleFailure,
    CommandCancelled,
    SelectionLost,
    SelectionIncompatible,
    StalePreview,
}

impl TuiDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoFeatureTarget => "no_feature_target",
            Self::InvalidArrowInput => "invalid_arrow_input",
            Self::InvalidTransition => "invalid_transition",
            Self::AmbiguousSelection => "ambiguous_selection",
            Self::CommandRejected => "command_rejected",
            Self::HistoryRejected => "history_rejected",
            Self::LifecycleFailure => "lifecycle_failure",
            Self::CommandCancelled => "command_cancelled",
            Self::SelectionLost => "selection_lost",
            Self::SelectionIncompatible => "selection_incompatible",
            Self::StalePreview => "stale_preview",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiDiagnostic {
    pub code: TuiDiagnosticCode,
    pub detail: String,
    pub canonical_revision: String,
    pub axis: Option<StateAxis>,
    pub event: Option<StateEventKind>,
    pub from: Option<String>,
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
    lifecycle: LifecycleState,
    focus: FocusState,
    capture: CaptureState,
    selection: SelectionState,
    interaction_mode: InteractionMode,
    command_phase: CommandPhase,
    active_command: Option<String>,
    command_source_revision: Option<String>,
    history: HistoryState,
    recoverable_revisions: Vec<String>,
    presentation_generation: u64,
    canonical_revision: String,
    acknowledgement_sequence: u64,
    last_acknowledgement: Option<GestureAcknowledgement>,
    transition_sequence: u64,
    last_transition_acknowledgement: Option<StateAcknowledgement>,
}

impl TuiSession {
    pub fn new(
        targets: impl IntoIterator<Item = FeatureTarget>,
        canonical_revision: impl AsRef<str>,
    ) -> Self {
        Self {
            targets: targets.into_iter().collect(),
            selected_index: None,
            lifecycle: LifecycleState::InteractiveReady,
            focus: FocusState::Focused,
            capture: CaptureState::None,
            selection: SelectionState::None,
            interaction_mode: InteractionMode::ModelessReady,
            command_phase: CommandPhase::Idle,
            active_command: None,
            command_source_revision: None,
            history: HistoryState::Linear {
                can_undo: false,
                can_redo: false,
            },
            recoverable_revisions: Vec::new(),
            presentation_generation: 0,
            canonical_revision: canonical_revision.as_ref().to_string(),
            acknowledgement_sequence: 0,
            last_acknowledgement: None,
            transition_sequence: 0,
            last_transition_acknowledgement: None,
        }
    }

    pub fn new_probing(
        targets: impl IntoIterator<Item = FeatureTarget>,
        canonical_revision: impl AsRef<str>,
    ) -> Self {
        let mut session = Self::new(targets, canonical_revision);
        session.lifecycle = LifecycleState::Probing;
        session
    }

    pub fn from_feature_graph(graph: &FeatureGraph, canonical_revision: impl AsRef<str>) -> Self {
        let targets = graph
            .features()
            .map(|feature| FeatureTarget::new(feature.id.as_str(), feature.kind));
        Self::new(targets, canonical_revision)
    }

    pub fn from_feature_graph_probing(
        graph: &FeatureGraph,
        canonical_revision: impl AsRef<str>,
    ) -> Self {
        let targets = graph
            .features()
            .map(|feature| FeatureTarget::new(feature.id.as_str(), feature.kind));
        Self::new_probing(targets, canonical_revision)
    }

    pub fn state(&self) -> TuiState {
        TuiState {
            lifecycle: self.lifecycle,
            focus: self.focus,
            capture: self.capture.clone(),
            selected_target: self.selected_target().map(str::to_string),
            selection: self.selection.clone(),
            interaction_mode: self.interaction_mode.clone(),
            command_phase: self.command_phase.clone(),
            command_source_revision: self.command_source_revision.clone(),
            history: self.history.clone(),
            recoverable_revisions: self.recoverable_revisions.clone(),
            presentation_generation: self.presentation_generation,
            canonical_revision: self.canonical_revision.clone(),
            last_acknowledgement: self.last_acknowledgement.clone(),
            last_transition_acknowledgement: self.last_transition_acknowledgement.clone(),
        }
    }

    pub fn transition_lifecycle(
        &mut self,
        event: LifecycleEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.handle_lifecycle(event)
    }

    pub fn transition_focus_capture(
        &mut self,
        event: FocusCaptureEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.handle_focus_capture(event)
    }

    pub fn transition_selection(
        &mut self,
        event: SelectionEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.handle_selection(event)
    }

    pub fn transition_interaction(
        &mut self,
        event: InteractionEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.handle_interaction(event)
    }

    pub fn transition_command(
        &mut self,
        event: CommandEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.handle_command(event)
    }

    pub fn transition_history(
        &mut self,
        event: HistoryEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.handle_history(event)
    }

    pub fn transition(&mut self, event: StateEvent) -> Result<StateTransition, TuiDiagnostic> {
        match event {
            StateEvent::Lifecycle(event) => self.handle_lifecycle(event),
            StateEvent::FocusCapture(event) => self.handle_focus_capture(event),
            StateEvent::Selection(event) => self.handle_selection(event),
            StateEvent::Interaction(event) => self.handle_interaction(event),
            StateEvent::Command(event) => self.handle_command(event),
            StateEvent::History(event) => self.handle_history(event),
        }
    }

    fn handle_lifecycle(
        &mut self,
        event: LifecycleEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.apply_transition(StateEvent::Lifecycle(event))
    }

    fn handle_focus_capture(
        &mut self,
        event: FocusCaptureEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.apply_transition(StateEvent::FocusCapture(event))
    }

    fn handle_selection(
        &mut self,
        event: SelectionEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.apply_transition(StateEvent::Selection(event))
    }

    fn handle_interaction(
        &mut self,
        event: InteractionEvent,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.apply_transition(StateEvent::Interaction(event))
    }

    fn handle_command(&mut self, event: CommandEvent) -> Result<StateTransition, TuiDiagnostic> {
        self.apply_transition(StateEvent::Command(event))
    }

    fn handle_history(&mut self, event: HistoryEvent) -> Result<StateTransition, TuiDiagnostic> {
        self.apply_transition(StateEvent::History(event))
    }

    fn apply_transition(&mut self, event: StateEvent) -> Result<StateTransition, TuiDiagnostic> {
        let (axis, kind) = match &event {
            StateEvent::Lifecycle(event) => (
                StateAxis::Lifecycle,
                StateEventKind::Lifecycle(event.kind()),
            ),
            StateEvent::FocusCapture(event) => (
                StateAxis::FocusCapture,
                StateEventKind::FocusCapture(event.kind()),
            ),
            StateEvent::Selection(event) => (
                StateAxis::Selection,
                StateEventKind::Selection(event.kind()),
            ),
            StateEvent::Command(event) => (
                StateAxis::CommandPhase,
                StateEventKind::Command(event.kind()),
            ),
            StateEvent::Interaction(event) => (
                StateAxis::InteractionMode,
                StateEventKind::Interaction(event.kind()),
            ),
            StateEvent::History(event) => {
                (StateAxis::History, StateEventKind::History(event.kind()))
            }
        };
        if self.lifecycle != LifecycleState::InteractiveReady
            && !matches!(&event, StateEvent::Lifecycle(_))
        {
            return self.invalid_transition(axis, kind);
        }
        if self.focus == FocusState::FocusLost
            && !matches!(
                &event,
                StateEvent::FocusCapture(FocusCaptureEvent::FocusIn) | StateEvent::Lifecycle(_)
            )
        {
            return self.invalid_transition(axis, kind);
        }
        match event {
            StateEvent::Lifecycle(LifecycleEvent::ProbeStarted)
                if self.lifecycle == LifecycleState::HeadlessOnly =>
            {
                self.lifecycle = LifecycleState::Probing;
                self.finish_transition(kind, "probing", TransientState::Ready)
            }
            StateEvent::Lifecycle(LifecycleEvent::ProbeSucceeded)
                if self.lifecycle == LifecycleState::Probing =>
            {
                self.lifecycle = LifecycleState::InteractiveReady;
                self.focus = FocusState::Focused;
                self.capture = CaptureState::None;
                self.interaction_mode = InteractionMode::ModelessReady;
                self.command_phase = CommandPhase::Idle;
                self.active_command = None;
                self.command_source_revision = None;
                self.finish_transition(kind, "interactive readiness", TransientState::Ready)
            }
            StateEvent::Lifecycle(LifecycleEvent::ProbeFailed { detail })
                if self.lifecycle == LifecycleState::Probing =>
            {
                self.lifecycle = LifecycleState::HeadlessOnly;
                let diagnostic = self.operation_diagnostic(
                    TuiDiagnosticCode::LifecycleFailure,
                    axis,
                    kind,
                    detail,
                    "Probing",
                );
                self.finish_transition_with_diagnostic(
                    kind,
                    "headless-only recovery",
                    TransientState::Error,
                    Some(diagnostic),
                )
            }
            StateEvent::Lifecycle(LifecycleEvent::ResizeStarted)
                if self.lifecycle == LifecycleState::InteractiveReady =>
            {
                self.invalidate_for_resize();
                self.lifecycle = LifecycleState::Resizing;
                self.finish_transition(kind, "resizing", TransientState::ResizeRecovery)
            }
            StateEvent::Lifecycle(LifecycleEvent::ResizeCompleted)
                if self.lifecycle == LifecycleState::Resizing =>
            {
                self.lifecycle = LifecycleState::InteractiveReady;
                if self.command_phase == CommandPhase::Idle {
                    self.interaction_mode = InteractionMode::ModelessReady;
                } else if !matches!(self.command_phase, CommandPhase::Outcome { .. }) {
                    self.interaction_mode = InteractionMode::CommandModal;
                }
                self.finish_transition(kind, "resize recovery complete", TransientState::Ready)
            }
            StateEvent::Lifecycle(LifecycleEvent::ResizeFailed { detail })
                if self.lifecycle == LifecycleState::Resizing =>
            {
                self.invalidate_for_resize();
                self.lifecycle = LifecycleState::Restoring;
                let diagnostic = self.operation_diagnostic(
                    TuiDiagnosticCode::LifecycleFailure,
                    axis,
                    kind,
                    detail,
                    "Resizing",
                );
                self.finish_transition_with_diagnostic(
                    kind,
                    "resize recovery failed",
                    TransientState::Error,
                    Some(diagnostic),
                )
            }
            StateEvent::Lifecycle(LifecycleEvent::RuntimeFailure { detail })
                if matches!(
                    self.lifecycle,
                    LifecycleState::InteractiveReady | LifecycleState::Resizing
                ) =>
            {
                let from = format!("{:?}", self.lifecycle);
                self.invalidate_for_resize();
                self.lifecycle = LifecycleState::Restoring;
                let diagnostic = self.operation_diagnostic(
                    TuiDiagnosticCode::LifecycleFailure,
                    axis,
                    kind,
                    detail,
                    &from,
                );
                self.finish_transition_with_diagnostic(
                    kind,
                    "restoring after runtime failure",
                    TransientState::Warning,
                    Some(diagnostic),
                )
            }
            StateEvent::Lifecycle(LifecycleEvent::RestoreCompleted)
                if self.lifecycle == LifecycleState::Restoring =>
            {
                self.lifecycle = LifecycleState::HeadlessOnly;
                self.finish_transition(kind, "headless-only recovery", TransientState::Error)
            }
            StateEvent::Lifecycle(LifecycleEvent::CloseRequested)
                if !matches!(
                    self.lifecycle,
                    LifecycleState::Closing | LifecycleState::Closed
                ) =>
            {
                self.capture = CaptureState::None;
                self.restore_selection_after_cancel();
                self.interaction_mode = InteractionMode::ModelessReady;
                self.command_phase = CommandPhase::Idle;
                self.active_command = None;
                self.command_source_revision = None;
                if let HistoryState::Applying {
                    can_undo, can_redo, ..
                } = self.history.clone()
                {
                    self.history = HistoryState::Linear { can_undo, can_redo };
                }
                self.lifecycle = LifecycleState::Closing;
                self.finish_transition(kind, "closing", TransientState::Cancelled)
            }
            StateEvent::Lifecycle(LifecycleEvent::CleanupCompleted)
                if self.lifecycle == LifecycleState::Closing =>
            {
                self.lifecycle = LifecycleState::Closed;
                self.finish_transition(kind, "closed", TransientState::Ready)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::PointerPressed {
                tool,
                origin,
                candidate,
            }) if self.lifecycle == LifecycleState::InteractiveReady
                && self.focus == FocusState::Focused
                && self.capture == CaptureState::None
                && self.interaction_mode == InteractionMode::ModelessReady =>
            {
                let previous = self.selected_ids();
                self.capture = CaptureState::PointerCapture(PointerCapture {
                    tool,
                    origin,
                    candidate: candidate.clone(),
                });
                if let Some(candidate) = candidate {
                    self.selected_index = None;
                    self.selection = SelectionState::Candidate {
                        candidates: vec![candidate],
                        previous,
                    };
                }
                self.finish_transition(kind, "pointer candidate", TransientState::Candidate)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::PointerMoved { candidate })
                if matches!(self.capture, CaptureState::PointerCapture(_))
                    && self.focus == FocusState::Focused =>
            {
                if let CaptureState::PointerCapture(capture) = &mut self.capture {
                    capture.candidate = candidate.clone();
                }
                if let Some(candidate) = candidate {
                    self.selected_index = None;
                    let previous = self.selected_ids();
                    self.selection = SelectionState::Candidate {
                        candidates: vec![candidate],
                        previous,
                    };
                }
                self.finish_transition(kind, "pointer capture updated", TransientState::Candidate)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::DragStarted)
                if self.focus == FocusState::Focused
                    && self.interaction_mode == InteractionMode::ModelessReady
                    && matches!(self.capture, CaptureState::PointerCapture(_)) =>
            {
                let tool = match &self.capture {
                    CaptureState::PointerCapture(capture) => capture.tool,
                    CaptureState::None => unreachable!("capture was checked above"),
                };
                self.interaction_mode = InteractionMode::DragActive { tool };
                self.finish_transition(kind, "drag active", TransientState::Drag)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::DragFinished)
                if matches!(self.interaction_mode, InteractionMode::DragActive { .. })
                    && matches!(self.capture, CaptureState::PointerCapture(_)) =>
            {
                self.interaction_mode = InteractionMode::ModelessReady;
                self.capture = CaptureState::None;
                self.finish_transition(kind, "drag finished", TransientState::Ready)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::PointerReleased)
                if matches!(self.capture, CaptureState::PointerCapture(_)) =>
            {
                self.capture = CaptureState::None;
                if matches!(self.interaction_mode, InteractionMode::DragActive { .. }) {
                    self.interaction_mode = InteractionMode::ModelessReady;
                }
                self.finish_transition(kind, "pointer release acknowledged", TransientState::Ready)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::FocusLost)
                if self.focus == FocusState::Focused =>
            {
                self.focus = FocusState::FocusLost;
                self.capture = CaptureState::None;
                self.restore_selection_after_cancel();
                self.invalidate_preview_for_focus_loss();
                self.cancel_history_application();
                if matches!(self.interaction_mode, InteractionMode::DragActive { .. }) {
                    self.interaction_mode = InteractionMode::ModelessReady;
                }
                self.finish_transition(
                    kind,
                    "focus lost; capture cancelled",
                    TransientState::FocusRecovery,
                )
            }
            StateEvent::FocusCapture(FocusCaptureEvent::FocusIn)
                if self.focus == FocusState::FocusLost
                    && self.lifecycle == LifecycleState::InteractiveReady =>
            {
                self.focus = FocusState::Focused;
                self.capture = CaptureState::None;
                self.interaction_mode = InteractionMode::RecoveryReady;
                self.finish_transition(kind, "focus recovery ready", TransientState::FocusRecovery)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::RecoveryCompleted)
                if self.interaction_mode == InteractionMode::RecoveryReady
                    && self.focus == FocusState::Focused =>
            {
                self.interaction_mode = if self.command_phase == CommandPhase::Idle {
                    InteractionMode::ModelessReady
                } else {
                    InteractionMode::CommandModal
                };
                self.finish_transition(kind, "focus recovery complete", TransientState::Ready)
            }
            StateEvent::FocusCapture(FocusCaptureEvent::CaptureCancelled)
                if matches!(self.capture, CaptureState::PointerCapture(_))
                    || matches!(self.interaction_mode, InteractionMode::DragActive { .. }) =>
            {
                self.capture = CaptureState::None;
                self.interaction_mode = InteractionMode::ModelessReady;
                self.restore_selection_after_cancel();
                self.finish_transition(kind, "capture cancelled", TransientState::Cancelled)
            }
            StateEvent::Selection(SelectionEvent::Nominate { candidates })
                if self.lifecycle == LifecycleState::InteractiveReady
                    && self.focus == FocusState::Focused
                    && !candidates.is_empty() =>
            {
                let previous = self.selected_ids();
                self.selected_index = None;
                self.selection = SelectionState::Candidate {
                    candidates,
                    previous,
                };
                self.finish_transition(kind, "selection candidate", TransientState::Candidate)
            }
            StateEvent::Selection(SelectionEvent::Verify(SelectionVerification::Exact {
                stable_ids,
            })) if matches!(self.selection, SelectionState::Candidate { .. }) => {
                let valid = !stable_ids.is_empty()
                    && stable_ids_are_distinct(&stable_ids)
                    && stable_ids.iter().all(|stable_id| {
                        matches!(
                            &self.selection,
                            SelectionState::Candidate { candidates, .. }
                                if candidates.contains(stable_id)
                        ) && self.targets.iter().any(|target| target.id == *stable_id)
                    });
                if !valid {
                    Err(self.operation_diagnostic(
                        TuiDiagnosticCode::SelectionIncompatible,
                        axis,
                        kind,
                        "verified selection is not an authoritative candidate".to_string(),
                        "Candidate",
                    ))
                } else {
                    self.selected_index = stable_ids.first().and_then(|stable_id| {
                        self.targets
                            .iter()
                            .position(|target| target.id == *stable_id)
                    });
                    self.selection = SelectionState::Selected { stable_ids };
                    self.finish_transition(kind, "selection verified", TransientState::Selected)
                }
            }
            StateEvent::Selection(SelectionEvent::Verify(SelectionVerification::Ambiguous {
                stable_ids,
            })) if matches!(self.selection, SelectionState::Candidate { .. }) => {
                let valid = stable_ids.len() > 1
                    && stable_ids_are_distinct(&stable_ids)
                    && stable_ids.iter().all(|stable_id| {
                        matches!(
                            &self.selection,
                            SelectionState::Candidate { candidates, .. }
                                if candidates.contains(stable_id)
                        ) && self.targets.iter().any(|target| target.id == *stable_id)
                    });
                if !valid {
                    Err(self.operation_diagnostic(
                        TuiDiagnosticCode::SelectionIncompatible,
                        axis,
                        kind,
                        "ambiguous selection contains an unauthoritative id".to_string(),
                        "Candidate",
                    ))
                } else {
                    let previous = self.selected_ids();
                    self.selected_index = None;
                    self.selection = SelectionState::Candidate {
                        candidates: stable_ids,
                        previous,
                    };
                    let diagnostic = TuiDiagnostic {
                        code: TuiDiagnosticCode::AmbiguousSelection,
                        detail: "authoritative selection has multiple candidates".to_string(),
                        canonical_revision: self.canonical_revision.clone(),
                        axis: Some(axis),
                        event: Some(kind),
                        from: Some("Candidate".to_string()),
                    };
                    self.finish_transition_with_diagnostic(
                        kind,
                        "selection remains pending",
                        TransientState::Warning,
                        Some(diagnostic),
                    )
                }
            }
            StateEvent::Selection(SelectionEvent::Verify(SelectionVerification::Lost))
                if matches!(self.selection, SelectionState::Candidate { .. }) =>
            {
                self.selected_index = None;
                self.selection = SelectionState::None;
                let diagnostic = self.operation_diagnostic(
                    TuiDiagnosticCode::SelectionLost,
                    axis,
                    kind,
                    "authoritative selection reference was lost".to_string(),
                    "Candidate",
                );
                self.finish_transition_with_diagnostic(
                    kind,
                    "selection reference lost",
                    TransientState::Error,
                    Some(diagnostic),
                )
            }
            StateEvent::Selection(SelectionEvent::Verify(SelectionVerification::Incompatible))
                if matches!(self.selection, SelectionState::Candidate { .. }) =>
            {
                self.selected_index = None;
                self.selection = SelectionState::None;
                let diagnostic = self.operation_diagnostic(
                    TuiDiagnosticCode::SelectionIncompatible,
                    axis,
                    kind,
                    "authoritative selection reference is incompatible".to_string(),
                    "Candidate",
                );
                self.finish_transition_with_diagnostic(
                    kind,
                    "selection reference incompatible",
                    TransientState::Error,
                    Some(diagnostic),
                )
            }
            StateEvent::Selection(SelectionEvent::Clear)
                if !matches!(self.selection, SelectionState::None) =>
            {
                self.selected_index = None;
                self.selection = SelectionState::None;
                self.finish_transition(kind, "selection cleared", TransientState::Ready)
            }
            StateEvent::Interaction(InteractionEvent::OpenCommand { command }) => {
                self.open_command(command, axis, kind)
            }
            StateEvent::Interaction(InteractionEvent::StartDrag { tool })
                if self.lifecycle == LifecycleState::InteractiveReady
                    && self.focus == FocusState::Focused
                    && self.interaction_mode == InteractionMode::ModelessReady
                    && matches!(&self.capture, CaptureState::PointerCapture(capture) if capture.tool == tool) =>
            {
                self.interaction_mode = InteractionMode::DragActive { tool };
                self.finish_transition(kind, "drag active", TransientState::Drag)
            }
            StateEvent::Interaction(InteractionEvent::FinishDrag)
                if matches!(self.interaction_mode, InteractionMode::DragActive { .. })
                    && matches!(self.capture, CaptureState::PointerCapture(_)) =>
            {
                self.interaction_mode = InteractionMode::ModelessReady;
                self.capture = CaptureState::None;
                self.finish_transition(kind, "drag finished", TransientState::Ready)
            }
            StateEvent::Interaction(InteractionEvent::RecoveryCompleted)
                if self.interaction_mode == InteractionMode::RecoveryReady
                    && self.focus == FocusState::Focused =>
            {
                self.interaction_mode = if self.command_phase == CommandPhase::Idle {
                    InteractionMode::ModelessReady
                } else {
                    InteractionMode::CommandModal
                };
                self.finish_transition(kind, "focus recovery complete", TransientState::Ready)
            }
            StateEvent::Interaction(InteractionEvent::CloseCommand)
                if matches!(self.command_phase, CommandPhase::Outcome { .. })
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                self.command_phase = CommandPhase::Idle;
                self.active_command = None;
                self.command_source_revision = None;
                self.interaction_mode = InteractionMode::ModelessReady;
                self.finish_transition(kind, "command outcome dismissed", TransientState::Ready)
            }
            StateEvent::Interaction(InteractionEvent::StartHistory { direction }) => {
                let available = match &direction {
                    HistoryDirection::Undo => {
                        matches!(self.history, HistoryState::Linear { can_undo: true, .. })
                    }
                    HistoryDirection::Redo => {
                        matches!(self.history, HistoryState::Linear { can_redo: true, .. })
                    }
                    HistoryDirection::NamedRevision { name } => {
                        self.recoverable_revisions.contains(name)
                    }
                };
                if available
                    && matches!(self.history, HistoryState::Linear { .. })
                    && self.interaction_mode == InteractionMode::ModelessReady
                    && self.command_phase == CommandPhase::Idle
                    && self.focus == FocusState::Focused
                {
                    let (can_undo, can_redo) = self.history_availability();
                    self.history = HistoryState::Applying {
                        direction,
                        can_undo,
                        can_redo,
                    };
                    self.interaction_mode = InteractionMode::HistoryApplying;
                    self.finish_transition(
                        kind,
                        "history application started",
                        TransientState::Ready,
                    )
                } else {
                    self.invalid_transition(axis, kind)
                }
            }
            StateEvent::Command(CommandEvent::Open { command }) => {
                self.open_command(command, axis, kind)
            }
            StateEvent::Command(CommandEvent::DraftUpdated { input_fingerprint })
                if matches!(self.command_phase, CommandPhase::Draft { .. })
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                let command = match &self.command_phase {
                    CommandPhase::Draft { command, .. } => command.clone(),
                    _ => unreachable!("draft phase was checked above"),
                };
                self.command_phase = CommandPhase::Draft {
                    command,
                    input_fingerprint,
                };
                self.finish_transition(kind, "command draft updated", TransientState::Ready)
            }
            StateEvent::Command(CommandEvent::PreviewRequested)
                if matches!(self.command_phase, CommandPhase::Draft { .. })
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                let input_fingerprint = match &self.command_phase {
                    CommandPhase::Draft {
                        input_fingerprint, ..
                    } => input_fingerprint.clone(),
                    _ => unreachable!("draft phase was checked above"),
                };
                self.command_phase = CommandPhase::Previewing { input_fingerprint };
                self.finish_transition(kind, "command previewing", TransientState::Drag)
            }
            StateEvent::Command(CommandEvent::PreviewCompleted(PreviewResult::Ready))
                if let CommandPhase::Previewing { input_fingerprint } = &self.command_phase
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                self.command_phase = CommandPhase::PreviewReady {
                    input_fingerprint: input_fingerprint.clone(),
                };
                self.finish_transition(kind, "command preview ready", TransientState::Ready)
            }
            StateEvent::Command(CommandEvent::PreviewCompleted(PreviewResult::Rejected {
                detail,
            })) if let CommandPhase::Previewing { input_fingerprint } = &self.command_phase
                && self.interaction_mode == InteractionMode::CommandModal =>
            {
                self.command_phase = CommandPhase::Draft {
                    command: self
                        .active_command
                        .clone()
                        .expect("preview has an active command"),
                    input_fingerprint: input_fingerprint.clone(),
                };
                let diagnostic = TuiDiagnostic {
                    code: TuiDiagnosticCode::CommandRejected,
                    detail,
                    canonical_revision: self.canonical_revision.clone(),
                    axis: Some(axis),
                    event: Some(kind),
                    from: Some("Previewing".to_string()),
                };
                self.finish_transition_with_diagnostic(
                    kind,
                    "command preview rejected",
                    TransientState::Warning,
                    Some(diagnostic),
                )
            }
            StateEvent::Command(CommandEvent::CommitRequested)
                if matches!(self.command_phase, CommandPhase::PreviewReady { .. })
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                let input_fingerprint = match &self.command_phase {
                    CommandPhase::PreviewReady { input_fingerprint } => input_fingerprint.clone(),
                    _ => unreachable!("preview-ready phase was checked above"),
                };
                self.command_phase = CommandPhase::Committing { input_fingerprint };
                self.finish_transition(kind, "command committing", TransientState::Drag)
            }
            StateEvent::Command(CommandEvent::CommitAccepted {
                source_revision,
                validated_revision,
                revision,
            }) if matches!(self.command_phase, CommandPhase::Committing { .. })
                && self.interaction_mode == InteractionMode::CommandModal
                && !revision.is_empty() =>
            {
                if self.command_source_revision.as_deref() != Some(source_revision.as_str())
                    || validated_revision != source_revision
                {
                    Err(self.operation_diagnostic(
                        TuiDiagnosticCode::StalePreview,
                        axis,
                        kind,
                        format!(
                            "commit source revision {source_revision} does not match preview revision"
                        ),
                        "Committing",
                    ))
                } else {
                    self.canonical_revision = revision.clone();
                    self.history = HistoryState::Linear {
                        can_undo: true,
                        can_redo: false,
                    };
                    self.command_phase = CommandPhase::Outcome {
                        outcome: CommandOutcome::Committed { revision },
                    };
                    self.finish_transition(kind, "command committed", TransientState::Selected)
                }
            }
            StateEvent::Command(CommandEvent::CommitRejected { detail })
                if matches!(self.command_phase, CommandPhase::Committing { .. })
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                self.command_phase = CommandPhase::Outcome {
                    outcome: CommandOutcome::Rejected {
                        detail: detail.clone(),
                    },
                };
                let diagnostic = self.operation_diagnostic(
                    TuiDiagnosticCode::CommandRejected,
                    axis,
                    kind,
                    detail,
                    "Committing",
                );
                self.finish_transition_with_diagnostic(
                    kind,
                    "command rejected",
                    TransientState::Error,
                    Some(diagnostic),
                )
            }
            StateEvent::Command(CommandEvent::CancelRequested)
                if matches!(
                    self.command_phase,
                    CommandPhase::Draft { .. }
                        | CommandPhase::Previewing { .. }
                        | CommandPhase::PreviewReady { .. }
                        | CommandPhase::Committing { .. }
                ) && self.interaction_mode == InteractionMode::CommandModal =>
            {
                self.command_phase = CommandPhase::Cancelling;
                self.finish_transition(kind, "command cancelling", TransientState::Cancelled)
            }
            StateEvent::Command(CommandEvent::CancellationCompleted { detail })
                if self.command_phase == CommandPhase::Cancelling
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                self.command_phase = CommandPhase::Outcome {
                    outcome: CommandOutcome::Cancelled {
                        detail: detail.clone(),
                    },
                };
                let diagnostic = self.operation_diagnostic(
                    TuiDiagnosticCode::CommandCancelled,
                    axis,
                    kind,
                    detail,
                    "Cancelling",
                );
                self.finish_transition_with_diagnostic(
                    kind,
                    "command cancelled",
                    TransientState::Cancelled,
                    Some(diagnostic),
                )
            }
            StateEvent::Command(CommandEvent::OutcomeDismissed)
                if matches!(self.command_phase, CommandPhase::Outcome { .. })
                    && self.interaction_mode == InteractionMode::CommandModal =>
            {
                self.command_phase = CommandPhase::Idle;
                self.active_command = None;
                self.command_source_revision = None;
                self.interaction_mode = InteractionMode::ModelessReady;
                self.finish_transition(kind, "command outcome dismissed", TransientState::Ready)
            }
            StateEvent::History(HistoryEvent::UndoRequested)
                if matches!(self.history, HistoryState::Linear { can_undo: true, .. })
                    && self.interaction_mode == InteractionMode::ModelessReady
                    && self.command_phase == CommandPhase::Idle
                    && self.focus == FocusState::Focused =>
            {
                let (can_undo, can_redo) = self.history_availability();
                self.history = HistoryState::Applying {
                    direction: HistoryDirection::Undo,
                    can_undo,
                    can_redo,
                };
                self.interaction_mode = InteractionMode::HistoryApplying;
                self.finish_transition(kind, "history undo applying", TransientState::Ready)
            }
            StateEvent::History(HistoryEvent::RedoRequested)
                if matches!(self.history, HistoryState::Linear { can_redo: true, .. })
                    && self.interaction_mode == InteractionMode::ModelessReady
                    && self.command_phase == CommandPhase::Idle
                    && self.focus == FocusState::Focused =>
            {
                let (can_undo, can_redo) = self.history_availability();
                self.history = HistoryState::Applying {
                    direction: HistoryDirection::Redo,
                    can_undo,
                    can_redo,
                };
                self.interaction_mode = InteractionMode::HistoryApplying;
                self.finish_transition(kind, "history redo applying", TransientState::Ready)
            }
            StateEvent::History(HistoryEvent::RestoreNamedRevision { name })
                if self.recoverable_revisions.contains(&name)
                    && matches!(self.history, HistoryState::Linear { .. })
                    && self.interaction_mode == InteractionMode::ModelessReady
                    && self.command_phase == CommandPhase::Idle
                    && self.focus == FocusState::Focused =>
            {
                let (can_undo, can_redo) = self.history_availability();
                self.history = HistoryState::Applying {
                    direction: HistoryDirection::NamedRevision { name },
                    can_undo,
                    can_redo,
                };
                self.interaction_mode = InteractionMode::HistoryApplying;
                self.finish_transition(kind, "named revision applying", TransientState::Ready)
            }
            StateEvent::History(HistoryEvent::ApplyCompleted(HistoryApplyResult::Applied {
                revision,
                can_undo,
                can_redo,
            })) if matches!(self.history, HistoryState::Applying { .. })
                && !revision.is_empty() =>
            {
                self.canonical_revision = revision;
                self.history = HistoryState::Linear { can_undo, can_redo };
                self.interaction_mode = InteractionMode::ModelessReady;
                self.finish_transition(kind, "history application complete", TransientState::Ready)
            }
            StateEvent::History(HistoryEvent::ApplyCompleted(HistoryApplyResult::Rejected {
                detail,
            })) if matches!(self.history, HistoryState::Applying { .. }) => {
                let (can_undo, can_redo) = self.history_availability();
                self.history = HistoryState::Linear { can_undo, can_redo };
                self.interaction_mode = InteractionMode::ModelessReady;
                let diagnostic = TuiDiagnostic {
                    code: TuiDiagnosticCode::HistoryRejected,
                    detail,
                    canonical_revision: self.canonical_revision.clone(),
                    axis: Some(axis),
                    event: Some(kind),
                    from: Some("Applying".to_string()),
                };
                self.finish_transition_with_diagnostic(
                    kind,
                    "history application rejected",
                    TransientState::Error,
                    Some(diagnostic),
                )
            }
            StateEvent::History(HistoryEvent::DivergentCommit {
                revision,
                preserved_named_revision,
            }) if matches!(self.history, HistoryState::Linear { can_redo: true, .. })
                && self.interaction_mode == InteractionMode::ModelessReady
                && self.command_phase == CommandPhase::Idle
                && self.focus == FocusState::Focused
                && !revision.is_empty()
                && !preserved_named_revision.is_empty() =>
            {
                self.canonical_revision = revision;
                if !self
                    .recoverable_revisions
                    .contains(&preserved_named_revision)
                {
                    self.recoverable_revisions.push(preserved_named_revision);
                }
                self.history = HistoryState::Linear {
                    can_undo: true,
                    can_redo: false,
                };
                self.finish_transition(kind, "named revision preserved", TransientState::Selected)
            }
            _ => self.invalid_transition(axis, kind),
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
                axis: None,
                event: None,
                from: None,
            };
            return self.finish(acknowledgement, Some(diagnostic));
        }

        if let Err(diagnostic) = self.guard_navigation() {
            let acknowledgement = self.acknowledgement(
                sequence,
                key,
                NavigationResult::NoFeatureTarget,
                &format!("Acknowledgement {sequence}: arrow navigation rejected"),
                TransientState::Error,
            );
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
        let stable_id = self.targets[index].id.clone();
        if let Err(diagnostic) = self
            .transition_selection(SelectionEvent::Nominate {
                candidates: vec![stable_id.clone()],
            })
            .and_then(|_| {
                self.transition_selection(SelectionEvent::Verify(SelectionVerification::Exact {
                    stable_ids: vec![stable_id.clone()],
                }))
            })
        {
            let acknowledgement = self.acknowledgement(
                sequence,
                key,
                NavigationResult::NoFeatureTarget,
                &format!("Acknowledgement {sequence}: arrow navigation rejected"),
                TransientState::Error,
            );
            return self.finish(acknowledgement, Some(diagnostic));
        }
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

    fn guard_navigation(&self) -> Result<(), TuiDiagnostic> {
        let event = StateEventKind::Selection(SelectionEventKind::Nominate);
        if self.lifecycle != LifecycleState::InteractiveReady
            || self.focus != FocusState::Focused
            || self.capture != CaptureState::None
            || self.interaction_mode != InteractionMode::ModelessReady
            || self.command_phase != CommandPhase::Idle
            || !matches!(self.history, HistoryState::Linear { .. })
        {
            return Err(self
                .invalid_transition(StateAxis::Selection, event)
                .unwrap_err());
        }
        Ok(())
    }

    pub fn process_terminal_input(&mut self, bytes: &[u8]) -> Result<RenderedInput, TuiDiagnostic> {
        let key = decode_arrow(bytes).ok_or_else(|| TuiDiagnostic {
            code: TuiDiagnosticCode::InvalidArrowInput,
            detail: "expected one legacy terminal arrow sequence".to_string(),
            canonical_revision: self.canonical_revision.clone(),
            axis: None,
            event: None,
            from: None,
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

    fn finish_transition(
        &mut self,
        event: StateEventKind,
        text: &str,
        state: TransientState,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.finish_transition_with_diagnostic(event, text, state, None)
    }

    fn finish_transition_with_diagnostic(
        &mut self,
        event: StateEventKind,
        text: &str,
        state: TransientState,
        diagnostic: Option<TuiDiagnostic>,
    ) -> Result<StateTransition, TuiDiagnostic> {
        self.transition_sequence += 1;
        let visual = transient_visuals()
            .iter()
            .find(|visual| visual.state == state)
            .expect("theme transient state mapping is complete");
        let acknowledgement = StateAcknowledgement {
            sequence: self.transition_sequence,
            event,
            text: text.to_string(),
            marker: visual.marker.expect("theme marker is present"),
            color: visual.color,
        };
        self.last_transition_acknowledgement = Some(acknowledgement.clone());
        Ok(StateTransition {
            state: self.state(),
            acknowledgement,
            diagnostic,
        })
    }

    fn operation_diagnostic(
        &self,
        code: TuiDiagnosticCode,
        axis: StateAxis,
        event: StateEventKind,
        detail: String,
        from: &str,
    ) -> TuiDiagnostic {
        TuiDiagnostic {
            code,
            detail,
            canonical_revision: self.canonical_revision.clone(),
            axis: Some(axis),
            event: Some(event),
            from: Some(from.to_string()),
        }
    }

    fn open_command(
        &mut self,
        command: String,
        axis: StateAxis,
        event: StateEventKind,
    ) -> Result<StateTransition, TuiDiagnostic> {
        if self.focus != FocusState::Focused
            || self.interaction_mode != InteractionMode::ModelessReady
            || self.command_phase != CommandPhase::Idle
            || command.is_empty()
        {
            return self.invalid_transition(axis, event);
        }
        self.active_command = Some(command.clone());
        self.command_source_revision = Some(self.canonical_revision.clone());
        self.interaction_mode = InteractionMode::CommandModal;
        self.command_phase = CommandPhase::Draft {
            command,
            input_fingerprint: String::new(),
        };
        self.finish_transition(event, "command draft open", TransientState::Ready)
    }

    fn invalid_transition(
        &self,
        axis: StateAxis,
        event: StateEventKind,
    ) -> Result<StateTransition, TuiDiagnostic> {
        let from = match axis {
            StateAxis::Lifecycle => format!("lifecycle={:?}", self.lifecycle),
            StateAxis::FocusCapture => {
                format!("focus={:?};capture={:?}", self.focus, self.capture)
            }
            StateAxis::Selection => format!("selection={:?}", self.selection),
            StateAxis::InteractionMode => {
                format!("interaction={:?}", self.interaction_mode)
            }
            StateAxis::CommandPhase => format!("command={:?}", self.command_phase),
            StateAxis::History => format!("history={:?}", self.history),
        };
        Err(TuiDiagnostic {
            code: TuiDiagnosticCode::InvalidTransition,
            detail: format!("{event:?} is not valid from {from}"),
            canonical_revision: self.canonical_revision.clone(),
            axis: Some(axis),
            event: Some(event),
            from: Some(from),
        })
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
        match &self.selection {
            SelectionState::Selected { stable_ids } => stable_ids.first().map(String::as_str),
            SelectionState::None | SelectionState::Candidate { .. } => None,
        }
    }

    fn selected_ids(&self) -> Option<Vec<String>> {
        match &self.selection {
            SelectionState::Selected { stable_ids } => Some(stable_ids.clone()),
            SelectionState::Candidate { previous, .. } => previous.clone(),
            SelectionState::None => None,
        }
    }

    fn restore_selection_after_cancel(&mut self) {
        let selection = std::mem::replace(&mut self.selection, SelectionState::None);
        if let SelectionState::Candidate { previous, .. } = selection {
            if let Some(stable_ids) = previous {
                self.selected_index = stable_ids.first().and_then(|stable_id| {
                    self.targets
                        .iter()
                        .position(|target| target.id == *stable_id)
                });
                self.selection = SelectionState::Selected { stable_ids };
            }
        } else {
            self.selection = selection;
        }
    }

    fn history_availability(&self) -> (bool, bool) {
        match &self.history {
            HistoryState::Linear { can_undo, can_redo }
            | HistoryState::Applying {
                can_undo, can_redo, ..
            } => (*can_undo, *can_redo),
        }
    }

    fn invalidate_for_resize(&mut self) {
        self.presentation_generation += 1;
        self.capture = CaptureState::None;
        self.restore_selection_after_cancel();
        self.cancel_history_application();
        if matches!(self.interaction_mode, InteractionMode::DragActive { .. }) {
            self.interaction_mode = InteractionMode::ModelessReady;
        }
        let preview_fingerprint = match &self.command_phase {
            CommandPhase::Previewing { input_fingerprint }
            | CommandPhase::PreviewReady { input_fingerprint } => Some(input_fingerprint.clone()),
            _ => None,
        };
        if let Some(input_fingerprint) = preview_fingerprint {
            self.command_phase = CommandPhase::Draft {
                command: self
                    .active_command
                    .clone()
                    .expect("a preview has an active command"),
                input_fingerprint,
            };
            self.interaction_mode = InteractionMode::CommandModal;
        }
    }

    fn invalidate_preview_for_focus_loss(&mut self) {
        let preview_fingerprint = match &self.command_phase {
            CommandPhase::Previewing { input_fingerprint }
            | CommandPhase::PreviewReady { input_fingerprint } => Some(input_fingerprint.clone()),
            _ => None,
        };
        if let Some(input_fingerprint) = preview_fingerprint {
            self.command_phase = CommandPhase::Draft {
                command: self
                    .active_command
                    .clone()
                    .expect("a preview has an active command"),
                input_fingerprint,
            };
        }
    }

    fn cancel_history_application(&mut self) {
        if let HistoryState::Applying {
            can_undo, can_redo, ..
        } = self.history.clone()
        {
            self.history = HistoryState::Linear { can_undo, can_redo };
            if self.interaction_mode == InteractionMode::HistoryApplying {
                self.interaction_mode = InteractionMode::ModelessReady;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateTransition {
    pub state: TuiState,
    pub acknowledgement: StateAcknowledgement,
    pub diagnostic: Option<TuiDiagnostic>,
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

fn stable_ids_are_distinct(stable_ids: &[String]) -> bool {
    stable_ids
        .iter()
        .enumerate()
        .all(|(index, stable_id)| stable_ids[..index].iter().all(|prior| prior != stable_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.tui/1");
    }
}

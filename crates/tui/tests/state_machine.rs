use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_theme::NonColorMarker;
use threeterm_tui::{
    CaptureState, CommandEvent, CommandOutcome, CommandPhase, FocusCaptureEvent, FocusState,
    HistoryApplyResult, HistoryDirection, HistoryEvent, HistoryState, InteractionEvent,
    InteractionMode, InteractionTool, LifecycleEvent, LifecycleState, PointerOrigin, PreviewResult,
    SelectionEvent, SelectionState, SelectionVerification, StateAxis, StateEvent,
    TuiDiagnosticCode, TuiSession,
};

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-tui-state-machine-{nanos}"))
}

#[test]
fn state_transition_uses_the_host_projection_without_mutating_canonical_state() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("host has a canonical snapshot");
    let graph = host
        .current_graph()
        .expect("host exposes its read-only canonical graph");
    let mut session = TuiSession::from_feature_graph(&graph, &before.revision_hash);

    let mut probing_session = TuiSession::from_feature_graph_probing(&graph, &before.revision_hash);
    assert_eq!(probing_session.state().lifecycle, LifecycleState::Probing);
    let probe_failure = probing_session
        .transition_lifecycle(LifecycleEvent::ProbeFailed {
            detail: "graphics capability unavailable".to_string(),
        })
        .expect("failed capability gate enters headless-only mode");
    assert_eq!(probe_failure.state.lifecycle, LifecycleState::HeadlessOnly);
    assert_eq!(
        probe_failure
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::LifecycleFailure)
    );
    probing_session
        .transition_lifecycle(LifecycleEvent::ProbeStarted)
        .expect("headless mode retries through a fresh probe");
    probing_session
        .transition_lifecycle(LifecycleEvent::ProbeSucceeded)
        .expect("the production projection can become interactive after probing");
    assert_eq!(host.current(), Some(before.clone()));

    let transition = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ResizeStarted))
        .expect("resize starts from interactive readiness");
    assert_eq!(transition.state.lifecycle, LifecycleState::Resizing);
    assert_eq!(
        transition.acknowledgement.marker,
        NonColorMarker::ResizeRecoveryGlyph
    );
    assert!(transition.diagnostic.is_none());
    assert_eq!(host.current(), Some(before.clone()));

    let invalid = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ProbeSucceeded))
        .expect_err("probing completion is invalid while resizing");
    assert_eq!(invalid.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(invalid.axis, Some(StateAxis::Lifecycle));
    assert_eq!(invalid.canonical_revision, before.revision_hash);
    assert_eq!(session.state().lifecycle, LifecycleState::Resizing);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn lifecycle_handlers_cover_probe_recovery_resize_and_close() {
    let mut session = TuiSession::new_probing([], "revision-lifecycle");
    assert_eq!(session.state().lifecycle, LifecycleState::Probing);

    let ready = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ProbeSucceeded))
        .expect("successful probe enters interactive readiness");
    assert_eq!(ready.state.lifecycle, LifecycleState::InteractiveReady);

    let restoring = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::RuntimeFailure {
            detail: "image lost".to_string(),
        }))
        .expect("runtime failure enters restoration");
    assert_eq!(restoring.state.lifecycle, LifecycleState::Restoring);

    let headless = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::RestoreCompleted))
        .expect("bounded restoration enters headless mode");
    assert_eq!(headless.state.lifecycle, LifecycleState::HeadlessOnly);

    session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ProbeStarted))
        .expect("headless mode requires a fresh probe");
    let failed = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ProbeFailed {
            detail: "capability missing".to_string(),
        }))
        .expect("failed probe remains headless");
    assert_eq!(failed.state.lifecycle, LifecycleState::HeadlessOnly);
    assert_eq!(
        failed.diagnostic.as_ref().map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::LifecycleFailure)
    );

    let invalid_resize = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ResizeStarted))
        .expect_err("headless mode cannot resize an interactive presentation");
    assert_eq!(invalid_resize.code, TuiDiagnosticCode::InvalidTransition);

    session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ProbeStarted))
        .expect("probe can be retried");
    session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ProbeSucceeded))
        .expect("retry can restore interactive readiness");
    session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ResizeStarted))
        .expect("resize invalidates the old presentation");
    let resized = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::ResizeCompleted))
        .expect("resize rebuild completes");
    assert_eq!(resized.state.lifecycle, LifecycleState::InteractiveReady);

    session
        .transition_lifecycle(LifecycleEvent::ResizeStarted)
        .expect("a second resize can fail explicitly");
    let resize_failed = session
        .transition_lifecycle(LifecycleEvent::ResizeFailed {
            detail: "layout rebuild failed".to_string(),
        })
        .expect("resize failure enters restoration");
    assert_eq!(resize_failed.state.lifecycle, LifecycleState::Restoring);
    assert_eq!(
        resize_failed
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::LifecycleFailure)
    );

    let mut runtime_resize = TuiSession::new([], "revision-runtime-resize");
    runtime_resize
        .transition_lifecycle(LifecycleEvent::ResizeStarted)
        .expect("runtime failure test enters resize");
    let runtime_failed = runtime_resize
        .transition_lifecycle(LifecycleEvent::RuntimeFailure {
            detail: "terminal write failed".to_string(),
        })
        .expect("runtime failure during resize enters restoration");
    assert_eq!(
        runtime_failed
            .diagnostic
            .as_ref()
            .and_then(|diagnostic| diagnostic.from.as_deref()),
        Some("Resizing")
    );
    session
        .transition_lifecycle(LifecycleEvent::RestoreCompleted)
        .expect("failed resize cleanup reaches headless mode");

    let mut closing_session = TuiSession::new([], "revision-closing");
    closing_session
        .transition_focus_capture(FocusCaptureEvent::PointerPressed {
            tool: InteractionTool::Orbit,
            origin: PointerOrigin { column: 5, row: 5 },
            candidate: None,
        })
        .expect("close test starts with active capture");
    let closing = closing_session
        .transition_lifecycle(LifecycleEvent::CloseRequested)
        .expect("close cancels active transient input");
    assert_eq!(closing.state.capture, CaptureState::None);
    assert_eq!(closing.state.command_phase, CommandPhase::Idle);
    closing_session
        .transition_lifecycle(LifecycleEvent::CleanupCompleted)
        .expect("close test cleanup completes");
    let closed_invalid = closing_session
        .transition_focus_capture(FocusCaptureEvent::FocusIn)
        .expect_err("closed sessions reject interactive events");
    assert_eq!(closed_invalid.code, TuiDiagnosticCode::InvalidTransition);

    session
        .transition(StateEvent::Lifecycle(LifecycleEvent::CloseRequested))
        .expect("close begins bounded cleanup");
    let closed = session
        .transition(StateEvent::Lifecycle(LifecycleEvent::CleanupCompleted))
        .expect("cleanup reaches the terminal state");
    assert_eq!(closed.state.lifecycle, LifecycleState::Closed);
    assert!(
        session
            .transition(StateEvent::Lifecycle(LifecycleEvent::ProbeStarted))
            .is_err()
    );
}

#[test]
fn resize_invalidates_capture_and_preview_but_preserves_the_open_draft() {
    let mut capture_session = TuiSession::new(
        [threeterm_tui::FeatureTarget::new("feature-a", "box")],
        "revision-resize-capture",
    );
    capture_session
        .transition_focus_capture(FocusCaptureEvent::PointerPressed {
            tool: InteractionTool::Selection,
            origin: PointerOrigin { column: 3, row: 4 },
            candidate: Some("feature-a".to_string()),
        })
        .expect("capture starts before resize");
    let invalidated = capture_session
        .transition_lifecycle(LifecycleEvent::ResizeStarted)
        .expect("resize cancels capture");
    assert_eq!(invalidated.state.capture, CaptureState::None);
    assert_eq!(invalidated.state.lifecycle, LifecycleState::Resizing);

    let mut command_session = TuiSession::new([], "revision-resize-preview");
    command_session
        .transition_command(CommandEvent::Open {
            command: "extrude".to_string(),
        })
        .expect("command opens");
    command_session
        .transition_command(CommandEvent::PreviewRequested)
        .expect("preview starts");
    command_session
        .transition_lifecycle(LifecycleEvent::ResizeStarted)
        .expect("resize invalidates the preview presentation");
    assert!(matches!(
        command_session.state().command_phase,
        CommandPhase::Draft { .. }
    ));
    assert_eq!(
        command_session.state().interaction_mode,
        InteractionMode::CommandModal
    );
    command_session
        .transition_lifecycle(LifecycleEvent::ResizeCompleted)
        .expect("resize completes without closing the draft");
    assert!(matches!(
        command_session.state().command_phase,
        CommandPhase::Draft { .. }
    ));
}

#[test]
fn focus_capture_and_selection_handlers_cancel_transient_input_safely() {
    let mut session = TuiSession::new(
        [
            threeterm_tui::FeatureTarget::new("feature-a", "box"),
            threeterm_tui::FeatureTarget::new("feature-b", "fillet"),
        ],
        "revision-focus",
    );

    let nominated = session
        .transition(StateEvent::Selection(SelectionEvent::Nominate {
            candidates: vec!["feature-a".to_string()],
        }))
        .expect("a target can become a pending candidate");
    assert!(matches!(
        nominated.state.selection,
        SelectionState::Candidate { .. }
    ));

    let selected = session
        .transition(StateEvent::Selection(SelectionEvent::Verify(
            SelectionVerification::Exact {
                stable_ids: vec!["feature-a".to_string()],
            },
        )))
        .expect("an exact candidate becomes selected");
    assert_eq!(
        selected.state.selection,
        SelectionState::Selected {
            stable_ids: vec!["feature-a".to_string()]
        }
    );

    let pressed = session
        .transition(StateEvent::FocusCapture(
            FocusCaptureEvent::PointerPressed {
                tool: InteractionTool::Selection,
                origin: PointerOrigin { column: 4, row: 2 },
                candidate: Some("feature-a".to_string()),
            },
        ))
        .expect("pointer press creates explicit capture");
    assert!(matches!(
        pressed.state.capture,
        CaptureState::PointerCapture(..)
    ));
    assert_eq!(pressed.state.focus, FocusState::Focused);
    assert_eq!(
        pressed.state.interaction_mode,
        InteractionMode::ModelessReady
    );

    session
        .transition(StateEvent::FocusCapture(FocusCaptureEvent::DragStarted))
        .expect("capture can become an active drag");
    let lost = session
        .transition(StateEvent::FocusCapture(FocusCaptureEvent::FocusLost))
        .expect("focus loss cancels active capture");
    assert_eq!(lost.state.focus, FocusState::FocusLost);
    assert_eq!(lost.state.capture, CaptureState::None);
    assert_eq!(lost.state.interaction_mode, InteractionMode::ModelessReady);
    assert_eq!(
        lost.state.selection,
        SelectionState::Selected {
            stable_ids: vec!["feature-a".to_string()]
        }
    );

    let recovery = session
        .transition(StateEvent::FocusCapture(FocusCaptureEvent::FocusIn))
        .expect("focus in enters recovery readiness");
    assert_eq!(recovery.state.focus, FocusState::Focused);
    assert_eq!(
        recovery.state.interaction_mode,
        InteractionMode::RecoveryReady
    );
    let ready = session
        .transition(StateEvent::FocusCapture(
            FocusCaptureEvent::RecoveryCompleted,
        ))
        .expect("recovery requires explicit readiness acknowledgement");
    assert_eq!(ready.state.interaction_mode, InteractionMode::ModelessReady);

    session
        .transition(StateEvent::FocusCapture(
            FocusCaptureEvent::PointerPressed {
                tool: InteractionTool::Selection,
                origin: PointerOrigin { column: 7, row: 3 },
                candidate: Some("feature-a".to_string()),
            },
        ))
        .expect("a fresh press is required after recovery");
    let cancelled = session
        .transition(StateEvent::FocusCapture(
            FocusCaptureEvent::CaptureCancelled,
        ))
        .expect("explicit cancellation clears pending capture");
    assert_eq!(cancelled.state.capture, CaptureState::None);
    assert_eq!(
        cancelled
            .state
            .last_transition_acknowledgement
            .unwrap()
            .marker,
        NonColorMarker::CancellationGlyph
    );

    let mut pointer_hints = TuiSession::new([], "revision-pointer-hints");
    pointer_hints
        .transition_focus_capture(FocusCaptureEvent::PointerPressed {
            tool: InteractionTool::Selection,
            origin: PointerOrigin { column: 8, row: 8 },
            candidate: Some("feature-a".to_string()),
        })
        .expect("pointer press creates a candidate");
    pointer_hints
        .transition_focus_capture(FocusCaptureEvent::PointerMoved {
            candidate: Some("feature-b".to_string()),
        })
        .expect("motion updates only the transient candidate");
    let released = pointer_hints
        .transition_focus_capture(FocusCaptureEvent::PointerReleased)
        .expect("release is only a finish hint");
    assert_eq!(released.state.capture, CaptureState::None);
    assert!(matches!(
        released.state.selection,
        SelectionState::Candidate { .. }
    ));

    let ambiguous = session
        .transition(StateEvent::Selection(SelectionEvent::Nominate {
            candidates: vec!["feature-a".to_string(), "feature-b".to_string()],
        }))
        .expect("ambiguous candidates remain pending");
    assert!(ambiguous.diagnostic.is_none());
    let ambiguous = session
        .transition(StateEvent::Selection(SelectionEvent::Verify(
            SelectionVerification::Ambiguous {
                stable_ids: vec!["feature-a".to_string(), "feature-b".to_string()],
            },
        )))
        .expect("ambiguity is an explicit selection outcome");
    assert_eq!(
        ambiguous
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::AmbiguousSelection)
    );
    assert!(matches!(
        ambiguous.state.selection,
        SelectionState::Candidate { .. }
    ));
    let before_unknown_ambiguity = session.state();
    let unknown_ambiguity = session
        .transition_selection(SelectionEvent::Verify(SelectionVerification::Ambiguous {
            stable_ids: vec!["feature-a".to_string(), "unknown".to_string()],
        }))
        .expect_err("ambiguous verification cannot introduce an unknown id");
    assert_eq!(
        unknown_ambiguity.code,
        TuiDiagnosticCode::SelectionIncompatible
    );
    assert_eq!(session.state(), before_unknown_ambiguity);

    let invalid = session
        .transition(StateEvent::Selection(SelectionEvent::Verify(
            SelectionVerification::Exact {
                stable_ids: Vec::new(),
            },
        )))
        .expect_err("empty verification cannot select a target");
    assert_eq!(invalid.code, TuiDiagnosticCode::SelectionIncompatible);
    assert_eq!(invalid.axis, Some(StateAxis::Selection));

    session
        .transition_selection(SelectionEvent::Nominate {
            candidates: vec!["feature-a".to_string()],
        })
        .expect("a candidate can be retried after ambiguity");
    let before_mismatch = session.state();
    let mismatch = session
        .transition_selection(SelectionEvent::Verify(SelectionVerification::Exact {
            stable_ids: vec!["feature-b".to_string()],
        }))
        .expect_err("verification cannot select outside the authoritative candidate");
    assert_eq!(mismatch.code, TuiDiagnosticCode::SelectionIncompatible);
    assert_eq!(session.state(), before_mismatch);
    let lost = session
        .transition_selection(SelectionEvent::Verify(SelectionVerification::Lost))
        .expect("lost references become an explicit diagnostic outcome");
    assert_eq!(lost.state.selection, SelectionState::None);
    assert_eq!(
        lost.diagnostic.as_ref().map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::SelectionLost)
    );

    session
        .transition_selection(SelectionEvent::Nominate {
            candidates: vec!["feature-a".to_string()],
        })
        .expect("selection can be nominated again");
    let incompatible = session
        .transition_selection(SelectionEvent::Verify(SelectionVerification::Incompatible))
        .expect("incompatible references remain an explicit failure");
    assert_eq!(incompatible.state.selection, SelectionState::None);
    assert_eq!(
        incompatible
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::SelectionIncompatible)
    );
}

#[test]
fn interaction_and_command_handlers_enforce_one_modal_phase_graph() {
    let mut session = TuiSession::new([], "revision-command");

    let opened = session
        .transition(StateEvent::Command(CommandEvent::Open {
            command: "extrude".to_string(),
        }))
        .expect("modeless readiness opens one command modal");
    assert_eq!(opened.state.interaction_mode, InteractionMode::CommandModal);
    assert!(matches!(
        opened.state.command_phase,
        CommandPhase::Draft { .. }
    ));

    session
        .transition(StateEvent::Command(CommandEvent::DraftUpdated {
            input_fingerprint: "draft-1".to_string(),
        }))
        .expect("draft values remain transient");
    session
        .transition(StateEvent::Command(CommandEvent::PreviewRequested))
        .expect("draft can request a read-only preview");
    assert!(matches!(
        session.state().command_phase,
        CommandPhase::Previewing { .. }
    ));
    session
        .transition(StateEvent::Command(CommandEvent::PreviewCompleted(
            PreviewResult::Ready,
        )))
        .expect("preview becomes ready");
    session
        .transition(StateEvent::Command(CommandEvent::CommitRequested))
        .expect("only a ready preview can commit");
    let rejected = session
        .transition(StateEvent::Command(CommandEvent::CommitRejected {
            detail: "stale revision".to_string(),
        }))
        .expect("a rejected commit produces an outcome");
    assert_eq!(
        rejected.state.command_phase,
        CommandPhase::Outcome {
            outcome: CommandOutcome::Rejected {
                detail: "stale revision".to_string()
            }
        }
    );
    assert_eq!(rejected.state.canonical_revision, "revision-command");
    assert_eq!(
        rejected
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::CommandRejected)
    );
    session
        .transition(StateEvent::Command(CommandEvent::OutcomeDismissed))
        .expect("outcome dismissal returns to modeless idle");
    assert_eq!(
        session.state().interaction_mode,
        InteractionMode::ModelessReady
    );
    assert_eq!(session.state().command_phase, CommandPhase::Idle);

    let invalid = session
        .transition(StateEvent::Command(CommandEvent::CommitRequested))
        .expect_err("commit cannot start without a modal preview");
    assert_eq!(invalid.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(invalid.axis, Some(StateAxis::CommandPhase));

    session
        .transition(StateEvent::Command(CommandEvent::Open {
            command: "fillet".to_string(),
        }))
        .expect("a later command may open after dismissal");
    let cancelled = session
        .transition(StateEvent::Command(CommandEvent::CancelRequested))
        .expect("draft cancellation enters the cancellation phase");
    assert_eq!(cancelled.state.command_phase, CommandPhase::Cancelling);
    assert_eq!(
        cancelled.acknowledgement.marker,
        NonColorMarker::CancellationGlyph
    );
    session
        .transition(StateEvent::Command(CommandEvent::CancellationCompleted {
            detail: "user cancelled".to_string(),
        }))
        .expect("cancellation produces an explicit outcome");
    assert!(matches!(
        session.state().command_phase,
        CommandPhase::Outcome {
            outcome: CommandOutcome::Cancelled { .. }
        }
    ));
    session
        .transition(StateEvent::Command(CommandEvent::OutcomeDismissed))
        .expect("cancelled outcome can be dismissed");

    let mut preview_failure = TuiSession::new([], "revision-preview-failure");
    preview_failure
        .transition_command(CommandEvent::Open {
            command: "extrude".to_string(),
        })
        .expect("preview failure command opens");
    preview_failure
        .transition_command(CommandEvent::PreviewRequested)
        .expect("preview failure enters previewing");
    let preview_rejected = preview_failure
        .transition_command(CommandEvent::PreviewCompleted(PreviewResult::Rejected {
            detail: "preview invalid".to_string(),
        }))
        .expect("preview failure returns to a draft");
    assert!(matches!(
        preview_rejected.state.command_phase,
        CommandPhase::Draft { .. }
    ));
    assert_eq!(
        preview_rejected
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::CommandRejected)
    );

    session
        .transition(StateEvent::FocusCapture(
            FocusCaptureEvent::PointerPressed {
                tool: InteractionTool::Orbit,
                origin: PointerOrigin { column: 1, row: 1 },
                candidate: None,
            },
        ))
        .expect("modeless pointer input can start a drag");
    let dragging = session
        .transition(StateEvent::FocusCapture(FocusCaptureEvent::DragStarted))
        .expect("capture enters drag mode");
    assert_eq!(
        dragging.state.interaction_mode,
        InteractionMode::DragActive {
            tool: InteractionTool::Orbit
        }
    );
    assert_eq!(dragging.acknowledgement.marker, NonColorMarker::MotionTrail);
    let finished = session
        .transition(StateEvent::FocusCapture(FocusCaptureEvent::DragFinished))
        .expect("drag can finish without release correctness");
    assert_eq!(finished.state.capture, CaptureState::None);
    assert_eq!(
        finished.state.interaction_mode,
        InteractionMode::ModelessReady
    );
}

#[test]
fn history_handlers_keep_one_linear_timeline_and_preserve_divergent_future() {
    let mut session = TuiSession::new([], "revision-history-1");
    let unavailable = session
        .transition(StateEvent::History(HistoryEvent::UndoRequested))
        .expect_err("undo is rejected when the linear timeline is empty");
    assert_eq!(unavailable.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(unavailable.axis, Some(StateAxis::History));

    session
        .transition(StateEvent::Command(CommandEvent::Open {
            command: "extrude".to_string(),
        }))
        .expect("command opens");
    session
        .transition(StateEvent::Command(CommandEvent::PreviewRequested))
        .expect("an empty draft can still enter the preview boundary");
    session
        .transition(StateEvent::Command(CommandEvent::PreviewCompleted(
            PreviewResult::Ready,
        )))
        .expect("preview completes");
    session
        .transition(StateEvent::Command(CommandEvent::CommitRequested))
        .expect("commit starts");
    let stale = session
        .transition(StateEvent::Command(CommandEvent::CommitAccepted {
            source_revision: "stale-preview".to_string(),
            validated_revision: "revision-history-2".to_string(),
            revision: "revision-history-stale".to_string(),
        }))
        .expect_err("a stale preview cannot be promoted");
    assert_eq!(stale.code, TuiDiagnosticCode::StalePreview);
    assert_eq!(
        session.state().command_phase,
        CommandPhase::Committing {
            input_fingerprint: String::new()
        }
    );
    session
        .transition(StateEvent::Command(CommandEvent::CommitAccepted {
            source_revision: "revision-history-1".to_string(),
            validated_revision: "revision-history-1".to_string(),
            revision: "revision-history-2".to_string(),
        }))
        .expect("accepted command advances the canonical projection identity");
    session
        .transition(StateEvent::Command(CommandEvent::OutcomeDismissed))
        .expect("commit outcome is dismissed");
    assert_eq!(
        session.state().history,
        HistoryState::Linear {
            can_undo: true,
            can_redo: false,
        }
    );

    let applying_undo = session
        .transition_interaction(InteractionEvent::StartHistory {
            direction: HistoryDirection::Undo,
        })
        .expect("available undo enters atomic history application");
    assert_eq!(
        applying_undo.state.history,
        HistoryState::Applying {
            direction: HistoryDirection::Undo,
            can_undo: true,
            can_redo: false,
        }
    );
    assert_eq!(
        applying_undo.state.interaction_mode,
        InteractionMode::HistoryApplying
    );
    session
        .transition_focus_capture(FocusCaptureEvent::FocusLost)
        .expect("focus loss cancels an in-flight history transition");
    session
        .transition_focus_capture(FocusCaptureEvent::FocusIn)
        .expect("history focus recovery begins");
    session
        .transition_interaction(InteractionEvent::RecoveryCompleted)
        .expect("history focus recovery completes");
    assert_eq!(
        session.state().history,
        HistoryState::Linear {
            can_undo: true,
            can_redo: false,
        }
    );
    session
        .transition_history(HistoryEvent::UndoRequested)
        .expect("history can be retried after recovery");
    session
        .transition(StateEvent::History(HistoryEvent::ApplyCompleted(
            HistoryApplyResult::Applied {
                revision: "revision-history-1".to_string(),
                can_undo: false,
                can_redo: true,
            },
        )))
        .expect("undo publishes one complete linear result");
    assert_eq!(session.state().canonical_revision, "revision-history-1");

    session
        .transition(StateEvent::History(HistoryEvent::RedoRequested))
        .expect("redo is now available");
    session
        .transition(StateEvent::History(HistoryEvent::ApplyCompleted(
            HistoryApplyResult::Applied {
                revision: "revision-history-2".to_string(),
                can_undo: true,
                can_redo: false,
            },
        )))
        .expect("redo publishes atomically");
    session
        .transition(StateEvent::History(HistoryEvent::UndoRequested))
        .expect("undo creates a recoverable future before divergence");
    session
        .transition(StateEvent::History(HistoryEvent::ApplyCompleted(
            HistoryApplyResult::Applied {
                revision: "revision-history-1".to_string(),
                can_undo: false,
                can_redo: true,
            },
        )))
        .expect("undo completes before divergent work");

    let diverged = session
        .transition(StateEvent::History(HistoryEvent::DivergentCommit {
            revision: "revision-history-3".to_string(),
            preserved_named_revision: "future-before-edit".to_string(),
        }))
        .expect("new work preserves the undone future as a named revision");
    assert_eq!(diverged.state.canonical_revision, "revision-history-3");
    assert_eq!(
        diverged.state.recoverable_revisions,
        vec!["future-before-edit".to_string()]
    );
    assert_eq!(
        diverged.state.history,
        HistoryState::Linear {
            can_undo: true,
            can_redo: false,
        }
    );

    session
        .transition(StateEvent::History(HistoryEvent::RestoreNamedRevision {
            name: "future-before-edit".to_string(),
        }))
        .expect("named revision restoration is an atomic history operation");
    let failed = session
        .transition(StateEvent::History(HistoryEvent::ApplyCompleted(
            HistoryApplyResult::Rejected {
                detail: "named revision is unavailable".to_string(),
            },
        )))
        .expect("failed history application returns to the linear timeline");
    assert_eq!(
        failed.diagnostic.as_ref().map(|diagnostic| diagnostic.code),
        Some(TuiDiagnosticCode::HistoryRejected)
    );
    assert_eq!(failed.state.canonical_revision, "revision-history-3");
    assert_eq!(
        failed.state.interaction_mode,
        InteractionMode::ModelessReady
    );
}

#[test]
fn interaction_axis_has_an_explicit_public_handler() {
    let mut session = TuiSession::new([], "revision-interaction-axis");

    let opened = session
        .transition_interaction(InteractionEvent::OpenCommand {
            command: "command-palette".to_string(),
        })
        .expect("interaction handler opens the one command modal");
    assert_eq!(opened.state.interaction_mode, InteractionMode::CommandModal);

    let nested = session
        .transition_interaction(InteractionEvent::OpenCommand {
            command: "nested".to_string(),
        })
        .expect_err("a command modal cannot nest another modal");
    assert_eq!(nested.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(nested.axis, Some(StateAxis::InteractionMode));

    let mut close_session = TuiSession::new([], "revision-interaction-close");
    close_session
        .transition_command(CommandEvent::Open {
            command: "close-me".to_string(),
        })
        .expect("close test command opens");
    close_session
        .transition_command(CommandEvent::CancelRequested)
        .expect("close test command cancels");
    close_session
        .transition_command(CommandEvent::CancellationCompleted {
            detail: "closed by caller".to_string(),
        })
        .expect("close test command has an outcome");
    close_session
        .transition_interaction(InteractionEvent::CloseCommand)
        .expect("interaction handler dismisses the command outcome");

    let mut drag_session = TuiSession::new([], "revision-interaction-drag");
    drag_session
        .transition_focus_capture(FocusCaptureEvent::PointerPressed {
            tool: InteractionTool::Orbit,
            origin: PointerOrigin { column: 2, row: 2 },
            candidate: None,
        })
        .expect("drag has a pointer capture");
    drag_session
        .transition_interaction(InteractionEvent::StartDrag {
            tool: InteractionTool::Orbit,
        })
        .expect("interaction handler enters drag mode");
    assert_eq!(
        drag_session.state().interaction_mode,
        InteractionMode::DragActive {
            tool: InteractionTool::Orbit
        }
    );
    drag_session
        .transition_interaction(InteractionEvent::FinishDrag)
        .expect("interaction handler exits drag mode");

    let mut focus_command = TuiSession::new([], "revision-interaction-command-focus");
    focus_command
        .transition_command(CommandEvent::Open {
            command: "extrude".to_string(),
        })
        .expect("command opens before focus loss");
    focus_command
        .transition_command(CommandEvent::PreviewRequested)
        .expect("preview is active before focus loss");
    let focus_lost = focus_command
        .transition_focus_capture(FocusCaptureEvent::FocusLost)
        .expect("focus loss cancels the transient preview");
    assert_eq!(focus_lost.state.focus, FocusState::FocusLost);
    assert!(matches!(
        focus_lost.state.command_phase,
        CommandPhase::Draft { .. }
    ));
    focus_command
        .transition_focus_capture(FocusCaptureEvent::FocusIn)
        .expect("focus returns through recovery");
    focus_command
        .transition_interaction(InteractionEvent::RecoveryCompleted)
        .expect("recovery can return to the open command modal");
    assert_eq!(
        focus_command.state().interaction_mode,
        InteractionMode::CommandModal
    );

    let mut recovery_session = TuiSession::new([], "revision-interaction-recovery");
    recovery_session
        .transition_focus_capture(FocusCaptureEvent::FocusLost)
        .expect("focus can be lost");
    recovery_session
        .transition_focus_capture(FocusCaptureEvent::FocusIn)
        .expect("focus enters recovery readiness");
    recovery_session
        .transition_interaction(InteractionEvent::RecoveryCompleted)
        .expect("interaction handler completes recovery");
    assert_eq!(
        recovery_session.state().interaction_mode,
        InteractionMode::ModelessReady
    );
}

#[test]
fn invalid_transition_matrix_is_structured_and_state_preserving() {
    let reject = |session: &mut TuiSession, event: StateEvent, axis: StateAxis| {
        let before = session.state();
        let diagnostic = session
            .transition(event)
            .expect_err("documented-invalid event must be rejected");
        assert_eq!(diagnostic.code, TuiDiagnosticCode::InvalidTransition);
        assert_eq!(diagnostic.axis, Some(axis));
        assert!(!diagnostic.detail.is_empty());
        assert_eq!(session.state(), before);
    };

    let mut lifecycle = TuiSession::new([], "revision-invalid-lifecycle");
    reject(
        &mut lifecycle,
        StateEvent::Lifecycle(LifecycleEvent::ProbeSucceeded),
        StateAxis::Lifecycle,
    );
    reject(
        &mut lifecycle,
        StateEvent::Lifecycle(LifecycleEvent::ResizeCompleted),
        StateAxis::Lifecycle,
    );
    reject(
        &mut lifecycle,
        StateEvent::Lifecycle(LifecycleEvent::RestoreCompleted),
        StateAxis::Lifecycle,
    );
    reject(
        &mut lifecycle,
        StateEvent::Lifecycle(LifecycleEvent::CleanupCompleted),
        StateAxis::Lifecycle,
    );
    reject(
        &mut lifecycle,
        StateEvent::Lifecycle(LifecycleEvent::ProbeStarted),
        StateAxis::Lifecycle,
    );

    let mut headless = TuiSession::new_probing([], "revision-invalid-headless");
    headless
        .transition_lifecycle(LifecycleEvent::ProbeFailed {
            detail: "no capability".to_string(),
        })
        .expect("invalid matrix setup enters headless mode");
    reject(
        &mut headless,
        StateEvent::History(HistoryEvent::UndoRequested),
        StateAxis::History,
    );

    let mut focus = TuiSession::new([], "revision-invalid-focus");
    reject(
        &mut focus,
        StateEvent::FocusCapture(FocusCaptureEvent::FocusIn),
        StateAxis::FocusCapture,
    );
    reject(
        &mut focus,
        StateEvent::FocusCapture(FocusCaptureEvent::PointerReleased),
        StateAxis::FocusCapture,
    );
    reject(
        &mut focus,
        StateEvent::FocusCapture(FocusCaptureEvent::DragStarted),
        StateAxis::FocusCapture,
    );
    reject(
        &mut focus,
        StateEvent::FocusCapture(FocusCaptureEvent::RecoveryCompleted),
        StateAxis::FocusCapture,
    );
    focus
        .transition_focus_capture(FocusCaptureEvent::PointerPressed {
            tool: InteractionTool::Selection,
            origin: PointerOrigin { column: 1, row: 1 },
            candidate: None,
        })
        .expect("focus invalid matrix setup capture");
    reject(
        &mut focus,
        StateEvent::FocusCapture(FocusCaptureEvent::PointerPressed {
            tool: InteractionTool::Orbit,
            origin: PointerOrigin { column: 2, row: 2 },
            candidate: None,
        }),
        StateAxis::FocusCapture,
    );

    let mut selection = TuiSession::new([], "revision-invalid-selection");
    reject(
        &mut selection,
        StateEvent::Selection(SelectionEvent::Verify(SelectionVerification::Exact {
            stable_ids: vec!["missing".to_string()],
        })),
        StateAxis::Selection,
    );
    reject(
        &mut selection,
        StateEvent::Selection(SelectionEvent::Clear),
        StateAxis::Selection,
    );
    reject(
        &mut selection,
        StateEvent::Selection(SelectionEvent::Nominate {
            candidates: Vec::new(),
        }),
        StateAxis::Selection,
    );

    let mut interaction = TuiSession::new([], "revision-invalid-interaction");
    reject(
        &mut interaction,
        StateEvent::Interaction(InteractionEvent::FinishDrag),
        StateAxis::InteractionMode,
    );
    reject(
        &mut interaction,
        StateEvent::Interaction(InteractionEvent::RecoveryCompleted),
        StateAxis::InteractionMode,
    );
    reject(
        &mut interaction,
        StateEvent::Interaction(InteractionEvent::CloseCommand),
        StateAxis::InteractionMode,
    );
    reject(
        &mut interaction,
        StateEvent::Interaction(InteractionEvent::StartDrag {
            tool: InteractionTool::Orbit,
        }),
        StateAxis::InteractionMode,
    );
    reject(
        &mut interaction,
        StateEvent::Interaction(InteractionEvent::StartHistory {
            direction: HistoryDirection::Undo,
        }),
        StateAxis::InteractionMode,
    );

    let mut command = TuiSession::new([], "revision-invalid-command");
    reject(
        &mut command,
        StateEvent::Command(CommandEvent::PreviewRequested),
        StateAxis::CommandPhase,
    );
    reject(
        &mut command,
        StateEvent::Command(CommandEvent::CommitRequested),
        StateAxis::CommandPhase,
    );
    reject(
        &mut command,
        StateEvent::Command(CommandEvent::CancelRequested),
        StateAxis::CommandPhase,
    );
    reject(
        &mut command,
        StateEvent::Command(CommandEvent::OutcomeDismissed),
        StateAxis::CommandPhase,
    );
    reject(
        &mut command,
        StateEvent::Command(CommandEvent::Open {
            command: String::new(),
        }),
        StateAxis::CommandPhase,
    );
    command
        .transition_command(CommandEvent::Open {
            command: "one-modal".to_string(),
        })
        .expect("command invalid matrix setup modal");
    reject(
        &mut command,
        StateEvent::Command(CommandEvent::Open {
            command: "nested".to_string(),
        }),
        StateAxis::CommandPhase,
    );

    let mut history = TuiSession::new([], "revision-invalid-history");
    reject(
        &mut history,
        StateEvent::History(HistoryEvent::UndoRequested),
        StateAxis::History,
    );
    reject(
        &mut history,
        StateEvent::History(HistoryEvent::RedoRequested),
        StateAxis::History,
    );
    reject(
        &mut history,
        StateEvent::History(HistoryEvent::ApplyCompleted(HistoryApplyResult::Rejected {
            detail: "not applying".to_string(),
        })),
        StateAxis::History,
    );
    reject(
        &mut history,
        StateEvent::History(HistoryEvent::RestoreNamedRevision {
            name: "missing".to_string(),
        }),
        StateAxis::History,
    );
    reject(
        &mut history,
        StateEvent::History(HistoryEvent::DivergentCommit {
            revision: "revision-invalid-history-2".to_string(),
            preserved_named_revision: "future".to_string(),
        }),
        StateAxis::History,
    );
}

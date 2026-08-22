use threeterm_tui::{
    FocusCaptureEvent, HistoryEvent, InteractionEvent, InteractionTool, LifecycleEvent,
    LifecycleState, PointerOrigin, SelectionEvent, StateAxis, TuiDiagnosticCode, TuiSession,
};

#[test]
fn headless_only_blocks_interactive_axes_and_retries_to_interactive_ready() {
    let mut session = TuiSession::new_probing([], "revision-headless-retention");
    let failed = session
        .transition_lifecycle(LifecycleEvent::ProbeFailed {
            detail: "no Ghostty".to_string(),
        })
        .expect("probe failure enters headless");
    assert_eq!(failed.state.lifecycle, LifecycleState::HeadlessOnly);
    assert_eq!(
        failed.diagnostic.as_ref().unwrap().code,
        TuiDiagnosticCode::LifecycleFailure
    );
    assert_eq!(
        failed.state.canonical_revision,
        "revision-headless-retention"
    );
    assert!(failed.acknowledgement.text.contains("headless"));

    // Every guarded axis must be InvalidTransition while HeadlessOnly.
    let blocked = session
        .transition_lifecycle(LifecycleEvent::ResizeStarted)
        .expect_err("resize blocked");
    assert_eq!(blocked.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(blocked.axis, Some(StateAxis::Lifecycle));
    assert_eq!(session.state().lifecycle, LifecycleState::HeadlessOnly);

    let blocked = session
        .transition_selection(SelectionEvent::Nominate {
            candidates: vec!["feature-a".to_string()],
        })
        .expect_err("selection blocked");
    assert_eq!(blocked.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(blocked.axis, Some(StateAxis::Selection));

    let blocked = session
        .transition_focus_capture(FocusCaptureEvent::PointerPressed {
            tool: InteractionTool::Orbit,
            origin: PointerOrigin { column: 1, row: 1 },
            candidate: None,
        })
        .expect_err("focus blocked");
    assert_eq!(blocked.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(blocked.axis, Some(StateAxis::FocusCapture));

    let blocked = session
        .transition_interaction(InteractionEvent::OpenCommand {
            command: "bracket".to_string(),
        })
        .expect_err("interaction blocked");
    assert_eq!(blocked.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(blocked.axis, Some(StateAxis::InteractionMode));

    let blocked = session
        .transition_history(HistoryEvent::UndoRequested)
        .expect_err("history blocked");
    assert_eq!(blocked.code, TuiDiagnosticCode::InvalidTransition);
    assert_eq!(blocked.axis, Some(StateAxis::History));

    // Retry via ProbeStarted -> Probing
    let probing = session
        .transition_lifecycle(LifecycleEvent::ProbeStarted)
        .expect("headless retries");
    assert_eq!(probing.state.lifecycle, LifecycleState::Probing);
    // Probing->InteractiveReady restores deterministically
    let ready = session
        .transition_lifecycle(LifecycleEvent::ProbeSucceeded)
        .expect("probe success restores interactive");
    assert_eq!(ready.state.lifecycle, LifecycleState::InteractiveReady);
    assert_eq!(
        ready.state.canonical_revision,
        "revision-headless-retention"
    );
    assert!(ready.acknowledgement.text.contains("interactive"));
    assert!(
        session
            .transition_lifecycle(LifecycleEvent::ResizeStarted)
            .is_ok()
    );
}

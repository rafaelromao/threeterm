use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_persistence::Bundle;
use threeterm_theme::NonColorMarker;
use threeterm_tui::{
    ArrowKey, CaptureState, CommandEvent, CommandPhase, FeatureTarget, FocusCaptureEvent,
    FocusState, HistoryEvent, HistoryState, InteractionEvent, InteractionMode, LifecycleEvent,
    LifecycleState, NavigationResult, PreviewResult, SelectionEvent, SelectionState,
    TuiDiagnosticCode, TuiSession,
};

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-tui-navigation-{nanos}"))
}

#[test]
fn modeless_arrow_navigation_uses_the_canonical_host_projection() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("first feature is persisted");
    host.save(&root, "feature-b", "fillet")
        .expect("second feature is persisted");
    let before = host.current().expect("host has a canonical snapshot");
    let graph = host
        .current_graph()
        .expect("host exposes its read-only canonical graph");

    let mut session = TuiSession::from_feature_graph(&graph, &before.revision_hash);
    let initial = session.state();
    assert!(initial.selected_target.is_none());
    assert_eq!(initial.interaction_mode, InteractionMode::ModelessReady);
    assert_eq!(initial.canonical_revision, before.revision_hash);

    let outcome = session.press(ArrowKey::Down);

    assert!(outcome.diagnostic.is_none());
    assert_eq!(outcome.frame.selected_target.as_deref(), Some("feature-a"));
    assert_eq!(
        session.state().selection,
        SelectionState::Selected {
            stable_ids: vec!["feature-a".to_string()]
        }
    );
    session
        .transition_selection(SelectionEvent::Clear)
        .expect("explicit selection clearing updates the public projection");
    assert_eq!(session.state().selected_target, None);
    assert_eq!(outcome.frame.acknowledgement.sequence, 1);
    assert_eq!(
        outcome.frame.acknowledgement.marker,
        NonColorMarker::SelectionGlyph
    );
    assert!(outcome.frame.acknowledgement.text.contains("feature-a"));
    assert_eq!(host.current(), Some(before.clone()));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn arrow_aliases_traverse_in_order_and_acknowledge_boundaries() {
    let mut session = TuiSession::new(
        [
            FeatureTarget::new("feature-a", "box"),
            FeatureTarget::new("feature-b", "fillet"),
        ],
        "revision-test",
    );

    let last = session.press(ArrowKey::Up);
    assert_eq!(last.frame.selected_target.as_deref(), Some("feature-b"));
    assert_eq!(last.frame.acknowledgement.sequence, 1);
    assert_eq!(last.frame.acknowledgement.result, NavigationResult::Moved);

    let previous = session.press(ArrowKey::Left);
    assert_eq!(previous.frame.selected_target.as_deref(), Some("feature-a"));
    assert_eq!(previous.frame.acknowledgement.sequence, 2);

    let next = session.press(ArrowKey::Right);
    assert_eq!(next.frame.selected_target.as_deref(), Some("feature-b"));
    assert_eq!(next.frame.acknowledgement.sequence, 3);

    let boundary = session.press(ArrowKey::Down);
    assert_eq!(boundary.frame.selected_target.as_deref(), Some("feature-b"));
    assert_eq!(
        boundary.frame.acknowledgement.result,
        NavigationResult::Boundary
    );
    assert_eq!(boundary.frame.acknowledgement.sequence, 4);
    assert!(boundary.frame.acknowledgement.text.contains("boundary"));
}

#[test]
fn each_arrow_press_has_text_and_marker_acknowledgement() {
    let mut session = TuiSession::new([FeatureTarget::new("feature-a", "box")], "revision-ack");

    let first = session.press(ArrowKey::Down);
    let second = session.press(ArrowKey::Down);
    let third = session.press(ArrowKey::Down);

    assert_eq!(first.frame.acknowledgement.sequence, 1);
    assert_eq!(second.frame.acknowledgement.sequence, 2);
    assert_eq!(third.frame.acknowledgement.sequence, 3);
    assert_eq!(first.frame.acknowledgement.key, ArrowKey::Down);
    assert_eq!(second.frame.acknowledgement.key, ArrowKey::Down);
    assert!(!first.frame.acknowledgement.text.is_empty());
    assert!(!second.frame.acknowledgement.text.is_empty());
    assert!(!third.frame.acknowledgement.text.is_empty());
    assert_eq!(
        first.frame.acknowledgement.marker,
        NonColorMarker::SelectionGlyph
    );
    assert_eq!(
        second.frame.acknowledgement.marker,
        NonColorMarker::SelectionGlyph
    );
    assert_ne!(
        first.frame.acknowledgement.text,
        second.frame.acknowledgement.text
    );
    assert_ne!(
        second.frame.acknowledgement.text,
        third.frame.acknowledgement.text
    );
    assert_eq!(
        session.state().last_acknowledgement,
        Some(third.frame.acknowledgement)
    );
}

#[test]
fn empty_canonical_projection_reports_a_visible_structured_failure() {
    let root = temporary_bundle_root();
    Bundle::create(&root).expect("empty canonical bundle is created");
    let host = Host::new();
    host.load(&root)
        .expect("host loads the empty canonical bundle");
    let before = host.current().expect("host has an empty snapshot");
    let graph = host
        .current_graph()
        .expect("host exposes the empty canonical graph");
    assert!(graph.features().next().is_none());

    let mut session = TuiSession::from_feature_graph(&graph, &before.revision_hash);
    let outcome = session.press(ArrowKey::Right);

    let diagnostic = outcome.diagnostic.expect("empty navigation is diagnosed");
    assert_eq!(diagnostic.code, TuiDiagnosticCode::NoFeatureTarget);
    assert_eq!(diagnostic.code.as_str(), "no_feature_target");
    assert_eq!(diagnostic.canonical_revision, before.revision_hash);
    assert_eq!(outcome.frame.selected_target, None);
    assert_eq!(outcome.frame.acknowledgement.sequence, 1);
    assert_eq!(
        outcome.frame.acknowledgement.result,
        NavigationResult::NoFeatureTarget
    );
    assert_eq!(
        outcome.frame.acknowledgement.marker,
        NonColorMarker::ErrorGlyph
    );
    assert!(!outcome.frame.acknowledgement.text.is_empty());
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_session_exposes_orthogonal_ready_state_without_mutating_host() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("first feature is persisted");
    host.save(&root, "feature-b", "fillet")
        .expect("second feature is persisted");
    let before = host.current().expect("host has a canonical snapshot");
    let graph = host
        .current_graph()
        .expect("host exposes its read-only canonical graph");
    let mut session = TuiSession::from_feature_graph(&graph, &before.revision_hash);

    let state = session.state();
    assert_eq!(state.lifecycle, LifecycleState::InteractiveReady);
    assert_eq!(state.focus, FocusState::Focused);
    assert_eq!(state.capture, CaptureState::None);
    assert_eq!(state.interaction_mode, InteractionMode::ModelessReady);
    assert_eq!(state.command_phase, CommandPhase::Idle);
    assert_eq!(
        state.history,
        HistoryState::Linear {
            can_undo: false,
            can_redo: false,
        }
    );
    assert_eq!(state.canonical_revision, before.revision_hash);

    let moved = session.press(ArrowKey::Down);
    let boundary = session.press(ArrowKey::Up);
    assert_eq!(moved.frame.selected_target.as_deref(), Some("feature-a"));
    assert_eq!(boundary.frame.selected_target.as_deref(), Some("feature-a"));
    assert_eq!(
        boundary.frame.acknowledgement.result,
        NavigationResult::Boundary
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn terminal_arrow_bytes_drive_selection_and_render_the_visible_acknowledgement() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("host has a canonical snapshot");
    let graph = host
        .current_graph()
        .expect("host exposes its read-only canonical graph");
    let mut session = TuiSession::from_feature_graph(&graph, &before.revision_hash);

    let rendered = session
        .process_terminal_input(b"\x1b[B")
        .expect("down arrow is decoded by the production input path");

    assert_eq!(rendered.frame.selected_target.as_deref(), Some("feature-a"));
    assert!(rendered.overlay.contains("[selection-glyph]"));
    assert!(rendered.overlay.contains("Acknowledgement 1:"));
    assert!(rendered.overlay.contains("feature-a"));
    assert!(rendered.diagnostic.is_none());
    assert_eq!(host.current(), Some(before.clone()));

    let invalid = session
        .process_terminal_input(b"not-an-arrow")
        .expect_err("malformed terminal input is diagnosed");
    assert_eq!(invalid.code, TuiDiagnosticCode::InvalidArrowInput);
    assert_eq!(invalid.code.as_str(), "invalid_arrow_input");
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_arrow_navigation_rejects_non_ready_states_without_mutation() {
    let cases = vec![
        {
            let mut session = TuiSession::new([FeatureTarget::new("feature-a", "box")], "r");
            session
                .transition_lifecycle(LifecycleEvent::ResizeStarted)
                .expect("resize starts");
            session
        },
        {
            let mut session = TuiSession::new([FeatureTarget::new("feature-a", "box")], "r");
            session
                .transition_focus_capture(FocusCaptureEvent::FocusLost)
                .expect("focus is lost");
            session
        },
        {
            let mut session = TuiSession::new([FeatureTarget::new("feature-a", "box")], "r");
            session
                .transition_command(CommandEvent::Open {
                    command: "fillet".to_string(),
                })
                .expect("command opens");
            session
        },
        {
            let mut session = TuiSession::new([FeatureTarget::new("feature-a", "box")], "r");
            session
                .transition_focus_capture(FocusCaptureEvent::PointerPressed {
                    tool: threeterm_tui::InteractionTool::Selection,
                    origin: threeterm_tui::PointerOrigin { column: 1, row: 1 },
                    candidate: Some("feature-a".to_string()),
                })
                .expect("capture starts");
            session
        },
        {
            let mut session = TuiSession::new([FeatureTarget::new("feature-a", "box")], "r");
            session
                .transition_lifecycle(LifecycleEvent::CloseRequested)
                .expect("close starts");
            session
                .transition_lifecycle(LifecycleEvent::CleanupCompleted)
                .expect("close completes");
            session
        },
        {
            let mut session =
                TuiSession::new_probing([FeatureTarget::new("feature-a", "box")], "r");
            session
                .transition_lifecycle(LifecycleEvent::ProbeFailed {
                    detail: "headless".to_string(),
                })
                .expect("probe failure enters headless mode");
            session
        },
        {
            let mut session = TuiSession::new([FeatureTarget::new("feature-a", "box")], "r");
            session
                .transition_command(CommandEvent::Open {
                    command: "fillet".to_string(),
                })
                .expect("command opens");
            session
                .transition_command(CommandEvent::DraftUpdated {
                    input_fingerprint: "input".to_string(),
                })
                .expect("draft updates");
            session
                .transition_command(CommandEvent::PreviewRequested)
                .expect("preview starts");
            session
                .transition_command(CommandEvent::PreviewCompleted(PreviewResult::Ready))
                .expect("preview completes");
            session
                .transition_command(CommandEvent::CommitRequested)
                .expect("commit starts");
            session
                .transition_command(CommandEvent::CommitAccepted {
                    source_revision: "r".to_string(),
                    validated_revision: "r".to_string(),
                    revision: "r2".to_string(),
                })
                .expect("commit completes");
            session
                .transition_interaction(InteractionEvent::CloseCommand)
                .expect("command outcome closes");
            session
                .transition_history(HistoryEvent::UndoRequested)
                .expect("history starts");
            session
        },
    ];

    for mut session in cases {
        let before = session.state();
        let outcome = session.press(ArrowKey::Down);
        let diagnostic = outcome.diagnostic.expect("navigation is diagnosed");
        assert_eq!(diagnostic.code, TuiDiagnosticCode::InvalidTransition);
        assert_eq!(diagnostic.axis, Some(threeterm_tui::StateAxis::Selection));
        assert_eq!(session.state().selection, before.selection);
        assert_eq!(session.state().selected_target, before.selected_target);
    }
}

use std::fs;
use std::path::PathBuf;

use threeterm_domain::{ProjectGeneration, history::HistoryState};
use threeterm_persistence::{Bundle, write_fresh};

fn root(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "threeterm-history-persistence-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    path
}

#[test]
fn history_events_reopen_and_replay_as_two_equal_states() {
    let path = root("replay");
    let bundle = Bundle::at(&path);
    write_fresh(&path, ProjectGeneration::with_id("history-test")).expect("fresh bundle");
    let state = HistoryState::default();
    let event = state
        .initialize_l_bracket("l", 10.0, 5.0, 3.0, 1.0)
        .expect("history event");
    bundle
        .append_features_with_history(&[], &event)
        .expect("history event publishes");

    let loaded = bundle.open().expect("bundle reopens");
    assert_eq!(loaded.history.active.features.len(), 5);
    let (first, second) = bundle.replay_history_states().expect("history replays");
    assert_eq!(first, second);
    assert_eq!(first, loaded.history);

    let _ = fs::remove_dir_all(path);
}

#[test]
fn legacy_feature_logs_replay_with_an_empty_history_state() {
    let path = root("legacy");
    let bundle = Bundle::at(&path);
    bundle
        .append_feature("legacy-feature", "box")
        .expect("legacy feature publishes");
    let loaded = bundle.open().expect("legacy bundle reopens");
    assert_eq!(loaded.history, HistoryState::default());
    assert_eq!(loaded.graph.features().count(), 1);

    let _ = fs::remove_dir_all(path);
}

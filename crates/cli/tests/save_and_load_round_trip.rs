use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn save_then_separate_load_preserves_feature_graph_and_revision_hashes() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-save-load-{suffix}"));

    let saved = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "save"])
        .arg(&root)
        .args(["--feature-id", "box-1", "--kind", "box"])
        .output()
        .expect("save process runs");
    assert!(
        saved.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&saved.stderr)
    );
    assert!(saved.stderr.is_empty());
    let saved: Value = serde_json::from_slice(&saved.stdout).expect("save response is JSON");

    let loaded = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "load"])
        .arg(&root)
        .output()
        .expect("load process runs");
    assert!(
        loaded.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    assert!(loaded.stderr.is_empty());
    let loaded: Value = serde_json::from_slice(&loaded.stdout).expect("load response is JSON");

    assert_eq!(
        saved.as_object().expect("save response is an object").len(),
        3
    );
    assert_eq!(
        loaded
            .as_object()
            .expect("load response is an object")
            .len(),
        4
    );
    assert_eq!(saved["schema_version"], "threeterm.command.save.response/1");
    assert_eq!(
        loaded["schema_version"],
        "threeterm.command.load.response/1"
    );
    assert_eq!(loaded["recovered_from_previous"], false);
    for key in ["feature_graph_hash", "revision_hash"] {
        let saved_hash = saved[key].as_str().expect("save response hash is a string");
        let loaded_hash = loaded[key]
            .as_str()
            .expect("load response hash is a string");
        assert_eq!(saved_hash.len(), 64);
        assert!(
            saved_hash
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
        assert_eq!(saved_hash, loaded_hash);
    }
    assert!(root.join("manifest.json").is_file());
    let transactions = fs::read_to_string(root.join("transactions.log"))
        .expect("canonical transaction log is readable");
    assert!(!transactions.is_empty());

    let _ = fs::remove_dir_all(root);
}

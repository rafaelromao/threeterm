use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_persistence::load;

#[test]
fn new_project_creates_and_reloads_one_empty_revision_bundle() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-new-project-{suffix}"));
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .arg("new-project")
        .arg(&root)
        .output()
        .expect("threeterm binary runs");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).expect("response is JSON");
    let loaded = load(&root).expect("created bundle reloads");
    assert_eq!(response["generation_id"], loaded.manifest.generation_id);
    assert_eq!(loaded.manifest.revision_count, 1);
    assert_eq!(loaded.manifest.transaction_count, 0);
    assert!(loaded.transactions.is_empty());
    assert!(root.join("manifest.json").is_file());
    assert!(root.join("transactions.log").is_file());

    let _ = fs::remove_dir_all(root);
}

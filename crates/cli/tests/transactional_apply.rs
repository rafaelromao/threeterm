use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-cli-apply-{suffix}"))
}

fn run(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "command {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    serde_json::from_str(text.trim()).expect("stdout is one JSON value")
}

fn apply(
    root: &Path,
    revision: &str,
    operation: &str,
    feature_id: &str,
    kind: Option<&str>,
) -> Value {
    let path = root.to_str().expect("root is UTF-8");
    let mut args = vec![
        "--machine",
        "apply",
        path,
        "--expected-revision",
        revision,
        "--operation",
        operation,
        "--feature-id",
        feature_id,
    ];
    if let Some(kind) = kind {
        args.extend(["--kind", kind]);
    }
    run(&args)
}

#[test]
fn machine_mode_applies_operations_and_reloads_identical_identity() {
    let root = root();
    let path = root.to_str().expect("root is UTF-8");
    let created = run(&["--machine", "new-project", path]);
    assert_eq!(created["generation_id"].as_str().unwrap().len(), 64);

    let initial = run(&["--machine", "identity", path]);
    let added = apply(
        &root,
        initial["revision_hash"].as_str().unwrap(),
        "add",
        "box",
        Some("cube"),
    );
    let set = apply(
        &root,
        added["revision_hash"].as_str().unwrap(),
        "set",
        "box",
        Some("sphere"),
    );
    let removed = apply(
        &root,
        set["revision_hash"].as_str().unwrap(),
        "remove",
        "box",
        None,
    );
    assert_eq!(removed["transaction_count"], 3);

    let before_rejection_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_rejection_log = fs::read(root.join("transactions.log")).expect("log reads");
    let stale = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "--machine",
            "apply",
            path,
            "--expected-revision",
            initial["revision_hash"].as_str().unwrap(),
            "--operation",
            "add",
            "--feature-id",
            "stale",
            "--kind",
            "cube",
        ])
        .output()
        .expect("stale command runs");
    assert!(!stale.status.success());
    assert!(stale.stdout.is_empty());
    assert!(!stale.stderr.is_empty());
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_rejection_manifest
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).unwrap(),
        before_rejection_log
    );

    let before_load = run(&["--machine", "identity", path]);
    let _loaded = run(&["--machine", "load", path]);
    let after_load = run(&["--machine", "identity", path]);
    assert_eq!(
        after_load, before_load,
        "identity is byte-equal after reload"
    );

    let _ = fs::remove_dir_all(&root);
}

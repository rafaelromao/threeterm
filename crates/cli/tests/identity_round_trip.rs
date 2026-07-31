//! End-to-end integration test for the Project Generation identity invariant.
//!
//! Runs the full MVP operation set on a project (create, add-feature, set-parameter,
//! add-feature, set-parameter, remove-feature) and asserts the Project Generation
//! identity is byte-equal after a reload from disk. The test invokes the
//! production `threeterm` binary via `CARGO_BIN_EXE_threeterm` so the whole
//! stack — dispatcher, persistence layer, integrity verifier — is exercised
//! on the production code path.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_threeterm")
}

fn fresh_bundle_root(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = std::sync::atomic::AtomicU64::new(0);
    let seq = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "threeterm-identity-round-trip-{name}-{nanos}-{seq}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn run(args: &[OsString]) -> std::process::Output {
    let bin = bin();
    Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm binary runs")
}

fn json_from_stdout(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is utf-8");
    serde_json::from_str(&stdout).expect("stdout is parseable JSON")
}

fn identity_hex(output: &std::process::Output) -> String {
    let parsed = json_from_stdout(output);
    if let Some(value) = parsed["identity"].as_str() {
        return value.to_string();
    }
    // The new-project response carries the identity as `manifest.transaction_sha256`.
    let from_manifest = parsed["manifest"]["transaction_sha256"]
        .as_str()
        .expect("identity must be reachable as either top-level or manifest.transaction_sha256");
    from_manifest.to_string()
}

fn s(value: &str) -> OsString {
    OsString::from(value)
}

#[test]
fn identity_survives_a_reload_after_applying_the_full_mvp_operation_set() {
    let dir = fresh_bundle_root("full-mvp");

    let new_project = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(
        new_project.status.success(),
        "new-project failed: stderr: {}",
        String::from_utf8_lossy(&new_project.stderr)
    );
    let initial_identity = identity_hex(&new_project);

    let add_sketch = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(
            r#"{"kind":"add-feature","feature_id":"sketch-1","feature_kind":"sketch","parameters":{"width":10.0}}"#,
        ),
    ]);
    assert!(
        add_sketch.status.success(),
        "add-feature(sketch) failed: stderr: {}",
        String::from_utf8_lossy(&add_sketch.stderr)
    );
    let after_add_sketch = identity_hex(&add_sketch);

    let set_width = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(r#"{"kind":"set-parameter","feature_id":"sketch-1","parameter":"width","value":20.0}"#),
    ]);
    assert!(
        set_width.status.success(),
        "set-parameter(width) failed: stderr: {}",
        String::from_utf8_lossy(&set_width.stderr)
    );
    let after_set_width = identity_hex(&set_width);

    let add_extrude = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(
            r#"{"kind":"add-feature","feature_id":"extrude-1","feature_kind":"extrude","parameters":{"depth":5.0}}"#,
        ),
    ]);
    assert!(
        add_extrude.status.success(),
        "add-feature(extrude) failed: stderr: {}",
        String::from_utf8_lossy(&add_extrude.stderr)
    );
    let after_add_extrude = identity_hex(&add_extrude);

    let set_depth = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(r#"{"kind":"set-parameter","feature_id":"extrude-1","parameter":"depth","value":7.0}"#),
    ]);
    assert!(
        set_depth.status.success(),
        "set-parameter(depth) failed: stderr: {}",
        String::from_utf8_lossy(&set_depth.stderr)
    );
    let after_set_depth = identity_hex(&set_depth);

    let remove_sketch = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(r#"{"kind":"remove-feature","feature_id":"sketch-1"}"#),
    ]);
    assert!(
        remove_sketch.status.success(),
        "remove-feature(sketch) failed: stderr: {}",
        String::from_utf8_lossy(&remove_sketch.stderr)
    );
    let after_remove_sketch = identity_hex(&remove_sketch);

    // Sanity: every accepted command transaction produces a different identity.
    assert_ne!(initial_identity, after_add_sketch);
    assert_ne!(after_add_sketch, after_set_width);
    assert_ne!(after_set_width, after_add_extrude);
    assert_ne!(after_add_extrude, after_set_depth);
    assert_ne!(after_set_depth, after_remove_sketch);

    // `identity` returns the same SHA-256 hex as the last `apply`.
    let identity_cmd = run(&[s("identity"), dir.clone().into_os_string()]);
    assert!(
        identity_cmd.status.success(),
        "identity failed: stderr: {}",
        String::from_utf8_lossy(&identity_cmd.stderr)
    );
    let from_identity = identity_hex(&identity_cmd);
    assert_eq!(
        from_identity, after_remove_sketch,
        "`identity` must match the last `apply`"
    );

    // `load` returns the same identity and confirms the log state.
    let load = run(&[s("load"), dir.clone().into_os_string()]);
    assert!(
        load.status.success(),
        "load failed: stderr: {}",
        String::from_utf8_lossy(&load.stderr)
    );
    let loaded = json_from_stdout(&load);
    assert_eq!(
        loaded["manifest"]["transaction_sha256"], after_remove_sketch,
        "`load` must report the same identity as the last `apply`"
    );
    assert_eq!(
        loaded["transaction_count"],
        5,
        "five accepted transactions: stderr: {}",
        String::from_utf8_lossy(&load.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bundle_loader_rejects_a_tampered_log_line() {
    let dir = fresh_bundle_root("tampered");
    let new_project = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(new_project.status.success());

    let add = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(
            r#"{"kind":"add-feature","feature_id":"feat-1","feature_kind":"sketch","parameters":{}}"#,
        ),
    ]);
    assert!(add.status.success());

    // Tamper with the canonical log on disk.
    let log_path = dir.join("canonical/transactions.ndjson");
    let mut bytes = fs::read(&log_path).expect("log readable");
    if let Some(open_idx) = bytes.iter().position(|b| *b == b'{') {
        bytes[open_idx + 1] = b'X';
    }
    fs::write(&log_path, &bytes).expect("rewrite log");

    let identity = run(&[s("identity"), dir.clone().into_os_string()]);
    assert!(
        !identity.status.success(),
        "tampered log must be rejected by the identity subcommand"
    );
    let stderr_text = String::from_utf8_lossy(&identity.stderr);
    assert!(
        stderr_text.contains("persistence_failure"),
        "stderr must carry a structured persistence_failure diagnostic, got: {stderr_text}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn apply_command_writes_a_digest_chain_to_the_log() {
    let dir = fresh_bundle_root("digest-chain");
    let new_project = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(new_project.status.success());

    let add = run(&[
        s("apply"),
        dir.clone().into_os_string(),
        s(
            r#"{"kind":"add-feature","feature_id":"feat-1","feature_kind":"sketch","parameters":{}}"#,
        ),
    ]);
    assert!(add.status.success());

    let log_path = dir.join("canonical/transactions.ndjson");
    let raw = fs::read_to_string(&log_path).expect("log readable");
    let lines: Vec<&str> = raw.split('\n').filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1, "one accepted transaction => one log line");
    let entry: Value = serde_json::from_str(lines[0]).expect("line is JSON");
    let parent = entry["parent_identity"]
        .as_str()
        .expect("parent_identity is a string");
    assert_eq!(
        parent.len(),
        64,
        "parent_identity is the lowercase hex of the empty-log digest"
    );
    assert_eq!(
        parent, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "parent_identity of the first transaction must be the empty-log digest"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn identity_subcommand_succeeds_immediately_after_new_project() {
    let dir = fresh_bundle_root("identity-after-new-project");
    let new_project = run(&[s("new-project"), dir.clone().into_os_string()]);
    assert!(new_project.status.success());
    let identity = run(&[s("identity"), dir.clone().into_os_string()]);
    assert!(identity.status.success());
    let a = identity_hex(&new_project);
    let b = identity_hex(&identity);
    assert_eq!(a, b, "identity must match between new-project and identity");
    let _ = fs::remove_dir_all(&dir);
}

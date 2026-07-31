//! Subprocess integration test for the integrity-failure diagnostic.
//!
//! Covers the production-bound failure path from issue #235:
//!
//! - `threeterm --machine save <bundle> ...` writes a valid bundle.
//! - The test flips a single hex character in
//!   `manifest.json#terminal_log_digest` to deterministically trigger
//!   `LogDigestMismatch`.
//! - `threeterm --machine load <bundle>` exits non-zero, writes a
//!   structured `integrity_failure` diagnostic to stderr, and emits no
//!   stdout.
//! - The host's current state was NOT mutated by the failed load: a fresh
//!   `load` after the manifest is repaired sees the same hashes the
//!   original `save` produced (canonical-state preservation).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static COUNTER: LazyLock<AtomicU64> = LazyLock::new(|| AtomicU64::new(0));

fn unique_bundle_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "threeterm-235-fail-{}-{}-{}",
        std::process::id(),
        label,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create bundle temp dir");
    dir
}

fn run(args: &[&str]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm binary runs")
}

fn parse_success_json(output: &std::process::Output, what: &str) -> Value {
    assert!(
        output.status.success(),
        "{what} exit non-zero: {:?}; stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{what} stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is utf-8");
    serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!("{what} stdout is parseable JSON: {err}; raw={stdout}");
    })
}

fn assert_integrity_failure(output: &std::process::Output, expected_detail: &str, what: &str) {
    assert_eq!(
        output.status.code(),
        Some(threeterm_cli::dispatch::EXIT_INTEGRITY_FAILURE as u32 as i32),
        "{what} should exit {} on integrity failure, got {:?}; stderr={}",
        threeterm_cli::dispatch::EXIT_INTEGRITY_FAILURE,
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "{what} stdout must be empty on diagnostic, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr is utf-8");
    let parsed: Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|err| panic!("{what} stderr is parseable JSON: {err}; raw={stderr}"));

    assert_eq!(
        parsed["code"], "integrity_failure",
        "{what} diagnostic code"
    );
    assert_eq!(parsed["arg"], expected_detail, "{what} diagnostic detail");
    assert_eq!(
        parsed["schema_version"],
        Value::from(threeterm_protocol::schema_version())
    );
}

fn flip_terminal_log_digest(bundle: &Path) -> (String, String) {
    let manifest_path = bundle.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).expect("manifest.json readable");
    let mut value: Value = serde_json::from_str(&raw).expect("manifest.json is parseable JSON");

    let original = value["terminal_log_digest_hex"]
        .as_str()
        .expect("terminal_log_digest_hex is a string")
        .to_string();

    let mut chars: Vec<char> = original.chars().collect();
    let target_idx = chars.iter().position(|c| *c != '0').unwrap_or(0);
    let replacement = match chars[target_idx] {
        '0' => '1',
        '1' => '2',
        c if c.is_ascii_digit() => {
            let next_digit = c.to_digit(10).unwrap() + 1;
            char::from_digit(next_digit, 10).unwrap()
        }
        'a' => 'b',
        'b' => 'c',
        other => other,
    };
    chars[target_idx] = replacement;
    let tampered: String = chars.into_iter().collect();

    *value.get_mut("terminal_log_digest_hex").unwrap() = Value::from(tampered.clone());

    let rewritten =
        serde_json::to_string_pretty(&value).expect("manifest.json re-serializes as JSON");
    std::fs::write(&manifest_path, rewritten).expect("manifest.json rewritten");

    (original, tampered)
}

fn restore_terminal_log_digest(bundle: &Path, original: &str) {
    let manifest_path = bundle.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).expect("manifest.json readable");
    let mut value: Value = serde_json::from_str(&raw).expect("manifest.json is parseable JSON");
    *value.get_mut("terminal_log_digest_hex").unwrap() = Value::from(original);
    let rewritten =
        serde_json::to_string_pretty(&value).expect("manifest.json re-serializes as JSON");
    std::fs::write(&manifest_path, rewritten).expect("manifest.json restored");
}

#[test]
fn tampered_manifest_emits_integrity_failure_diagnostic() {
    let dir = unique_bundle_dir("tamper");
    let bundle = dir.to_str().expect("bundle path is utf-8");

    let save_output = run(&[
        "--machine",
        "save",
        bundle,
        "--feature-id",
        "box-1",
        "--kind",
        "box",
    ]);
    let save_json = parse_success_json(&save_output, "save");

    let saved_graph_hash = save_json["feature_graph_hash"]
        .as_str()
        .expect("save.feature_graph_hash is a string");
    let saved_revision_hash = save_json["revision_hash"]
        .as_str()
        .expect("save.revision_hash is a string");

    let (original_log_digest, _tampered_log_digest) = flip_terminal_log_digest(&dir);

    let load_after_tamper = run(&["--machine", "load", bundle]);
    assert_integrity_failure(
        &load_after_tamper,
        "log_digest_mismatch",
        "load after manifest tamper",
    );

    restore_terminal_log_digest(&dir, &original_log_digest);

    let load_after_repair = run(&["--machine", "load", bundle]);
    let recovered_json = parse_success_json(&load_after_repair, "load after manifest repair");

    assert_eq!(
        recovered_json["feature_graph_hash"]
            .as_str()
            .expect("recovered.feature_graph_hash is a string"),
        saved_graph_hash,
        "feature_graph_hash is recovered after manifest repair"
    );
    assert_eq!(
        recovered_json["revision_hash"]
            .as_str()
            .expect("recovered.revision_hash is a string"),
        saved_revision_hash,
        "revision_hash is recovered after manifest repair"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

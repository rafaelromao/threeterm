use std::ffi::OsString;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_occt_worker::OcctWorker;

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-cli-headless-probe-{nanos}"))
}

fn dispatch(args: Vec<OsString>) -> (i32, Vec<u8>, Vec<u8>) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = threeterm_cli::dispatch::dispatch(args, &mut stdout, &mut stderr);
    (code, stdout, stderr)
}

#[test]
fn cli_bracket_headless_succeeds_even_when_interactive_probe_would_fail() {
    if OcctWorker::locate().is_err() {
        eprintln!("headless_probe_routing: OCCT worker unavailable");
        return;
    }
    // This test proves Headless Automation is unblocked even when the
    // unattached terminal capability probe would refuse Interactive Modeling.
    // The CLI does not require a Ghostty probe — it reuses the same Host
    // canonical path. The probe-failure part is exercised in threeterm-tui
    // capability_headless tests; here we prove the same bundle's headless
    // bracket via CLI dispatch succeeds.
    let root = temporary_bundle_root();

    // Same bundle: CLI bracket via headless dispatch succeeds despite probe failure.
    let bundle = root.to_string_lossy().to_string();
    let args = vec![
        OsString::from("--machine"),
        OsString::from("bracket"),
        OsString::from(&bundle),
        OsString::from("--bracket-id"),
        OsString::from("bracket-cli"),
        OsString::from("--length"),
        OsString::from("50"),
        OsString::from("--width"),
        OsString::from("20"),
        OsString::from("--height"),
        OsString::from("20"),
        OsString::from("--thickness"),
        OsString::from("5"),
    ];
    let (code, stdout, stderr) = dispatch(args);
    assert_eq!(
        code,
        0,
        "headless bracket via CLI must succeed: stderr={}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty(), "stderr empty on success");
    assert!(!stdout.is_empty(), "stdout carries machine JSON");
    let parsed: serde_json::Value = serde_json::from_slice(&stdout).expect("stdout is JSON");
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.bracket.response/1"
    );
    assert!(parsed["revision_hash"].as_str().is_some());
    assert!(parsed["feature_graph_hash"].as_str().is_some());
    // Cleanup
    std::fs::remove_dir_all(root).expect("bundle removed");
}

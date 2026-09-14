use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_occt_worker::{BracketRequest, OcctWorker};

#[test]
#[ignore = "requires the qualified graphical Ghostty toolchain"]
fn production_tui_ghostty_session() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-graphical-launch-{}-{suffix}",
        std::process::id()
    ));
    let evidence = root.join("evidence");
    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("graphical launch requires the OCCT worker: {error}"));
    Host::new()
        .create_bracket(
            &root,
            BracketRequest::new("graphical-l-bracket", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("l-bracket"),
            &worker,
        )
        .expect("the graphical launch starts from a real L-bracket Project Generation");

    let runner =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/graphical-tui.sh");
    let output = Command::new("bash")
        .arg(runner)
        .arg("production_tui_ghostty_session")
        .arg("--tui-binary")
        .arg(env!("CARGO_BIN_EXE_threeterm-tui"))
        .arg("--project-root")
        .arg(&root)
        .arg("--evidence-root")
        .arg(&evidence)
        .output()
        .expect("graphical runner starts");
    assert!(
        output.status.success(),
        "graphical runner failed: stdout={} stderr={} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        evidence.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(evidence.join("manifest.json")).expect("graphical evidence manifest exists"),
    )
    .expect("graphical evidence manifest is JSON");
    assert_eq!(manifest["schema_version"], "threeterm.graphical-tui/1");
    assert_eq!(manifest["result"], "passed");
    assert_eq!(manifest["test"], "production_tui_ghostty_session");
    assert_eq!(manifest["events"]["probe"], "passed");
    assert_eq!(manifest["events"]["readiness"], "passed");
    assert_eq!(manifest["events"]["orbit"], "passed");
    assert_eq!(manifest["events"]["cleanup"], "passed");
    assert_eq!(manifest["processes"]["owned"], "stopped");
    assert!(manifest["artifacts"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["kind"] == "startup_screenshot")
            && items
                .iter()
                .any(|item| item["kind"] == "cleanup_screenshot")
            && items.iter().all(|item| item["sha256"].as_str().is_some())
    }));
}

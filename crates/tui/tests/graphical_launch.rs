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
    assert_eq!(manifest["configuration"]["palette"], "catppuccin");
    let startup = &manifest["viewport"]["startup"];
    let orbit = &manifest["viewport"]["orbit"];
    assert_eq!(startup["schema_version"], "threeterm.viewport-evidence/1");
    assert_eq!(startup["frame"]["width"], 800);
    assert_eq!(startup["frame"]["height"], 480);
    assert_eq!(startup["palette"]["name"], "catppuccin");
    assert_eq!(
        startup["palette"]["colors"]["body"],
        serde_json::json!([125, 125, 152])
    );
    assert!(startup["scene"]["solids"].as_array().is_some_and(|solids| {
        solids.iter().any(|solid| {
            solid["feature_id"] == "l-bracket" && solid["triangle_count"].as_u64().unwrap_or(0) > 0
        })
    }));
    assert!(startup["scene"]["body_pixels"].as_u64().unwrap_or(0) > 0);
    assert_ne!(startup["frame"]["image_id"], orbit["frame"]["image_id"]);
    assert_ne!(
        startup["camera"]["yaw_degrees"],
        orbit["camera"]["yaw_degrees"]
    );
    assert_eq!(startup["frame"]["revision"], orbit["frame"]["revision"]);
    assert_eq!(
        manifest["cleanup_evidence"]["final_image_id"],
        manifest["cleanup_evidence"]["final_delete_image_id"]
    );
    assert!(
        manifest["cleanup_evidence"]["deletions"]
            .as_array()
            .is_some_and(|deletions| deletions.len() >= 2)
    );
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

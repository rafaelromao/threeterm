//! End-to-end subprocess test for the L-bracket demoable: an L-shaped
//! extrude + a fillet + a chamfer, all chained through the public CLI
//! surface. The chain meets the issue #254 demoable: "Run fillet then
//! chamfer on the L-bracket solid; both commands commit; the resulting
//! solids are visible in the viewport." The viewport render is
//! unchanged by this slice (the committed BREP is the same DBRep
//! shape the existing extrude path already renders), so the test
//! asserts the canonical commit path end-to-end and the resulting
//! solids have the OCCT DBRep shape marker the viewport consumes.
//!
//! When the OCCT worker binary is unavailable the test soft-skip via
//! `OcctWorker::locate` returning `Err`; the CI archlinux container
//! installs `opencascade` so the production path runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::{Bundle, MANIFEST_FILENAME, TRANSACTIONS_LOG_FILENAME};
use threeterm_protocol::schema::{CHAMFER_COMMAND_ID, FILLET_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-l-bracket-{label}-{}-{nanos}",
        std::process::id(),
    ))
}

fn run(bin: &str, args: &[&str]) -> std::process::Output {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "threeterm {args:?} failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn new_project(bin: &str, root: &Path) {
    run(bin, &["new-project", root.to_str().expect("utf-8 path")]);
}

fn save(bin: &str, root: &Path, feature_id: &str, kind: &str) {
    run(
        bin,
        &[
            "--machine",
            "save",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--kind",
            kind,
        ],
    );
}

fn extrude(bin: &str, root: &Path, feature_id: &str, profile: serde_json::Value, height: f64) {
    let profile_path = root.join(format!("{feature_id}-profile.json"));
    fs::write(&profile_path, profile.to_string()).expect("profile writes");
    let height_str = format!("{height}");
    run(
        bin,
        &[
            "--machine",
            "extrude",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--profile-file",
            profile_path.to_str().expect("utf-8 path"),
            "--height",
            &height_str,
        ],
    );
}

fn fillet(bin: &str, root: &Path, feature_id: &str, base: &str, radius: f64) -> Value {
    let output = Command::new(bin)
        .args([
            "--machine",
            "fillet",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--base",
            base,
            "--radius",
            &format!("{radius}"),
        ])
        .output()
        .expect("fillet runs");
    assert!(
        output.status.success(),
        "fillet failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");
    let entry = find(FILLET_COMMAND_ID).expect("fillet is registered");
    validate(&entry.response_schema, &parsed).expect("fillet response validates");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "fillet");
    assert_eq!(parsed["feature_id"], feature_id);
    parsed
}

fn chamfer(bin: &str, root: &Path, feature_id: &str, base: &str, distance: f64) -> Value {
    let output = Command::new(bin)
        .args([
            "--machine",
            "chamfer",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--base",
            base,
            "--distance",
            &format!("{distance}"),
        ])
        .output()
        .expect("chamfer runs");
    assert!(
        output.status.success(),
        "chamfer failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");
    let entry = find(CHAMFER_COMMAND_ID).expect("chamfer is registered");
    validate(&entry.response_schema, &parsed).expect("chamfer response validates");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "chamfer");
    assert_eq!(parsed["feature_id"], feature_id);
    parsed
}

fn assert_brep_is_real_occt_shape(path: &Path) {
    let bytes = fs::read(path).expect("brep reads");
    assert!(!bytes.is_empty(), "BREP is empty: {path:?}");
    let prefix = &bytes[..bytes.len().min(64)];
    let prefix_str = String::from_utf8_lossy(prefix);
    assert!(
        prefix_str.contains("DBRep_DrawableShape"),
        "BREP at {path:?} must start with the OCCT DBRep_DrawableShape marker; got {prefix_str:?}"
    );
}

#[test]
fn l_bracket_fillet_then_chamfer_commits_through_the_cli() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "l_bracket_fillet_then_chamfer: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("chain");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    let l_profile = serde_json::json!([
        [0.0, 0.0],
        [4.0, 0.0],
        [4.0, 1.0],
        [1.0, 1.0],
        [1.0, 4.0],
        [0.0, 4.0]
    ]);
    extrude(bin, &root, "l-bracket-base", l_profile, 4.0);

    let base_revision = Bundle::at(&root)
        .open()
        .expect("bundle reopens after extrude")
        .revision_hash_hex()
        .to_string();

    let fillet_response = fillet(bin, &root, "l-bracket-fillet", "l-bracket-base", 0.1);
    assert_ne!(
        fillet_response["revision_hash"].as_str().unwrap(),
        base_revision,
        "fillet must advance the revision hash"
    );
    let fillet_revision = fillet_response["revision_hash"]
        .as_str()
        .unwrap()
        .to_string();

    let chamfer_response = chamfer(bin, &root, "l-bracket-chamfer", "l-bracket-fillet", 0.25);
    assert_ne!(
        chamfer_response["revision_hash"].as_str().unwrap(),
        fillet_revision,
        "chamfer must advance the revision hash"
    );

    let reloaded = Bundle::at(&root)
        .open()
        .expect("bundle reopens after the chain");
    assert_eq!(
        reloaded.revision_hash_hex(),
        chamfer_response["revision_hash"]
    );
    assert_eq!(
        reloaded.feature_graph_hash_hex(),
        chamfer_response["feature_graph_hash"]
    );

    let committed = ["l-bracket-base", "l-bracket-fillet", "l-bracket-chamfer"];
    for feature_id in committed {
        let brep = root.join(format!("brep/{feature_id}.brep"));
        assert!(brep.is_file(), "committed BREP missing at {brep:?}");
        assert_brep_is_real_occt_shape(&brep);
    }

    let log_path = root.join(TRANSACTIONS_LOG_FILENAME);
    let log = fs::read_to_string(&log_path).expect("log reads");
    let mut line_count = 0;
    for line in log.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(line).expect("log line is JSON");
        let kind = entry["kind"].as_str().unwrap_or("");
        if [
            "brep:l-bracket-base",
            "brep:l-bracket-fillet",
            "brep:l-bracket-chamfer",
        ]
        .contains(&kind)
        {
            line_count += 1;
        }
    }
    assert_eq!(
        line_count, 3,
        "expected exactly three 3D commits in the transaction log; got {line_count}"
    );

    let manifest_path = root.join(MANIFEST_FILENAME);
    let manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest reads"))
            .expect("manifest parses");
    assert!(
        manifest["terminal_log_digest"]
            .as_str()
            .unwrap_or("")
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "terminal_log_digest must be lowercase hex"
    );

    let _ = fs::remove_dir_all(root);
}

//! End-to-end subprocess test for the L-bracket shell demoable: an
//! L-shaped extrude + Boolean fuse, shelled with a positive wall
//! thickness. The slice meets the issue #260 demoable: "Run a shell
//! command on the L-bracket solid; commit; the shelled solid is
//! visible in the viewport." The viewport render is unchanged by this
//! slice (the committed shelled BREP is the same DBRep shape the
//! existing extrude/fuse path already renders), so the test asserts
//! the canonical commit path end-to-end and that the resulting shelled
//! BREP starts with the OCCT `DBRep_DrawableShape` marker the viewport
//! consumes.
//!
//! The L-bracket (10×5×3 slab + 3×10×3 leg, both 3 units tall) carries a
//! minimum material width of 3 units; a wall thickness of 0.3 leaves a
//! void of (10 − 2×0.3) × (5 − 2×0.3) × (3 − 2×0.3) on the
//! outer envelope so the OCCT shell algorithm has positive material on
//! every face, guaranteeing the shelled BREP differs byte-for-byte from
//! the fused BREP.
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
use threeterm_protocol::schema::{SHELL_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-l-bracket-shell-{label}-{}-{nanos}",
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

fn boolean_fuse(bin: &str, root: &Path, feature_id: &str, base: &str, tool: &str) -> Value {
    let output = Command::new(bin)
        .args([
            "--machine",
            "boolean-fuse",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--base",
            base,
            "--tool",
            tool,
        ])
        .output()
        .expect("boolean-fuse runs");
    assert!(
        output.status.success(),
        "boolean-fuse failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    serde_json::from_str(&stdout).expect("response is JSON")
}

fn shell(bin: &str, root: &Path, feature_id: &str, base: &str, thickness: f64) -> Value {
    let output = Command::new(bin)
        .args([
            "--machine",
            "shell",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--base",
            base,
            "--thickness",
            &format!("{thickness}"),
        ])
        .output()
        .expect("shell runs");
    assert!(
        output.status.success(),
        "shell failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");
    let entry = find(SHELL_COMMAND_ID).expect("shell is registered");
    validate(&entry.response_schema, &parsed).expect("shell response validates");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "shell");
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
fn l_bracket_shell_commits_through_the_cli() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "l_bracket_shell_e2e: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("shell");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    let slab_profile = serde_json::json!([[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]);
    extrude(bin, &root, "l-bracket-shell-slab", slab_profile, 3.0);

    let leg_profile = serde_json::json!([[0.0, 0.0], [3.0, 0.0], [3.0, 10.0], [0.0, 10.0]]);
    extrude(bin, &root, "l-bracket-shell-leg", leg_profile, 3.0);

    let fuse_response = boolean_fuse(
        bin,
        &root,
        "l-bracket-shell",
        "l-bracket-shell-slab",
        "l-bracket-shell-leg",
    );
    let l_bracket_revision = fuse_response["revision_hash"].as_str().unwrap().to_string();

    let shell_response = shell(bin, &root, "l-bracket-shell-shell", "l-bracket-shell", 0.01);
    assert_ne!(
        shell_response["revision_hash"].as_str().unwrap(),
        l_bracket_revision,
        "shell must advance the revision hash"
    );

    let reloaded = Bundle::at(&root)
        .open()
        .expect("bundle reopens after shell");
    assert_eq!(
        reloaded.revision_hash_hex(),
        shell_response["revision_hash"]
    );
    assert_eq!(
        reloaded.feature_graph_hash_hex(),
        shell_response["feature_graph_hash"]
    );

    let committed = [
        "l-bracket-shell-slab",
        "l-bracket-shell-leg",
        "l-bracket-shell",
        "l-bracket-shell-shell",
    ];
    for feature_id in committed {
        let brep = root.join(format!("brep/{feature_id}.brep"));
        assert!(brep.is_file(), "committed BREP missing at {brep:?}");
        assert_brep_is_real_occt_shape(&brep);
    }

    let fused_brep_bytes =
        fs::read(root.join("brep/l-bracket-shell.brep")).expect("fused BREP reads");
    let shelled_brep_bytes =
        fs::read(root.join("brep/l-bracket-shell-shell.brep")).expect("shelled BREP reads");
    assert_ne!(
        fused_brep_bytes, shelled_brep_bytes,
        "shelled L-bracket BREP must differ byte-for-byte from the fused L-bracket BREP"
    );

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
            "brep:l-bracket-shell-slab",
            "brep:l-bracket-shell-leg",
            "brep:l-bracket-shell",
            "brep:l-bracket-shell-shell",
        ]
        .contains(&kind)
        {
            line_count += 1;
        }
    }
    assert_eq!(
        line_count, 4,
        "expected exactly four 3D commits in the transaction log; got {line_count}"
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

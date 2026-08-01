//! End-to-end subprocess test for the L-bracket linear pattern demoable:
//! the L-bracket solid (extrude + Boolean fuse) patterned along the +X
//! axis with a non-trivial count. The slice meets the issue #258
//! demoable: "Run a linear pattern on the L-bracket solid; commit; the
//! patterned solid is visible in the viewport." The viewport render is
//! unchanged by this slice (the committed patterned BREP is the same
//! DBRep shape the existing extrude/fuse path already renders), so the
//! test asserts the canonical commit path end-to-end and the
//! resulting patterned BREP starts with the OCCT `DBRep_DrawableShape`
//! marker the viewport consumes.
//!
//! Pattern direction (1, 0, 0) and spacing 12.0 with count 3 places the
//! copies at x ∈ [0, 10] (source), x ∈ [12, 22], and x ∈ [24, 34]; the
//! non-overlapping copies guarantee the patterned BREP differs
//! byte-for-byte from the source L-bracket.
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
use threeterm_protocol::schema::{LINEAR_PATTERN_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-l-bracket-linear-pattern-{label}-{}-{nanos}",
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

fn linear_pattern(
    bin: &str,
    root: &Path,
    feature_id: &str,
    base: &str,
    direction: &str,
    count: u32,
    spacing: f64,
) -> Value {
    let output = Command::new(bin)
        .args([
            "--machine",
            "linear-pattern",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--base",
            base,
            "--direction",
            direction,
            "--count",
            &count.to_string(),
            "--spacing",
            &format!("{spacing}"),
        ])
        .output()
        .expect("linear-pattern runs");
    assert!(
        output.status.success(),
        "linear-pattern failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");
    let entry = find(LINEAR_PATTERN_COMMAND_ID).expect("linear-pattern is registered");
    validate(&entry.response_schema, &parsed).expect("linear-pattern response validates");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "linear_pattern");
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
fn l_bracket_linear_pattern_commits_through_the_cli() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "l_bracket_linear_pattern_e2e: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("pattern");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    let slab_profile = serde_json::json!([[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]);
    extrude(
        bin,
        &root,
        "l-bracket-linear-pattern-slab",
        slab_profile,
        3.0,
    );

    let leg_profile = serde_json::json!([[0.0, 0.0], [3.0, 0.0], [3.0, 10.0], [0.0, 10.0]]);
    extrude(bin, &root, "l-bracket-linear-pattern-leg", leg_profile, 3.0);

    let fuse_response = boolean_fuse(
        bin,
        &root,
        "l-bracket-linear-pattern",
        "l-bracket-linear-pattern-slab",
        "l-bracket-linear-pattern-leg",
    );
    let l_bracket_revision = fuse_response["revision_hash"].as_str().unwrap().to_string();

    let pattern_response = linear_pattern(
        bin,
        &root,
        "l-bracket-linear-pattern-pattern",
        "l-bracket-linear-pattern",
        "1.0,0.0,0.0",
        3,
        12.0,
    );
    assert_ne!(
        pattern_response["revision_hash"].as_str().unwrap(),
        l_bracket_revision,
        "linear pattern must advance the revision hash"
    );

    let reloaded = Bundle::at(&root)
        .open()
        .expect("bundle reopens after linear pattern");
    assert_eq!(
        reloaded.revision_hash_hex(),
        pattern_response["revision_hash"]
    );
    assert_eq!(
        reloaded.feature_graph_hash_hex(),
        pattern_response["feature_graph_hash"]
    );

    let committed = [
        "l-bracket-linear-pattern-slab",
        "l-bracket-linear-pattern-leg",
        "l-bracket-linear-pattern",
        "l-bracket-linear-pattern-pattern",
    ];
    for feature_id in committed {
        let brep = root.join(format!("brep/{feature_id}.brep"));
        assert!(brep.is_file(), "committed BREP missing at {brep:?}");
        assert_brep_is_real_occt_shape(&brep);
    }

    let fused_brep_bytes =
        fs::read(root.join("brep/l-bracket-linear-pattern.brep")).expect("fused BREP reads");
    let patterned_brep_bytes = fs::read(root.join("brep/l-bracket-linear-pattern-pattern.brep"))
        .expect("patterned BREP reads");
    assert_ne!(
        fused_brep_bytes, patterned_brep_bytes,
        "patterned L-bracket BREP must differ byte-for-byte from the fused L-bracket BREP"
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
            "brep:l-bracket-linear-pattern-slab",
            "brep:l-bracket-linear-pattern-leg",
            "brep:l-bracket-linear-pattern",
            "brep:l-bracket-linear-pattern-pattern",
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

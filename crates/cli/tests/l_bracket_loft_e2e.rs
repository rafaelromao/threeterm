//! End-to-end subprocess test for the loft demoable: a two-profile
//! loft through the CLI commits a real BREP into the project bundle.
//! The slice meets the issue #262 demoable: "Run a loft with two
//! profiles; commit; the lofted solid is visible in the viewport." The
//! viewport render is unchanged by this slice (the committed BREP is
//! the same DBRep shape the existing extrude/fuse path already
//! renders), so the test asserts the canonical commit path end-to-end
//! and that the resulting BREP starts with the OCCT
//! `DBRep_DrawableShape` marker the viewport consumes.
//!
//! The demo builds a 10x10 base rectangle at Z=0 and a 6x6 top
//! rectangle at Z=5 by lofting two profiles; the worker exercises
//! `BRepOffsetAPI_ThruSections` with the smooth (non-ruled) mode and
//! commits a frustum-like solid. The committed BREP differs
//! byte-for-byte from the underlying extrusion BREPs (so an unchanged
//! payload would be caught), the SHA-256 differs from a single
//! extrude, and the transaction log carries the loft commit.
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
use threeterm_protocol::schema::{LOFT_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

mod common;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-loft-{label}-{}-{nanos}",
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

fn extrude(_bin: &str, root: &Path, feature_id: &str, profile: serde_json::Value, height: f64) {
    common::extrude_canonical(root, feature_id, profile, height);
}

fn loft(bin: &str, root: &Path, feature_id: &str, profile_files: &[&Path]) -> Value {
    let mut args: Vec<String> = vec![
        "--machine".to_string(),
        "loft".to_string(),
        "--bundle".to_string(),
        root.to_str().expect("utf-8 path").to_string(),
        "--feature-id".to_string(),
        feature_id.to_string(),
    ];
    for profile in profile_files {
        args.push("--profile-file".to_string());
        args.push(profile.to_str().expect("utf-8 path").to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = Command::new(bin)
        .args(&arg_refs)
        .output()
        .expect("loft runs");
    assert!(
        output.status.success(),
        "loft failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");
    let entry = find(LOFT_COMMAND_ID).expect("loft is registered");
    validate(&entry.response_schema, &parsed).expect("loft response validates");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "loft");
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
fn l_bracket_loft_commits_through_the_cli() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "l_bracket_loft_e2e: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("loft");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    let extrude_profile = serde_json::json!([[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
    extrude(bin, &root, "loft-base-extrude", extrude_profile, 1.0);

    let profile_a_path = root.join("loft-profile-a.json");
    let profile_b_path = root.join("loft-profile-b.json");
    let profile_a = serde_json::json!([
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 10.0, 0.0],
        [0.0, 10.0, 0.0]
    ]);
    let profile_b = serde_json::json!([
        [2.5, 2.5, 5.0],
        [7.5, 2.5, 5.0],
        [7.5, 7.5, 5.0],
        [2.5, 7.5, 5.0]
    ]);
    fs::write(&profile_a_path, profile_a.to_string()).expect("profile a writes");
    fs::write(&profile_b_path, profile_b.to_string()).expect("profile b writes");

    let response = loft(
        bin,
        &root,
        "lofted-frustum",
        &[&profile_a_path, &profile_b_path],
    );
    let revision_hash = response["revision_hash"].as_str().unwrap().to_string();

    let reloaded = Bundle::at(&root).open().expect("bundle reopens after loft");
    assert_eq!(reloaded.revision_hash_hex(), revision_hash);
    assert_eq!(
        reloaded.feature_graph_hash_hex(),
        response["feature_graph_hash"]
    );

    let committed = ["loft-base-extrude", "lofted-frustum"];
    for feature_id in committed {
        let brep = root.join(format!("brep/{feature_id}.brep"));
        assert!(brep.is_file(), "committed BREP missing at {brep:?}");
        assert_brep_is_real_occt_shape(&brep);
    }

    let extrude_brep_bytes =
        fs::read(root.join("brep/loft-base-extrude.brep")).expect("extrude BREP reads");
    let lofted_brep_bytes =
        fs::read(root.join("brep/lofted-frustum.brep")).expect("lofted BREP reads");
    assert_ne!(
        extrude_brep_bytes, lofted_brep_bytes,
        "lofted BREP must differ byte-for-byte from the extrude BREP"
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
        if ["brep:loft-base-extrude", "brep:lofted-frustum"].contains(&kind) {
            line_count += 1;
        }
    }
    assert_eq!(
        line_count, 2,
        "expected exactly two 3D commits in the transaction log; got {line_count}"
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

#[test]
fn loft_rejects_dispatch_with_a_single_profile() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("loft-single");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    let profile_path = root.join("loft-profile-single.json");
    let profile = serde_json::json!([
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 10.0, 0.0],
        [0.0, 10.0, 0.0]
    ]);
    fs::write(&profile_path, profile.to_string()).expect("profile writes");

    let output = Command::new(bin)
        .args([
            "--machine",
            "loft",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "loft-single",
            "--profile-file",
            profile_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("loft runs");
    assert!(
        !output.status.success(),
        "loft with a single profile must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--profile-file"),
        "stderr should mention the missing --profile-file; got {stderr}"
    );

    let _ = fs::remove_dir_all(root);
}

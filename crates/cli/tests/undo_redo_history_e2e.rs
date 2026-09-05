use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_occt_worker::OcctWorker;

fn temp_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-undo-redo-e2e-{label}-{suffix}"))
}

fn run(bin: &str, args: &[&str]) -> Value {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "command {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("response is JSON")
}

fn export_stl(bin: &str, root: &Path, output: &Path) -> PathBuf {
    let response = run(
        bin,
        &[
            "--machine",
            "export",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "l-bracket",
            "--formats",
            "stl",
            "--output-dir",
            output.to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(response["status"], "ok");
    let artifact = output.join("l-bracket.stl");
    assert!(artifact.is_file(), "STL export is on disk");
    artifact
}

#[test]
fn undo_redo_and_historical_edit_move_geometry_on_the_production_path() {
    if OcctWorker::locate().is_err() {
        eprintln!("undo_redo_history_e2e: OCCT worker unavailable");
        return;
    }
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("bundle");
    let output = temp_root("output");

    let created = run(
        bin,
        &[
            "--machine",
            "bracket",
            root.to_str().expect("utf-8 path"),
            "--bracket-id",
            "l-bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    assert_eq!(created["status"], "ok");
    assert_eq!(created["feature_id"], "l-bracket");
    let initial_stl = fs::read(export_stl(bin, &root, &output)).expect("initial STL reads");

    let edited = run(
        bin,
        &[
            "--machine",
            "historical-edit",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "l-bracket-base",
            "--parameter",
            "length",
            "--value",
            "61",
        ],
    );
    assert_eq!(edited["status"], "ok");
    assert_eq!(edited["active_revision"], "history-revision-2");
    let edited_stl = fs::read(export_stl(bin, &root, &output)).expect("edited STL reads");
    assert_ne!(
        edited_stl, initial_stl,
        "a successful historical edit changes exported current geometry"
    );

    let undone = run(
        bin,
        &["--machine", "undo", root.to_str().expect("utf-8 path")],
    );
    assert_eq!(undone["operation"], "undo");
    assert_eq!(undone["active_revision"], "history-revision-1");
    let undone_stl = fs::read(export_stl(bin, &root, &output)).expect("undone STL reads");
    assert_eq!(
        undone_stl, initial_stl,
        "undo restores the prior rendered and exported geometry"
    );

    let redone = run(
        bin,
        &["--machine", "redo", root.to_str().expect("utf-8 path")],
    );
    assert_eq!(redone["operation"], "redo");
    assert_eq!(redone["active_revision"], "history-revision-2");
    let redone_stl = fs::read(export_stl(bin, &root, &output)).expect("redone STL reads");
    assert_eq!(
        redone_stl, edited_stl,
        "redo restores the edited rendered and exported geometry"
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output);
}

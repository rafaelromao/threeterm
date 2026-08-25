//! Production subprocess coverage for export of a fused L-bracket.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_occt_worker::OcctWorker;
use threeterm_tui::TuiSession;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-export-{label}-{}-{nanos}",
        std::process::id()
    ))
}

fn run(bin: &str, args: &[&str]) {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_value(bin: &str, args: &[&str]) -> Value {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("response is JSON")
}

fn run_failed_value(bin: &str, args: &[&str]) -> Value {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).expect("diagnostic is JSON")
}

fn extrude(bin: &str, bundle: &Path, id: &str, profile: &str) {
    let path = bundle.join(format!("{id}.json"));
    fs::write(&path, profile).unwrap();
    run(
        bin,
        &[
            "--machine",
            "extrude",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            id,
            "--profile-file",
            path.to_str().unwrap(),
            "--height",
            "4",
        ],
    );
}

fn l_bracket(bin: &str, bundle: &Path) {
    run(bin, &["new-project", bundle.to_str().unwrap()]);
    extrude(bin, bundle, "vertical", "[[0,0],[4,0],[4,24],[0,24]]");
    extrude(bin, bundle, "horizontal", "[[0,0],[28,0],[28,4],[0,4]]");
    run(
        bin,
        &[
            "--machine",
            "boolean-fuse",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "l-bracket",
            "--base",
            "vertical",
            "--tool",
            "horizontal",
        ],
    );
}

#[test]
fn export_cli_writes_stl_3mf_and_step_for_a_fused_l_bracket() {
    if OcctWorker::locate().is_err() {
        eprintln!("export_e2e: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = temp_root("bundle");
    let output = temp_root("output");
    l_bracket(bin, &bundle);
    run(
        bin,
        &[
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "l-bracket",
            "--formats",
            "stl,3mf,step",
            "--output-dir",
            output.to_str().unwrap(),
        ],
    );

    let stl = fs::read(output.join("l-bracket.stl")).unwrap();
    let three_mf = fs::read(output.join("l-bracket.3mf")).unwrap();
    let step = fs::read(output.join("l-bracket.step")).unwrap();
    assert!(stl.starts_with(b"solid"));
    assert!(three_mf.starts_with(b"PK\x03\x04"));
    assert!(step.starts_with(b"ISO-10303-21"));
    let _ = fs::remove_dir_all(bundle);
    let _ = fs::remove_dir_all(output);
}

#[test]
fn export_warning_requires_an_explicit_override() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = temp_root("warning");
    run(bin, &["new-project", bundle.to_str().unwrap()]);
    let output = Command::new(bin)
        .args([
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "missing",
            "--formats",
            "stl",
            "--output-dir",
            bundle.to_str().unwrap(),
            "--tessellation-deflection",
            "1",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("coarse_tessellation") && stderr.contains("override_eligible"));
    let _ = fs::remove_dir_all(bundle);
}

#[test]
fn export_rejects_duplicate_formats_before_staging() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = temp_root("duplicate-format");
    run(bin, &["new-project", bundle.to_str().unwrap()]);
    let output = Command::new(bin)
        .args([
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "missing",
            "--formats",
            "stl,stl",
            "--output-dir",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicate export format"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(bundle);
}

#[test]
fn export_warning_override_allows_the_request_to_continue() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = temp_root("override");
    run(bin, &["new-project", bundle.to_str().unwrap()]);
    let output = Command::new(bin)
        .args([
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "missing",
            "--formats",
            "stl",
            "--output-dir",
            bundle.to_str().unwrap(),
            "--tessellation-deflection",
            "1",
            "--override-warnings",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("coarse_tessellation"));
    let _ = fs::remove_dir_all(bundle);
}

#[test]
fn fatal_export_leaves_bundle_and_output_unchanged() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = temp_root("fatal");
    let output = temp_root("fatal-output");
    run(bin, &["new-project", bundle.to_str().unwrap()]);
    let manifest = fs::read(bundle.join("manifest.json")).unwrap();
    let result = Command::new(bin)
        .args([
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "missing",
            "--formats",
            "stl,3mf,step",
            "--output-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(bundle.join("manifest.json")).unwrap(), manifest);
    assert!(
        !output.join("missing.stl").exists()
            && !output.join("missing.3mf").exists()
            && !output.join("missing.step").exists()
    );
    let _ = fs::remove_dir_all(bundle);
    let _ = fs::remove_dir_all(output);
}

#[test]
fn export_artifacts_have_required_3mf_structure_and_preserve_generations() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = temp_root("validate");
    let output = temp_root("validate-output");
    l_bracket(bin, &bundle);
    let current = fs::read(bundle.join("manifest.json")).unwrap();
    let previous = bundle.with_extension("previous");
    let previous_manifest = if previous.join("manifest.json").is_file() {
        Some(fs::read(previous.join("manifest.json")).unwrap())
    } else {
        None
    };
    run(
        bin,
        &[
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "l-bracket",
            "--formats",
            "stl,3mf,step",
            "--output-dir",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(fs::read(bundle.join("manifest.json")).unwrap(), current);
    if let Some(previous_manifest) = previous_manifest {
        assert_eq!(
            fs::read(previous.join("manifest.json")).unwrap(),
            previous_manifest
        );
    }
    let archive = fs::read(output.join("l-bracket.3mf")).unwrap();
    for path in [
        b"[Content_Types].xml".as_slice(),
        b"_rels/.rels",
        b"3D/3dmodel.model",
    ] {
        assert!(archive.windows(path.len()).any(|window| window == path));
    }
    let _ = fs::remove_dir_all(bundle);
    let _ = fs::remove_dir_all(output);
}

#[test]
fn stale_geometry_is_observable_across_cli_reload_tui_and_export_gate() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let bundle = temp_root("stale-history");
    let output = temp_root("stale-history-output");
    let worker_available = OcctWorker::locate().is_ok();
    if worker_available {
        l_bracket(bin, &bundle);
    } else {
        run(bin, &["new-project", bundle.to_str().unwrap()]);
    }
    run_value(
        bin,
        &[
            "--machine",
            "bracket",
            bundle.to_str().unwrap(),
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
    let edited = run_value(
        bin,
        &[
            "--machine",
            "historical-edit",
            bundle.to_str().unwrap(),
            "--feature-id",
            "l-bracket-base",
            "--parameter",
            "length",
            "--value",
            "0",
        ],
    );
    assert_eq!(edited["status"], "degraded");
    assert!(
        edited["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|feature| feature["last_valid_geometry_fingerprint"]
                .as_str()
                .is_some_and(|value| !value.is_empty()))
    );
    assert_eq!(
        edited["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|feature| feature["id"] == "l-bracket-base")
            .unwrap()["stale_last_valid_geometry"],
        true
    );

    let host = Host::new();
    let before = host.load(&bundle).expect("canonical state reloads");
    let history = host.history(&bundle).expect("history reloads");
    let mut tui = TuiSession::new([], "before-refresh");
    tui.refresh_stale_geometry(&history, "l-bracket");
    let stale_overlay = tui
        .stale_geometry_overlay()
        .expect("TUI exposes stale marker");
    assert!(stale_overlay.contains("stale-last-valid-geometry"));
    assert!(stale_overlay.contains("l-bracket-base"));

    let manifest = fs::read(bundle.join("manifest.json")).expect("manifest reads");
    let log = fs::read(bundle.join("transactions.log")).expect("log reads");
    let brep =
        worker_available.then(|| fs::read(bundle.join("brep/l-bracket.brep")).expect("BREP reads"));
    let refused = run_failed_value(
        bin,
        &[
            "--machine",
            "export",
            "--bundle",
            bundle.to_str().unwrap(),
            "--feature-id",
            "l-bracket",
            "--formats",
            "stl",
            "--output-dir",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(refused["code"], "stale_last_valid_geometry");
    assert_eq!(refused["feature_id"], "l-bracket");
    assert_eq!(refused["stale_features"].as_array().unwrap().len(), 3);
    assert!(
        refused["recovery"]
            .as_str()
            .unwrap()
            .contains("accept-stale-geometry")
    );
    assert!(!output.exists());
    assert_eq!(fs::read(bundle.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(bundle.join("transactions.log")).unwrap(), log);
    if let Some(brep) = &brep {
        assert_eq!(
            fs::read(bundle.join("brep/l-bracket.brep")).unwrap(),
            brep.as_slice()
        );
    }
    assert_eq!(host.current(), Some(before.clone()));

    if worker_available {
        let accepted = run_value(
            bin,
            &[
                "--machine",
                "export",
                "--bundle",
                bundle.to_str().unwrap(),
                "--feature-id",
                "l-bracket",
                "--formats",
                "stl",
                "--output-dir",
                output.to_str().unwrap(),
                "--accept-stale-geometry",
            ],
        );
        assert_eq!(accepted["accepted_stale_geometry"], true);
        assert_eq!(accepted["stale_geometry"]["feature_id"], "l-bracket");
        assert!(output.join("l-bracket.stl").is_file());
        assert_eq!(fs::read(bundle.join("manifest.json")).unwrap(), manifest);
        assert_eq!(fs::read(bundle.join("transactions.log")).unwrap(), log);
        assert_eq!(host.current(), Some(before));
    }

    let _ = fs::remove_dir_all(bundle);
    let _ = fs::remove_dir_all(output);
}

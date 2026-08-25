//! Production subprocess coverage for export of a fused L-bracket.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_occt_worker::OcctWorker;

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

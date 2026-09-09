//! End-to-end integration test for the Project Generation identity
//! invariant on the production code path.
//!
//! Demoable behavior: chain the full MVP operation set on a project
//! through the `threeterm` binary, reload from disk, and assert the
//! Project Generation identity is byte-equal across the round-trip.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_occt_worker::OcctWorker;

fn unique_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-identity-{label}-{suffix}"));
    let _ = fs::remove_dir_all(&root);
    root
}

fn run(args: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(args)
        .output()
        .expect("threeterm binary runs");
    assert!(
        output.status.success(),
        "threeterm {args:?} exited with status {:?}\n  stdout: {}\n  stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn read_manifest_generation_id(root: &Path) -> String {
    let manifest_path = root.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path).expect("manifest is readable");
    let value: Value = serde_json::from_str(&raw).expect("manifest is JSON");
    value["generation_id"]
        .as_str()
        .expect("manifest.generation_id is a string")
        .to_string()
}

fn read_manifest_terminal_log_digest(root: &Path) -> String {
    let manifest_path = root.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path).expect("manifest is readable");
    let value: Value = serde_json::from_str(&raw).expect("manifest is JSON");
    value["terminal_log_digest"]
        .as_str()
        .expect("manifest.terminal_log_digest is a string")
        .to_string()
}

fn read_manifest_revision_hash(root: &Path) -> String {
    let manifest_path = root.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path).expect("manifest is readable");
    let value: Value = serde_json::from_str(&raw).expect("manifest is JSON");
    value["revision_hash"]
        .as_str()
        .expect("manifest.revision_hash is a string")
        .to_string()
}

#[test]
fn generation_identity() {
    let _worker = OcctWorker::locate().unwrap_or_else(|error| {
        panic!(
            "generation_identity requires the OCCT worker; set THREETERM_OCCT_DIR, \
             THREETERM_OCCT_VENDOR=1, or THREETERM_OCCTBUILD_WORKER: {error:?}"
        )
    });
    let root = unique_root("empty");

    let new = run(&["new-project", root.to_str().expect("utf-8 path")]);
    let new_response: Value = serde_json::from_slice(&new.stdout).expect("new-project is JSON");
    let new_generation_id = new_response["generation_id"]
        .as_str()
        .expect("new-project.response.generation_id is a string")
        .to_string();
    assert_eq!(new_generation_id.len(), 64);

    run(&[
        "--machine",
        "save",
        root.to_str().expect("utf-8 path"),
        "--feature-id",
        "box-1",
        "--kind",
        "box",
    ]);
    let after_save = read_manifest_generation_id(&root);
    let after_save_revision = read_manifest_revision_hash(&root);
    assert_ne!(
        after_save, new_generation_id,
        "identity advances on every accepted command"
    );

    let profile = root.join("identity-profile.json");
    fs::write(&profile, "[[0.0,0.0],[4.0,0.0],[2.0,4.0]]").expect("profile writes");
    run(&[
        "--machine",
        "extrude",
        "--bundle",
        root.to_str().expect("utf-8 path"),
        "--feature-id",
        "extrude-1",
        "--profile-file",
        profile.to_str().expect("utf-8 path"),
        "--height",
        "2.0",
        "--mode",
        "additive",
    ]);
    let after_extrude = read_manifest_generation_id(&root);
    let after_extrude_revision = read_manifest_revision_hash(&root);
    assert_ne!(
        after_extrude, after_save,
        "Project Generation identity advances on an accepted extrude"
    );
    assert_ne!(
        after_extrude_revision, after_save_revision,
        "Revision Snapshot identity advances on an accepted extrude"
    );

    let _loaded = run(&["--machine", "load", root.to_str().expect("utf-8 path")]);
    let after_reload = read_manifest_generation_id(&root);
    let after_reload_digest = read_manifest_terminal_log_digest(&root);
    let after_reload_revision = read_manifest_revision_hash(&root);
    assert_eq!(
        after_reload, after_extrude,
        "Project Generation identity is byte-equal after reload"
    );
    assert_ne!(after_reload, after_reload_digest);
    assert_ne!(after_reload, after_reload_revision);
    assert_ne!(after_reload_revision, after_reload_digest);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn full_mvp_operation_set_preserves_byte_equal_identity_on_reload() {
    let root = unique_root("full-set");

    run(&["new-project", root.to_str().expect("utf-8 path")]);
    let initial = read_manifest_generation_id(&root);

    // The MVP operation set on the production code path: chain `save`
    // commands to add features, then `load` to reload from disk.
    let intents = [
        ("box", "box"),
        ("fillet-1", "fillet"),
        ("hole-1", "hole"),
        ("chamfer-1", "chamfer"),
    ];
    let mut last_identity = initial;
    for (feature_id, kind) in &intents {
        run(&[
            "--machine",
            "save",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--kind",
            kind,
        ]);
        let identity = read_manifest_generation_id(&root);
        assert_ne!(
            identity, last_identity,
            "identity advances on every accepted command"
        );
        last_identity = identity;
    }

    let before_reload = read_manifest_generation_id(&root);
    let before_reload_digest = read_manifest_terminal_log_digest(&root);
    assert_ne!(
        before_reload, before_reload_digest,
        "Project Generation identity remains distinct from the canonical log digest"
    );

    let _loaded = run(&["--machine", "load", root.to_str().expect("utf-8 path")]);
    let after_reload = read_manifest_generation_id(&root);
    assert_eq!(
        after_reload, before_reload,
        "Project Generation identity is byte-equal after reload"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn reloading_an_untouched_bundle_keeps_identity_byte_equal() {
    let root = unique_root("stable");

    run(&["new-project", root.to_str().expect("utf-8 path")]);
    let initial = read_manifest_generation_id(&root);

    for _ in 0..2 {
        let _loaded = run(&["--machine", "load", root.to_str().expect("utf-8 path")]);
        let after = read_manifest_generation_id(&root);
        assert_eq!(after, initial);
    }

    let _ = fs::remove_dir_all(root);
}

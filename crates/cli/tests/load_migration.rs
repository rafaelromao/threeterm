use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_domain::ProjectGeneration;
use threeterm_persistence::{
    MANIFEST_FILENAME, PRE_MIGRATION_BACKUP_SUFFIX, schema_epoch, write_fresh, write_v0_fixture,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-cli-load-{label}-{suffix}"))
}

fn load(root: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "load"])
        .arg(root)
        .output()
        .expect("load process runs")
}

#[test]
fn machine_load_migrates_prior_epoch_bundle_and_retains_backup() {
    let root = temp_root("migration");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-cli-prior"))
        .expect("prior-epoch bundle writes");
    let backup = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name()
            .expect("root has filename")
            .to_string_lossy()
    ));

    let output = load(&root);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).expect("load response is JSON");
    assert_eq!(
        response["schema_version"],
        "threeterm.command.load.response/1"
    );
    let manifest: Value = serde_json::from_slice(
        &fs::read(root.join(MANIFEST_FILENAME)).expect("migrated manifest reads"),
    )
    .expect("migrated manifest parses");
    assert_eq!(manifest["schema_version"], schema_epoch());
    assert!(backup.is_dir(), "pre-migration backup is retained");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(backup);
}

#[test]
fn machine_load_rejects_bad_manifests_without_changing_them() {
    for (label, mutation, expected_arg) in [
        (
            "malformed",
            json!({ "future_field": true }),
            "manifest_field_unknown",
        ),
        (
            "unsupported",
            json!({ "schema_version": "threeterm.persistence/99" }),
            "schema_unknown",
        ),
    ] {
        let root = temp_root(label);
        write_fresh(
            &root,
            ProjectGeneration::with_id(format!("generation-{label}")),
        )
        .expect("current bundle writes");
        let manifest_path = root.join(MANIFEST_FILENAME);
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest reads"))
                .expect("manifest parses");
        for (key, value) in mutation.as_object().expect("mutation is an object") {
            manifest[key] = value.clone();
        }
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
        )
        .expect("manifest writes");
        let source = fs::read(&manifest_path).expect("source manifest reads");

        let output = load(&root);

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let diagnostic: Value = serde_json::from_slice(&output.stderr).expect("diagnostic is JSON");
        assert_eq!(diagnostic["code"], "integrity_failure");
        assert_eq!(diagnostic["arg"], expected_arg);
        assert_eq!(
            fs::read(&manifest_path).expect("source manifest re-reads"),
            source,
            "{label} manifest remains byte-identical"
        );

        let _ = fs::remove_dir_all(root);
    }
}

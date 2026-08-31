use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};
use threeterm_domain::ProjectGeneration;
use threeterm_persistence::bundle::{
    BundleError, LoadPolicy, LoadedBundle, Manifest, PRE_MIGRATION_BACKUP_SUFFIX,
    PublicationFailurePoint, SchemaStatus, V0Manifest, detect_schema, fail_next_publication_at,
    load, load_with_policy, migrate_v0_to_v1, prior_schema_epoch, read_v0, schema_epoch,
    write_fresh, write_v0_fixture,
};

fn unique_temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "threeterm-mig-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn read_dir_recursive(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).expect("read_dir") {
            let entry = entry.expect("entry");
            let file_type = entry.file_type().expect("file_type");
            let entry_path = entry.path();
            if file_type.is_dir() {
                stack.push(entry_path);
            } else {
                let bytes = fs::read(&entry_path).expect("read file");
                entries.push((entry_path, bytes));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn fingerprint(path: &Path) -> Vec<(PathBuf, String)> {
    read_dir_recursive(path)
        .into_iter()
        .map(|(p, bytes)| {
            let digest = Sha256::digest(&bytes);
            let hex = digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            (p, hex)
        })
        .collect()
}

#[test]
fn v0_fixture_loads_on_v1_reader_with_sealed_backup() {
    let root = unique_temp_dir("happy");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-happy"))
        .expect("v0 fixture writes");

    let pre_fingerprint = fingerprint(&root);
    let backup_path = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name().unwrap().to_string_lossy()
    ));

    let loaded = load(&root).expect("v0 migrates to v1");
    assert_eq!(loaded.canonical_root, root);
    assert_eq!(loaded.manifest.schema_version, schema_epoch());
    assert!(loaded.transactions.is_empty());
    assert!(loaded.manifest.canonical_root_sha256.len() == 64);
    assert!(loaded.manifest.seal_sha256.len() == 64);

    let post_v1 = read_dir_recursive(&root);
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("manifest reads"))
            .expect("v1 manifest parses");
    assert_eq!(manifest.schema_version, schema_epoch());

    let backup_manifest_raw =
        fs::read(backup_path.join("manifest.json")).expect("backup manifest reads");
    let backup_manifest: V0Manifest =
        serde_json::from_slice(&backup_manifest_raw).expect("v0 manifest parses");
    assert_eq!(backup_manifest.schema_version, prior_schema_epoch());
    assert_eq!(backup_manifest.generation_id, loaded.manifest.generation_id);
    assert_eq!(backup_manifest.revision_id, loaded.manifest.revision_id);

    let pre_set: std::collections::BTreeSet<_> = pre_fingerprint
        .iter()
        .map(|(p, h)| (p.strip_prefix(&root).unwrap().to_path_buf(), h.clone()))
        .collect();
    let post_v1_set: std::collections::BTreeSet<_> = post_v1
        .iter()
        .filter(|(p, _)| !p.starts_with(&backup_path))
        .map(|(p, b)| (p.strip_prefix(&root).unwrap().to_path_buf(), b.clone()))
        .collect();
    let backup_set: std::collections::BTreeSet<_> = read_dir_recursive(&backup_path)
        .iter()
        .map(|(p, b)| {
            let digest = Sha256::digest(b);
            let hex = digest
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<String>();
            (p.strip_prefix(&backup_path).unwrap().to_path_buf(), hex)
        })
        .collect();
    assert_eq!(
        pre_set, backup_set,
        "backup must be a byte-for-byte copy of the source"
    );
    assert_eq!(
        post_v1_set.len(),
        pre_set.len(),
        "post-migration bundle has different file count than source"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_v0_root_recovers_with_a_recovery_status() {
    let root = unique_temp_dir("previous-v0-recovery");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-previous-v0"))
        .expect("v0 fixture writes");
    let previous = root.with_file_name(format!(
        "{}.previous-generation",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&root, &previous).expect("moves v0 generation to previous slot");

    let loaded = load(&root).expect("previous v0 generation migrates");
    assert!(loaded.recovered_from_previous);
    assert_eq!(loaded.manifest.schema_version, schema_epoch());

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(previous);
}

#[test]
fn migration_is_deterministic_across_invocations() {
    let root = unique_temp_dir("determinism");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-det"))
        .expect("v0 fixture writes");
    let v0 = read_v0(&root).expect("v0 reads");
    let (a_manifest, a_generation) = migrate_v0_to_v1(&v0).expect("first migration succeeds");
    let (b_manifest, b_generation) = migrate_v0_to_v1(&v0).expect("second migration succeeds");
    assert_eq!(a_manifest, b_manifest);
    assert_eq!(a_generation, b_generation);
    assert_eq!(
        a_manifest.canonical_root_sha256,
        b_manifest.canonical_root_sha256
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn migration_does_not_promote_prior_derived_results() {
    let root = unique_temp_dir("derived-results");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-derived"))
        .expect("v0 fixture writes");
    fs::create_dir_all(root.join("brep")).expect("brep directory creates");
    fs::write(root.join("brep/old.brep"), b"stale worker output").expect("brep writes");
    fs::create_dir_all(root.join("cache")).expect("cache directory creates");
    fs::write(root.join("cache/old.cache"), b"stale cache").expect("cache writes");

    let backup = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name().unwrap().to_string_lossy()
    ));
    load(&root).expect("v0 migrates");

    assert!(!root.join("brep").exists());
    assert!(!root.join("cache").exists());
    assert_eq!(
        fs::read(backup.join("brep/old.brep")).unwrap(),
        b"stale worker output"
    );
    assert_eq!(
        fs::read(backup.join("cache/old.cache")).unwrap(),
        b"stale cache"
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(backup);
}

#[test]
fn repeat_migration_idempotent_for_a_clean_v0_source() {
    let root = unique_temp_dir("repeat");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-repeat")).expect("v0 writes");
    let v0 = read_v0(&root).expect("v0 reads");
    let (m1, _) = migrate_v0_to_v1(&v0).expect("first migration succeeds");
    let v0_again = read_v0(&root).expect("v0 re-reads");
    let (m2, _) = migrate_v0_to_v1(&v0_again).expect("second migration succeeds");
    assert_eq!(m1, m2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn detect_schema_classifies_prior_and_unknown() {
    let v0_root = unique_temp_dir("detect-prior");
    let v1_root = unique_temp_dir("detect-current");
    let bad_root = unique_temp_dir("detect-unknown");
    write_v0_fixture(&v0_root, ProjectGeneration::with_id("g-prior")).expect("v0");
    write_fresh(&v1_root, ProjectGeneration::with_id("g-current")).expect("v1");
    fs::create_dir_all(&bad_root).expect("dir");
    fs::write(
        bad_root.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "threeterm.persistence/99",
            "generation_id": "g-bad",
            "revision_id": "r-bad"
        }))
        .unwrap(),
    )
    .expect("manifest");

    assert_eq!(detect_schema(&v0_root).expect("prior"), SchemaStatus::Prior);
    assert_eq!(
        detect_schema(&v1_root).expect("current"),
        SchemaStatus::Current
    );
    assert_eq!(
        detect_schema(&bad_root).expect("unknown"),
        SchemaStatus::Unknown
    );

    let _ = fs::remove_dir_all(v0_root);
    let _ = fs::remove_dir_all(v1_root);
    let _ = fs::remove_dir_all(bad_root);
}

#[test]
fn adversarial_v0_policy_fails_closed_before_creating_a_backup() {
    let root = unique_temp_dir("reject-v0");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-reject-v0"))
        .expect("v0 fixture writes");
    let before = fingerprint(&root);
    let backup_path = root.with_file_name(format!(
        "{}{}",
        root.file_name().unwrap().to_string_lossy(),
        PRE_MIGRATION_BACKUP_SUFFIX
    ));

    let error = load_with_policy(&root, LoadPolicy::RejectV0RequiresBackup)
        .expect_err("adversarial v0 policy must refuse migration");

    assert!(matches!(error, BundleError::SchemaEpochV0RequiresBackup));
    assert_eq!(fingerprint(&root), before);
    assert!(!backup_path.exists());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(backup_path);
}

#[test]
fn migration_failure_leaves_source_unchanged() {
    let root = unique_temp_dir("failure");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-fail")).expect("v0 writes");
    let pre = fingerprint(&root);

    fs::write(root.join("canonical/transactions.ndjson"), b"tampered\n").expect("tamper");
    let pre_tamper = fingerprint(&root);
    assert_ne!(pre, pre_tamper);

    let _ = load(&root).expect_err("tampered v0 fails to migrate");

    let post = fingerprint(&root);
    let pre_tamper_set: std::collections::BTreeSet<_> = pre_tamper.into_iter().collect();
    let post_set: std::collections::BTreeSet<_> = post.into_iter().collect();
    assert_eq!(
        pre_tamper_set, post_set,
        "source must remain byte-for-byte unchanged after a failed migration"
    );

    let backup_path = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name().unwrap().to_string_lossy()
    ));
    assert!(
        !backup_path.exists(),
        "no sealed backup may be left behind after a failed migration"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn interrupted_migration_replacement_preserves_the_only_canonical_generation() {
    let root = unique_temp_dir("replacement-failure");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-replacement"))
        .expect("v0 writes");
    let before = fingerprint(&root);

    fail_next_publication_at(PublicationFailurePoint::ReplaceCurrent);
    assert!(load(&root).is_err(), "replacement failure is surfaced");

    assert_eq!(
        fingerprint(&root),
        before,
        "v0 source remains byte-identical"
    );
    assert_eq!(
        detect_schema(&root).expect("source remains readable"),
        SchemaStatus::Prior
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn interrupted_migration_staging_sync_preserves_the_only_canonical_generation() {
    let root = unique_temp_dir("sync-failure");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-sync")).expect("v0 writes");
    let before = fingerprint(&root);

    fail_next_publication_at(PublicationFailurePoint::StagingSync);
    assert!(load(&root).is_err(), "staging sync failure is surfaced");

    assert_eq!(
        fingerprint(&root),
        before,
        "v0 source remains byte-identical when staging cannot be synced"
    );
    assert_eq!(
        detect_schema(&root).expect("source remains readable"),
        SchemaStatus::Prior
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn migration_parent_sync_error_leaves_the_promoted_generation_loadable() {
    let root = unique_temp_dir("parent-sync-failure");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-parent-sync"))
        .expect("v0 writes");

    fail_next_publication_at(PublicationFailurePoint::ParentSync);
    assert!(
        load(&root).is_err(),
        "post-promotion sync error is surfaced"
    );

    let loaded = load(&root).expect("promoted generation remains loadable");
    assert_eq!(loaded.manifest.schema_version, schema_epoch());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn interrupted_migration_promotion_restores_the_v0_source_and_retains_staging() {
    let root = unique_temp_dir("promotion-failure");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-promotion")).expect("v0 writes");
    let before = fingerprint(&root);

    fail_next_publication_at(PublicationFailurePoint::PromoteStaging);
    assert!(load(&root).is_err(), "promotion failure is surfaced");

    assert_eq!(
        fingerprint(&root),
        before,
        "v0 source is restored byte-for-byte"
    );
    assert_eq!(
        detect_schema(&root).expect("source remains readable"),
        SchemaStatus::Prior
    );
    let staging = root.with_file_name(format!(
        "{}.migrate-tmp-{}",
        root.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    assert!(staging.exists(), "sealed replacement remains available");
    let retried = load(&root).expect("retained backup permits migration retry");
    assert_eq!(retried.manifest.schema_version, schema_epoch());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn unknown_manifest_field_fails_closed_with_structured_error() {
    let root = unique_temp_dir("unknown-field");
    let generation = ProjectGeneration::with_id("g-unknown");
    write_fresh(&root, generation).expect("v1 writes");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("manifest"))
            .expect("value");
    value
        .as_object_mut()
        .unwrap()
        .insert("future_field".to_string(), serde_json::Value::Bool(true));
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .expect("rewrite");

    let err = load(&root).expect_err("v1 with unknown field is rejected");
    match err {
        BundleError::ManifestFieldUnknown { kind, field } => {
            assert_eq!(kind, "v1");
            assert_eq!(field, "future_field");
        }
        other => panic!("expected ManifestFieldUnknown, got {other:?}"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn v2_reader_boundary_refuses_unbacked_v0_layout() {
    // A v2 reader is expressed today as `detect_schema` against a directory
    // whose manifest says `schema_version == "0"` but which is the
    // pre-migration backup sibling — i.e., a v2 reader opens the v0 layout
    // that a migration left behind and sees it as `Unknown` because the
    // canonical v2 layout requires the prior-epoch backup to live in a
    // sibling, not at the canonical path.
    let root = unique_temp_dir("v2-refusal");
    write_v0_fixture(&root, ProjectGeneration::with_id("g-v0")).expect("v0");
    let _: LoadedBundle = load(&root).expect("v0 migrates");
    let backup_path = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name().unwrap().to_string_lossy()
    ));
    assert!(backup_path.exists());
    let backup_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(backup_path.join("manifest.json")).expect("backup manifest"),
    )
    .expect("backup value");
    assert_eq!(
        backup_manifest["schema_version"],
        json!(prior_schema_epoch())
    );
    assert_eq!(
        detect_schema(&backup_path).expect("v2 boundary classifies backup"),
        SchemaStatus::Unknown,
        "a v2 reader must refuse a directory that is the pre-migration backup sibling"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn migration_retry_failure_preserves_an_authenticated_pre_existing_backup() {
    let root = unique_temp_dir("backup-preserved");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-backup-keep"))
        .expect("v0 fixture writes");
    let backup_path = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name().unwrap().to_string_lossy()
    ));

    // Attempt one seals the backup and fails only at the final promotion.
    fail_next_publication_at(PublicationFailurePoint::PromoteStaging);
    assert!(
        load(&root).is_err(),
        "the first attempt's promotion failure is surfaced"
    );
    assert!(
        backup_path.is_dir(),
        "the first attempt leaves a sealed recovery backup"
    );
    let sealed_fingerprint = fingerprint(&backup_path);

    // Attempt two retains the authenticated pre-existing backup and fails
    // later, while writing the new staging generation.
    fail_next_publication_at(PublicationFailurePoint::StagingSync);
    assert!(
        load(&root).is_err(),
        "the retry's staging failure is surfaced"
    );
    assert_eq!(
        fingerprint(&backup_path),
        sealed_fingerprint,
        "a retry that fails after retaining the backup must not delete it"
    );
    assert!(
        read_v0(&backup_path).is_ok(),
        "the retained backup still authenticates as a complete v0 bundle"
    );

    let loaded = load(&root).expect("a later attempt migrates with the retained backup");
    assert_eq!(loaded.manifest.schema_version, schema_epoch());
    assert!(
        read_v0(&backup_path).is_ok(),
        "the authenticated backup survives the successful migration"
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(backup_path);
}

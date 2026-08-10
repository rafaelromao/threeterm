use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use threeterm_host::{Host, HostError};
use threeterm_persistence::bundle::{
    Bundle, EMPTY_LOG_DIGEST_HEX, Manifest, PublicationFailurePoint, fail_next_publication_at,
};

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-host-concurrent-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

fn assert_linear_log(root: &std::path::Path, expected_len: usize) {
    let loaded = Bundle::at(root).open().expect("bundle opens");
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).expect("manifest reads"))
            .expect("manifest parses");
    assert_eq!(manifest, loaded.manifest, "manifest is fully authenticated");
    assert_eq!(
        loaded.log.len(),
        expected_len,
        "every accepted save lands in one canonical log"
    );
    assert_eq!(loaded.manifest.transaction_count, loaded.log.len());
    assert_eq!(loaded.manifest.transaction_bytes, loaded.transactions.len());
    assert_eq!(
        loaded.manifest.terminal_log_digest,
        loaded.log.terminal_digest_hex()
    );
    let entries = loaded.log.entries();
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(
            entry.log_index, index,
            "log positions are unique and sequential"
        );
        let expected_previous = if index == 0 {
            EMPTY_LOG_DIGEST_HEX
        } else {
            entries[index - 1].terminal_digest.as_str()
        };
        assert_eq!(entry.previous_digest, expected_previous);
    }
}

fn previous_root(root: &std::path::Path) -> PathBuf {
    let mut previous = root.to_path_buf();
    previous.set_file_name(format!(
        "{}.previous-generation",
        root.file_name().expect("root has a name").to_string_lossy()
    ));
    previous
}

fn assert_no_publication_staging(root: &Path) {
    let root_name = root.file_name().expect("root has a name").to_string_lossy();
    let staging = std::fs::read_dir(root.parent().expect("root has a parent"))
        .expect("bundle parent reads")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(root_name.as_ref()) && name.contains(".publish-tmp-")
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(
        staging.is_empty(),
        "failed publication must not strand staging directories: {staging:?}"
    );
}

fn assert_host_save_storage_failure(point: PublicationFailurePoint) {
    let root = temp_root(&format!("{point:?}-failure"));
    let host = Host::new();
    host.save(&root, "box-1", "box")
        .expect("initial save succeeds");
    let prior_manifest = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let prior_log = std::fs::read(root.join("transactions.log")).expect("log reads");

    fail_next_publication_at(point);
    let error = host
        .save(&root, "box-2", "box")
        .expect_err("an injected storage failure must not report a successful save");
    assert!(
        matches!(error, HostError::Persistence(_)),
        "{point:?} must remain a structured persistence error: {error}"
    );
    assert_eq!(
        std::fs::read(root.join("manifest.json")).unwrap(),
        prior_manifest,
        "{point:?} must preserve the current manifest"
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).unwrap(),
        prior_log,
        "{point:?} must preserve the current log"
    );
    host.load(&root)
        .expect("the prior generation remains reloadable");
    assert_no_publication_staging(&root);

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(previous_root(&root));
}

#[test]
fn concurrent_host_saves_serialize_through_the_bundle_lock() {
    let root = temp_root("saves");
    const THREADS: usize = 8;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let root = root.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let host = Host::new();
            host.save(&root, &format!("box-{thread}"), "box")
                .expect("concurrent host save succeeds");
        }));
    }
    for handle in handles {
        handle.join().expect("host save thread completes");
    }

    assert_linear_log(&root, THREADS);
    let loaded = Bundle::at(&root).open().expect("bundle opens after race");
    let expected_features: BTreeSet<_> =
        (0..THREADS).map(|thread| format!("box-{thread}")).collect();
    let actual_features: BTreeSet<_> = loaded
        .log
        .entries()
        .iter()
        .map(|entry| entry.feature_id.clone())
        .collect();
    assert_eq!(actual_features, expected_features);
    assert_no_publication_staging(&root);

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(previous_root(&root));
}

#[test]
fn host_save_surfaces_log_sync_failure_without_changing_the_current_generation() {
    assert_host_save_storage_failure(PublicationFailurePoint::LogSync);
}

#[test]
fn host_save_surfaces_manifest_sync_failure_without_changing_the_current_generation() {
    assert_host_save_storage_failure(PublicationFailurePoint::ManifestSync);
}

#[test]
fn host_save_surfaces_staged_files_failure_without_changing_the_current_generation() {
    assert_host_save_storage_failure(PublicationFailurePoint::StagedFiles);
}

#[test]
fn host_save_surfaces_staging_directory_sync_failure_without_changing_the_current_generation() {
    assert_host_save_storage_failure(PublicationFailurePoint::StagingSync);
}

#[test]
fn host_save_surfaces_containing_directory_sync_failure_after_promotion() {
    let root = temp_root("parent-sync-failure");
    let host = Host::new();
    host.save(&root, "box-1", "box")
        .expect("initial save succeeds");
    let prior_manifest = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let prior_log = std::fs::read(root.join("transactions.log")).expect("log reads");

    fail_next_publication_at(PublicationFailurePoint::ParentSync);
    let error = host
        .save(&root, "box-2", "box")
        .expect_err("a parent sync failure must not report a successful save");
    assert!(
        matches!(error, HostError::Persistence(_)),
        "parent sync failure must remain a structured persistence error: {error}"
    );

    assert_linear_log(&root, 2);
    let current = host.load(&root).expect("the promoted generation reloads");
    assert_eq!(host.current(), Some(current));
    let previous = previous_root(&root);
    assert_linear_log(&previous, 1);
    assert_eq!(
        std::fs::read(previous.join("manifest.json")).unwrap(),
        prior_manifest,
        "the prior manifest remains byte-identical in the recovery slot"
    );
    assert_eq!(
        std::fs::read(previous.join("transactions.log")).unwrap(),
        prior_log,
        "the prior log remains byte-identical in the recovery slot"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(previous);
}

#[test]
fn a_later_host_save_recovers_after_each_injected_storage_failure() {
    for point in [
        PublicationFailurePoint::StagedFiles,
        PublicationFailurePoint::LogSync,
        PublicationFailurePoint::ManifestSync,
        PublicationFailurePoint::StagingSync,
        PublicationFailurePoint::ParentSync,
    ] {
        let root = temp_root(&format!("retry-{point:?}"));
        let host = Host::new();
        host.save(&root, "box-1", "box")
            .expect("initial save succeeds");

        fail_next_publication_at(point);
        let error = host
            .save(&root, "box-2", "box")
            .expect_err("the injected failure must be returned");
        assert!(
            matches!(error, HostError::Persistence(_)),
            "{point:?} must remain a structured persistence error: {error}"
        );

        host.save(&root, "box-retry", "box")
            .expect("a later save succeeds after the injected failure");
        let expected_len = if point == PublicationFailurePoint::ParentSync {
            3
        } else {
            2
        };
        assert_linear_log(&root, expected_len);
        assert_no_publication_staging(&root);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(previous_root(&root));
    }
}

#[test]
fn concurrent_host_save_brackets_serialize_through_the_bundle_lock() {
    let root = temp_root("brackets");
    const THREADS: usize = 4;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            let host = Host::new();
            host.save_bracket(&root, &format!("l-{thread}"), 60.0, 30.0, 40.0, 3.0)
                .expect("concurrent host save_bracket succeeds");
        }));
    }
    for handle in handles {
        handle.join().expect("host save_bracket thread completes");
    }

    assert_linear_log(&root, THREADS * 2);

    let _ = std::fs::remove_dir_all(&root);
    let mut previous = root.clone();
    previous.set_file_name(format!(
        "{}.previous-generation",
        root.file_name().expect("root has a name").to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(previous);
}

#[test]
fn save_bracket_migrates_a_prior_epoch_root() {
    use threeterm_domain::ProjectGeneration;
    use threeterm_persistence::{PRE_MIGRATION_BACKUP_SUFFIX, schema_epoch, write_v0_fixture};

    let root = temp_root("bracket-v0");
    write_v0_fixture(&root, ProjectGeneration::with_id("generation-bracket-v0"))
        .expect("v0 fixture writes");
    let backup = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name().expect("root has a name").to_string_lossy()
    ));

    let host = Host::new();
    host.save_bracket(&root, "l-1", 60.0, 30.0, 40.0, 3.0)
        .expect("save_bracket migrates and appends");

    let loaded = Bundle::at(&root).open().expect("bundle opens");
    assert_eq!(
        loaded.log.len(),
        2,
        "both bracket plates land in the migrated log"
    );
    assert_eq!(
        loaded.manifest.schema_version,
        schema_epoch(),
        "the bundle is migrated to the current epoch"
    );
    assert!(backup.is_dir(), "the pre-migration backup is retained");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(backup);
}

#[test]
fn concurrent_saves_and_loads_on_a_missing_root_serialize() {
    let root = temp_root("save-load-race");
    const THREADS: usize = 8;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            let host = Host::new();
            if thread % 2 == 0 {
                host.save(&root, &format!("box-{thread}"), "box")
                    .expect("concurrent save succeeds");
            } else {
                let result = host.load(&root);
                assert!(
                    result.is_ok()
                        || matches!(
                            result,
                            Err(threeterm_host::HostError::BundlePathMissing { .. })
                        ),
                    "a load either succeeds or reports a missing bundle, got {result:?}"
                );
            }
        }));
    }
    for handle in handles {
        handle.join().expect("save or load thread completes");
    }

    let loaded = Bundle::at(&root)
        .open()
        .expect("bundle opens after the race");
    assert_eq!(
        loaded.log.len(),
        THREADS / 2,
        "every save lands exactly once"
    );
    let entries = loaded.log.entries();
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.log_index, index);
        let expected_previous = if index == 0 {
            EMPTY_LOG_DIGEST_HEX
        } else {
            entries[index - 1].terminal_digest.as_str()
        };
        assert_eq!(entry.previous_digest, expected_previous);
    }

    let _ = std::fs::remove_dir_all(&root);
    let mut previous = root.clone();
    previous.set_file_name(format!(
        "{}.previous-generation",
        root.file_name().expect("root has a name").to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(previous);
}

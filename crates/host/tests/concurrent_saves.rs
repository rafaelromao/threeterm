use std::path::PathBuf;

use threeterm_host::Host;
use threeterm_persistence::bundle::{Bundle, EMPTY_LOG_DIGEST_HEX};

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
    assert_eq!(
        loaded.log.len(),
        expected_len,
        "every accepted save lands in one canonical log"
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

#[test]
fn concurrent_host_saves_serialize_through_the_bundle_lock() {
    let root = temp_root("saves");
    const THREADS: usize = 8;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            let host = Host::new();
            host.save(&root, &format!("box-{thread}"), "box")
                .expect("concurrent host save succeeds");
        }));
    }
    for handle in handles {
        handle.join().expect("host save thread completes");
    }

    assert_linear_log(&root, THREADS);

    let _ = std::fs::remove_dir_all(&root);
    let mut previous = root.clone();
    previous.set_file_name(format!(
        "{}.previous-generation",
        root.file_name().expect("root has a name").to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(previous);
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

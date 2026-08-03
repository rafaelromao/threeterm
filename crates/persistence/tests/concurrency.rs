use std::fs;
use std::path::{Path, PathBuf};

use threeterm_domain::ProjectGeneration;
use threeterm_persistence::bundle::{
    Bundle, EMPTY_LOG_DIGEST_HEX, MANIFEST_FILENAME, PublicationFailurePoint,
    TRANSACTIONS_LOG_FILENAME, fail_next_publication_at, load, write_v0_fixture,
};
use threeterm_persistence::{PREVIOUS_GENERATION_SUFFIX, WRITE_LOCK_SUFFIX};

fn unique_temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "threeterm-writes-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn previous_generation_sibling(root: &Path) -> PathBuf {
    let mut previous = root.to_path_buf();
    previous.set_file_name(format!(
        "{}{PREVIOUS_GENERATION_SUFFIX}",
        root.file_name().unwrap_or_default().to_string_lossy()
    ));
    previous
}

#[test]
fn successful_saves_retain_the_immediately_preceding_generation() {
    let root = unique_temp_dir("retain-preceding");
    let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
    bundle
        .append_feature("box-1", "box")
        .expect("first save publishes");
    let preceding_manifest =
        fs::read(root.join(MANIFEST_FILENAME)).expect("preceding manifest reads");
    let preceding_log =
        fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("preceding log reads");

    bundle
        .append_feature("box-2", "box")
        .expect("second save publishes");

    let previous = previous_generation_sibling(&root);
    assert!(
        previous.is_dir(),
        "preceding generation is retained on disk"
    );
    assert_eq!(
        fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
        preceding_manifest,
        "retained manifest matches the immediately preceding generation"
    );
    assert_eq!(
        fs::read(previous.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
        preceding_log,
        "retained log matches the immediately preceding generation"
    );
    let retained = Bundle::at(&previous)
        .open()
        .expect("retained generation opens");
    assert_eq!(retained.log.len(), 1);
    assert!(!retained.recovered_from_previous);

    let current = Bundle::at(&root).open().expect("current generation opens");
    assert_eq!(current.log.len(), 2);
    assert!(!current.recovered_from_previous);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous);
}

#[test]
fn concurrent_appends_serialize_into_one_linear_log() {
    let root = unique_temp_dir("concurrent-appends");
    let bundle = std::sync::Arc::new(
        Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates"),
    );

    const THREADS: usize = 8;
    const APPENDS_PER_THREAD: usize = 4;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let bundle = bundle.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..APPENDS_PER_THREAD {
                bundle
                    .append_feature(&format!("box-{thread}-{i}"), "box")
                    .expect("concurrent append succeeds");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("append thread completes");
    }

    let loaded = bundle.open().expect("bundle reopens");
    assert_eq!(
        loaded.log.len(),
        THREADS * APPENDS_PER_THREAD,
        "every accepted append lands in the canonical log"
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
        assert_eq!(
            entry.previous_digest, expected_previous,
            "predecessor digests chain without conflict"
        );
    }

    let parent = root.parent().expect("temp root has a parent");
    let stray = fs::read_dir(parent)
        .expect("parent reads")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(
                root.file_name()
                    .expect("root has a name")
                    .to_string_lossy()
                    .as_ref(),
            ) && name.contains(".publish-tmp-")
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(stray.is_empty(), "no staging directories remain: {stray:?}");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous_generation_sibling(&root));
}

#[test]
fn concurrent_migration_and_appends_serialize() {
    let root = unique_temp_dir("concurrent-migration");
    write_v0_fixture(
        &root,
        ProjectGeneration::with_id("generation-concurrent-migration"),
    )
    .expect("v0 fixture writes");
    let bundle = std::sync::Arc::new(Bundle::at(&root));

    const THREADS: usize = 8;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let root = root.clone();
        let bundle = bundle.clone();
        handles.push(std::thread::spawn(move || {
            load(&root).expect("load migrates the v0 source");
            bundle
                .append_feature(&format!("box-{thread}"), "box")
                .expect("append after migration succeeds");
        }));
    }
    for handle in handles {
        handle
            .join()
            .expect("migration and append thread completes");
    }

    let loaded = bundle.open().expect("bundle opens");
    assert_eq!(
        loaded.log.len(),
        THREADS,
        "every append after migration lands in one canonical log"
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

    let parent = root.parent().expect("temp root has a parent");
    let stray = fs::read_dir(parent)
        .expect("parent reads")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(
                root.file_name()
                    .expect("root has a name")
                    .to_string_lossy()
                    .as_ref(),
            ) && (name.contains(".migrate-tmp-") || name.contains(".publish-tmp-"))
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(stray.is_empty(), "no staging directories remain: {stray:?}");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous_generation_sibling(&root));
}

#[test]
fn concurrent_opens_reconcile_the_crash_state_idempotently() {
    let root = unique_temp_dir("concurrent-reconcile");
    let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
    bundle
        .append_feature("box-1", "box")
        .expect("first publish");
    bundle
        .append_feature("box-2", "box")
        .expect("second publish");
    let previous = previous_generation_sibling(&root);
    let retired = {
        let mut retired = previous.clone();
        retired.set_file_name(format!(
            "{}.retired-generation",
            previous.file_name().unwrap_or_default().to_string_lossy()
        ));
        retired
    };
    let preceding_manifest =
        fs::read(previous.join(MANIFEST_FILENAME)).expect("preceding manifest reads");
    fs::rename(&previous, &retired).expect("simulates an interrupted rotation");

    let mut handles = Vec::new();
    for _ in 0..4 {
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            let loaded = Bundle::at(&root).open().expect("concurrent open succeeds");
            assert_eq!(loaded.log.len(), 2);
        }));
    }
    for handle in handles {
        handle.join().expect("open thread completes");
    }

    assert_eq!(
        fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
        preceding_manifest,
        "the recognized previous slot is restored exactly once"
    );
    assert!(
        !retired.exists(),
        "the retired slot is drained by reconciliation"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous);
}

#[test]
fn reader_reconcile_waits_for_the_writer_lock() {
    let root = unique_temp_dir("reader-writer-lock");
    let bundle = Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates");
    bundle
        .append_feature("box-1", "box")
        .expect("first publish");
    bundle
        .append_feature("box-2", "box")
        .expect("second publish");
    let previous = previous_generation_sibling(&root);
    let retired = {
        let mut retired = previous.clone();
        retired.set_file_name(format!(
            "{}.retired-generation",
            previous.file_name().unwrap_or_default().to_string_lossy()
        ));
        retired
    };
    let preceding_manifest =
        fs::read(previous.join(MANIFEST_FILENAME)).expect("preceding manifest reads");
    let preceding_log =
        fs::read(previous.join(TRANSACTIONS_LOG_FILENAME)).expect("preceding log reads");
    let current_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("current manifest reads");
    let current_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("current log reads");
    fs::rename(&previous, &retired).expect("simulates the mid-rotation crash state");

    // The test plays the writer: it holds the per-root write lock while the
    // rotation is between `previous → retired` and `destination → previous`,
    // the exact state in which a lock-free reconciler would restore the
    // previous slot and fail the writer's replacement.
    let mut lock_path = root.clone();
    let mut lock_name = root.file_name().expect("root has a name").to_os_string();
    lock_name.push(WRITE_LOCK_SUFFIX);
    lock_path.set_file_name(lock_name);
    let lock_file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("lock file opens");
    lock_file.lock().expect("writer lock is exclusive");

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let reader_root = root.clone();
    let reader = std::thread::spawn(move || {
        started_tx.send(()).expect("started signal sends");
        let result = Bundle::at(&reader_root).open();
        done_tx.send(()).expect("completion signal sends");
        result
    });
    started_rx
        .recv()
        .expect("reader thread is running before the writer completes");

    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "a reader's reconcile must block while the writer holds the lock"
    );

    lock_file.unlock().expect("writer releases the lock");
    let loaded = reader
        .join()
        .expect("reader completes after the writer")
        .expect("reader open succeeds");
    assert_eq!(
        loaded.log.len(),
        2,
        "the reader opens the selected canonical generation"
    );
    assert_eq!(
        fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
        preceding_manifest,
        "the restored previous slot carries the preceding generation byte-for-byte"
    );
    assert_eq!(
        fs::read(previous.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
        preceding_log
    );

    bundle
        .append_feature("box-3", "box")
        .expect("a writer after the reader serializes normally");
    let reopened = bundle.open().expect("bundle opens");
    assert_eq!(reopened.log.len(), 3);
    let entries = reopened.log.entries();
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
    assert_eq!(
        fs::read(previous.join(MANIFEST_FILENAME)).unwrap(),
        current_manifest,
        "the preceding generation survives the writer's rotation"
    );
    assert_eq!(
        fs::read(previous.join(TRANSACTIONS_LOG_FILENAME)).unwrap(),
        current_log
    );
    assert!(
        !retired.exists(),
        "no retired slot survives a completed rotation"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous);
}

#[cfg(unix)]
#[test]
fn concurrent_appends_serialize_for_non_utf8_bundle_names() {
    use std::os::unix::ffi::OsStringExt;

    let parent = unique_temp_dir("non-utf8");
    let root = parent.join(std::ffi::OsString::from_vec(b"bundle-\xff\xfe".to_vec()));
    let bundle = std::sync::Arc::new(
        Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates"),
    );

    const THREADS: usize = 8;
    const APPENDS_PER_THREAD: usize = 4;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let bundle = bundle.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for i in 0..APPENDS_PER_THREAD {
                bundle
                    .append_feature(&format!("box-{thread}-{i}"), "box")
                    .expect("concurrent append on a non-UTF-8 bundle serializes");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("append thread completes");
    }

    let loaded = bundle.open().expect("non-UTF-8 bundle reopens");
    assert_eq!(
        loaded.log.len(),
        THREADS * APPENDS_PER_THREAD,
        "a non-UTF-8 bundle name still serializes concurrent appends"
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

    let _ = fs::remove_dir_all(&root);
    let mut previous = root.clone();
    let mut previous_name = root
        .file_name()
        .expect("non-UTF-8 root has a name")
        .to_os_string();
    previous_name.push(PREVIOUS_GENERATION_SUFFIX);
    previous.set_file_name(previous_name);
    let _ = fs::remove_dir_all(previous);
}

#[test]
fn a_failed_concurrent_append_never_breaks_the_linear_log() {
    let root = unique_temp_dir("concurrent-failure");
    let bundle = std::sync::Arc::new(
        Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates"),
    );

    const THREADS: usize = 8;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let bundle = bundle.clone();
        handles.push(std::thread::spawn(move || {
            if thread == 0 {
                fail_next_publication_at(PublicationFailurePoint::PromoteStaging);
                assert!(
                    bundle
                        .append_feature(&format!("box-fail-{thread}"), "box")
                        .is_err(),
                    "injected publication failure is surfaced to its writer"
                );
            } else {
                bundle
                    .append_feature(&format!("box-{thread}"), "box")
                    .expect("concurrent append succeeds");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("append thread completes");
    }

    let loaded = bundle
        .open()
        .expect("bundle opens after a failed concurrent append");
    assert_eq!(
        loaded.log.len(),
        THREADS - 1,
        "the failed append is never half-published"
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
    let previous = previous_generation_sibling(&root);
    assert!(
        previous.is_dir(),
        "preceding generation survives the failure"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous);
}

#[test]
fn concurrent_first_saves_serialize_creation_and_appends() {
    let root = unique_temp_dir("concurrent-first-saves");
    let bundle = std::sync::Arc::new(Bundle::at(&root));

    const THREADS: usize = 8;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let bundle = bundle.clone();
        handles.push(std::thread::spawn(move || {
            bundle
                .append_feature(&format!("box-{thread}"), "box")
                .expect("concurrent first save succeeds");
        }));
    }
    for handle in handles {
        handle.join().expect("save thread completes");
    }

    let loaded = bundle.open().expect("bundle opens");
    assert_eq!(
        loaded.log.len(),
        THREADS,
        "every concurrent first save lands in one canonical log"
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

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous_generation_sibling(&root));
}

use std::fs;
use std::path::{Path, PathBuf};

use threeterm_domain::ProjectGeneration;
use threeterm_persistence::PREVIOUS_GENERATION_SUFFIX;
use threeterm_persistence::bundle::{
    Bundle, EMPTY_LOG_DIGEST_HEX, MANIFEST_FILENAME, PublicationFailurePoint,
    TRANSACTIONS_LOG_FILENAME, fail_next_publication_at, load, write_v0_fixture,
};

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

    // The test plays the writer: it holds the per-root write lock (the
    // flock on the containing directory) while the rotation is between
    // `previous → retired` and `destination → previous`, the exact state in
    // which a lock-free reconciler would restore the previous slot and fail
    // the writer's replacement.
    let lock_file = std::fs::File::open(root.parent().expect("temp root has a parent"))
        .expect("lock directory opens");
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

#[test]
fn load_waits_for_an_in_flight_publication() {
    let root = unique_temp_dir("load-during-publication");
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
    let current_manifest = fs::read(root.join(MANIFEST_FILENAME)).expect("current manifest reads");

    // The test plays the writer: it holds the per-root write lock (the
    // flock on the containing directory) while the rotation is between
    // `destination → previous` and `staging → destination`, the exact
    // window in which an unlocked loader could observe a missing manifest
    // and fail instead of waiting.
    let lock_file = std::fs::File::open(root.parent().expect("temp root has a parent"))
        .expect("lock directory opens");
    lock_file.lock().expect("writer lock is exclusive");
    fs::rename(&previous, &retired).expect("preceding generation retires");
    fs::rename(&root, &previous).expect("simulates the mid-rotation window");

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let reader_root = root.clone();
    let reader = std::thread::spawn(move || {
        started_tx.send(()).expect("started signal sends");
        let result = load(&reader_root);
        done_tx.send(()).expect("completion signal sends");
        result
    });
    started_rx
        .recv()
        .expect("reader is running before the writer completes");

    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "a load must block while a publication holds the lock"
    );

    fs::rename(&previous, &root).expect("writer restores the canonical root");
    lock_file.unlock().expect("writer releases the lock");
    let loaded = reader
        .join()
        .expect("reader completes after the writer")
        .expect("load succeeds after the in-flight publication");
    assert_eq!(
        loaded.log.len(),
        2,
        "the load classifies the post-publication generation"
    );
    assert_eq!(
        fs::read(root.join(MANIFEST_FILENAME)).unwrap(),
        current_manifest,
        "the canonical root carries the published generation"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous);
    let _ = fs::remove_dir_all(retired);
}

#[test]
fn first_save_setup_failures_leave_no_partial_root() {
    for point in [
        PublicationFailurePoint::StagingSync,
        PublicationFailurePoint::PromoteStaging,
    ] {
        let root = unique_temp_dir(&format!("first-save-{point:?}"));
        let bundle = std::sync::Arc::new(Bundle::at(&root));

        fail_next_publication_at(point);
        assert!(
            bundle.append_feature("box-1", "box").is_err(),
            "{point:?} failure is surfaced"
        );
        assert!(
            !root.exists(),
            "no partial canonical root after an interrupted first save"
        );

        bundle
            .append_feature("box-1", "box")
            .expect("a later save creates the bundle cleanly");
        let loaded = bundle.open().expect("bundle opens");
        assert_eq!(loaded.log.len(), 1);
        assert_eq!(
            loaded.log.entries()[0].previous_digest,
            EMPTY_LOG_DIGEST_HEX
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(previous_generation_sibling(&root));
    }
}

#[test]
fn fresh_creation_with_a_bare_relative_root_succeeds() {
    let parent = unique_temp_dir("relative-root");
    fs::create_dir_all(&parent).expect("temp parent creates");
    let cwd = std::env::current_dir().expect("current dir");
    std::env::set_current_dir(&parent).expect("chdir into the temp parent");

    let result = Bundle::create_for_test("project", "00".repeat(16).as_str());
    let bundle = result.expect("a bare relative root creates and publishes");
    let loaded = bundle.open().expect("relative bundle opens");
    assert_eq!(loaded.log.len(), 0);
    bundle
        .append_feature("box-1", "box")
        .expect("a relative append publishes");
    let loaded = bundle.open().expect("bundle reopens");
    assert_eq!(loaded.log.len(), 1);

    std::env::set_current_dir(&cwd).expect("restore the original current dir");
    let _ = fs::remove_dir_all(&parent);
}

#[cfg(unix)]
#[test]
fn non_utf8_and_lossy_colliding_roots_keep_distinct_sibling_paths() {
    use std::os::unix::ffi::OsStringExt;

    let parent = unique_temp_dir("lossy-collision");
    fs::create_dir_all(&parent).expect("parent creates");
    // `b"project-\xff"` and "project-\u{FFFD}" collapse onto the same name
    // under `to_string_lossy()`, so their derived sibling paths must be
    // built from the raw `OsStr` bytes or the two bundles race on shared
    // staging, previous, and retired slots.
    let raw_root = parent.join(std::ffi::OsString::from_vec(b"project-\xff".to_vec()));
    let utf8_root = parent.join("project-\u{FFFD}");

    let raw_bundle = std::sync::Arc::new(
        Bundle::create_for_test(&raw_root, "00".repeat(16).as_str()).expect("raw bundle creates"),
    );
    let utf8_bundle = std::sync::Arc::new(
        Bundle::create_for_test(&utf8_root, "11".repeat(16).as_str()).expect("utf8 bundle creates"),
    );

    const THREADS: usize = 4;
    const APPENDS_PER_THREAD: usize = 3;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let raw_bundle = raw_bundle.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..APPENDS_PER_THREAD {
                raw_bundle
                    .append_feature(&format!("raw-{thread}-{i}"), "box")
                    .expect("raw bundle append serializes");
            }
        }));
    }
    for thread in 0..THREADS {
        let utf8_bundle = utf8_bundle.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..APPENDS_PER_THREAD {
                utf8_bundle
                    .append_feature(&format!("utf8-{thread}-{i}"), "box")
                    .expect("utf8 bundle append serializes");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("append thread completes");
    }

    for (bundle, prefix) in [(raw_bundle, "raw"), (utf8_bundle, "utf8")] {
        let loaded = bundle.open().expect("bundle reopens");
        assert_eq!(
            loaded.log.len(),
            THREADS * APPENDS_PER_THREAD,
            "{prefix} bundle keeps its own linear log"
        );
        let entries = loaded.log.entries();
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(entry.log_index, index);
            assert!(
                entry.feature_id.starts_with(prefix),
                "no cross-contamination between colliding roots"
            );
            let expected_previous = if index == 0 {
                EMPTY_LOG_DIGEST_HEX
            } else {
                entries[index - 1].terminal_digest.as_str()
            };
            assert_eq!(entry.previous_digest, expected_previous);
        }
    }

    let _ = fs::remove_dir_all(parent);
}

#[test]
fn first_save_recovers_with_stale_staging_candidates() {
    let root = unique_temp_dir("stale-staging");
    let bundle = std::sync::Arc::new(Bundle::at(&root));

    // Simulate interrupted saves from a prior process lifetime: the
    // PID-based staging candidate and its first sequence candidate both
    // remain on disk.
    let mut base = root.clone();
    base.set_file_name(format!(
        "{}.publish-tmp-{}",
        root.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    fs::create_dir_all(&base).expect("stale base staging creates");
    let mut sequence = base.clone();
    sequence.set_file_name(format!(
        "{}-0",
        base.file_name().unwrap_or_default().to_string_lossy()
    ));
    fs::create_dir_all(&sequence).expect("stale sequence staging creates");

    bundle
        .append_feature("box-1", "box")
        .expect("first save selects an absent staging candidate");
    let loaded = bundle.open().expect("bundle opens");
    assert_eq!(loaded.log.len(), 1);
    assert_eq!(
        loaded.log.entries()[0].previous_digest,
        EMPTY_LOG_DIGEST_HEX
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous_generation_sibling(&root));
}

#[cfg(unix)]
#[test]
fn symlinked_bundle_root_serializes_with_its_target() {
    use std::os::unix::fs::symlink;

    let parent = unique_temp_dir("symlink-root");
    fs::create_dir_all(&parent).expect("parent creates");
    let real_root = parent.join("real");
    let alias = parent.join("alias");
    let real_bundle = std::sync::Arc::new(
        Bundle::create_for_test(&real_root, "00".repeat(16).as_str()).expect("real bundle creates"),
    );
    symlink(&real_root, &alias).expect("alias symlink creates");
    let alias_bundle = std::sync::Arc::new(Bundle::at(&alias));

    const THREADS: usize = 4;
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let real_bundle = real_bundle.clone();
        handles.push(std::thread::spawn(move || {
            real_bundle
                .append_feature(&format!("real-{thread}"), "box")
                .expect("target append succeeds");
        }));
    }
    for thread in 0..THREADS {
        let alias_bundle = alias_bundle.clone();
        handles.push(std::thread::spawn(move || {
            alias_bundle
                .append_feature(&format!("alias-{thread}"), "box")
                .expect("alias append succeeds");
        }));
    }
    for handle in handles {
        handle.join().expect("append thread completes");
    }

    let loaded = Bundle::at(&real_root).open().expect("bundle opens");
    assert_eq!(
        loaded.log.len(),
        THREADS * 2,
        "aliases and target share one linear canonical log"
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
    assert!(
        fs::symlink_metadata(&alias)
            .expect("alias metadata reads")
            .file_type()
            .is_symlink(),
        "the alias symlink survives publication"
    );

    let _ = fs::remove_dir_all(parent);
}

#[cfg(unix)]
#[test]
fn non_utf8_backup_siblings_are_recognized_losslessly() {
    use std::os::unix::ffi::OsStringExt;

    use threeterm_persistence::PRE_MIGRATION_BACKUP_SUFFIX;
    use threeterm_persistence::bundle::{SchemaStatus, detect_schema};

    let parent = unique_temp_dir("non-utf8-backup");
    fs::create_dir_all(&parent).expect("parent creates");
    let root = parent.join(std::ffi::OsString::from_vec(b"backup-\xff".to_vec()));
    write_v0_fixture(&root, ProjectGeneration::with_id("g-non-utf8-backup"))
        .expect("v0 fixture writes");
    let backup = root.with_file_name({
        let mut name = root.file_name().expect("root has a name").to_os_string();
        name.push(PRE_MIGRATION_BACKUP_SUFFIX);
        name
    });

    load(&root).expect("v0 migrates");
    assert!(
        backup.exists(),
        "the pre-migration backup sibling is retained"
    );
    assert_eq!(
        detect_schema(&backup).expect("backup classifies"),
        SchemaStatus::Unknown,
        "a non-UTF-8 backup is recognized as a backup and never migratable"
    );

    let _ = fs::remove_dir_all(parent);
}

#[test]
fn interrupted_backup_creation_is_repaired_on_retry() {
    use threeterm_persistence::PRE_MIGRATION_BACKUP_SUFFIX;
    use threeterm_persistence::bundle::{V0Manifest, prior_schema_epoch, schema_epoch};

    let root = unique_temp_dir("partial-backup");
    write_v0_fixture(&root, ProjectGeneration::with_id("g-partial-backup"))
        .expect("v0 fixture writes");
    let backup = root.with_file_name(format!(
        "{}{PRE_MIGRATION_BACKUP_SUFFIX}",
        root.file_name().expect("root has a name").to_string_lossy()
    ));
    // Simulate an interrupted backup copy: the directory exists but holds
    // only a partial manifest.
    fs::create_dir_all(backup.join("canonical")).expect("partial backup dirs create");
    fs::write(backup.join(MANIFEST_FILENAME), b"partial").expect("partial manifest writes");

    let loaded = load(&root).expect("migration replaces the partial backup and proceeds");
    assert_eq!(loaded.manifest.schema_version, schema_epoch());

    let backup_manifest_raw =
        fs::read(backup.join(MANIFEST_FILENAME)).expect("repaired backup manifest reads");
    let backup_manifest: V0Manifest =
        serde_json::from_slice(&backup_manifest_raw).expect("repaired backup parses as v0");
    assert_eq!(backup_manifest.schema_version, prior_schema_epoch());
    assert!(
        backup.join("canonical/transactions.ndjson").is_file(),
        "the repaired backup is a complete v0 copy"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(backup);
}

#[test]
fn lock_identity_survives_bundle_path_replacement() {
    let root = unique_temp_dir("lock-identity-replacement");
    let bundle = std::sync::Arc::new(
        Bundle::create_for_test(&root, "00".repeat(16).as_str()).expect("bundle creates"),
    );
    bundle
        .append_feature("box-1", "box")
        .expect("first publish");

    // The lock identity is the containing directory, which generation
    // rotation never renames. Replacing the bundle root itself — the
    // strongest replacement a concurrent writer could perform — cannot
    // split serialization across lock identities.
    let lock_file = std::fs::File::open(root.parent().expect("temp root has a parent"))
        .expect("lock directory opens");
    lock_file.lock().expect("writer lock is exclusive");
    let previous = previous_generation_sibling(&root);
    let retired = {
        let mut retired = previous.clone();
        retired.set_file_name(format!(
            "{}.retired-generation",
            previous.file_name().unwrap_or_default().to_string_lossy()
        ));
        retired
    };
    fs::rename(&previous, &retired).expect("preceding generation retires");
    fs::rename(&root, &previous).expect("bundle root replaced mid-operation");

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let writer = {
        let bundle = bundle.clone();
        std::thread::spawn(move || {
            started_tx.send(()).expect("started signal sends");
            let result = bundle.append_feature("box-w1", "box");
            done_tx.send(()).expect("completion signal sends");
            result
        })
    };
    started_rx
        .recv()
        .expect("writer is running before the replacement");
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "a writer must block while the lock identity is held"
    );

    fs::rename(&previous, &root).expect("bundle root restored");
    lock_file.unlock().expect("writer releases the lock");
    writer
        .join()
        .expect("writer completes")
        .expect("writer serializes against the held lock identity");

    let loaded = bundle.open().expect("bundle opens");
    assert_eq!(
        loaded.log.len(),
        2,
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

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous);
    let _ = fs::remove_dir_all(retired);
}

#[cfg(unix)]
#[test]
fn pre_existing_migration_staging_symlink_is_skipped() {
    use std::os::unix::fs::symlink;

    use threeterm_persistence::bundle::schema_epoch;

    let root = unique_temp_dir("migration-staging-symlink");
    write_v0_fixture(&root, ProjectGeneration::with_id("g-staging-symlink"))
        .expect("v0 fixture writes");
    let staging = root.with_file_name(format!(
        "{}.migrate-tmp-{}",
        root.file_name().expect("root has a name").to_string_lossy(),
        std::process::id()
    ));
    let target = unique_temp_dir("staging-target");
    fs::create_dir_all(&target).expect("target creates");
    symlink(&target, &staging).expect("planted staging symlink creates");

    let loaded = load(&root).expect("migration skips the symlinked staging candidate");
    assert_eq!(loaded.manifest.schema_version, schema_epoch());
    assert!(
        fs::symlink_metadata(&staging)
            .expect("staging metadata reads")
            .file_type()
            .is_symlink(),
        "the planted staging symlink is left untouched"
    );
    assert!(
        !fs::symlink_metadata(root.join(MANIFEST_FILENAME))
            .expect("manifest metadata reads")
            .file_type()
            .is_symlink(),
        "the canonical manifest is a real file, not the symlink target"
    );
    assert!(
        fs::read_dir(&target)
            .expect("target reads")
            .next()
            .is_none(),
        "migration writes nothing through the symlink into the target"
    );

    let _ = fs::remove_dir_all(target);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(previous_generation_sibling(&root));
}

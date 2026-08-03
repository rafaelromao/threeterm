use std::fs;
use std::path::{Path, PathBuf};

use threeterm_persistence::PREVIOUS_GENERATION_SUFFIX;
use threeterm_persistence::bundle::{
    Bundle, EMPTY_LOG_DIGEST_HEX, MANIFEST_FILENAME, TRANSACTIONS_LOG_FILENAME,
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

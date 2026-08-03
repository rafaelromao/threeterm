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

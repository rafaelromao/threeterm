use std::fs;
use std::path::{Path, PathBuf};

use threeterm_persistence::PREVIOUS_GENERATION_SUFFIX;
use threeterm_persistence::bundle::{Bundle, MANIFEST_FILENAME, TRANSACTIONS_LOG_FILENAME};

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
    let preceding_log = fs::read(root.join(TRANSACTIONS_LOG_FILENAME)).expect("preceding log reads");

    bundle
        .append_feature("box-2", "box")
        .expect("second save publishes");

    let previous = previous_generation_sibling(&root);
    assert!(previous.is_dir(), "preceding generation is retained on disk");
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

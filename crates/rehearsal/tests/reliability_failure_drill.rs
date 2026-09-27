use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rehearsal::{
    RELIABILITY_TEST_ID, run_reliability_failure_drill, verify_reliability_failure_evidence,
};

fn temporary_root(label: &str) -> PathBuf {
    if let Ok(base) = std::env::var("THREETERM_RELIABILITY_EVIDENCE_ROOT") {
        return PathBuf::from(base).join(label);
    }
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-{label}-{}-{suffix}", std::process::id()))
}

#[test]
fn reliability_failure_drill_isolation_bounded_cleanup_and_retained_evidence() {
    let first = temporary_root("reliability-first");
    let second = temporary_root("reliability-second");
    fs::create_dir_all(&first).expect("first evidence root creates");
    fs::create_dir_all(&second).expect("second evidence root creates");
    fs::write(first.join("sentinel.txt"), b"first-journey").expect("first sentinel writes");
    fs::write(second.join("sentinel.txt"), b"second-journey").expect("second sentinel writes");

    let first_report = run_reliability_failure_drill(&first).expect("first failure drill runs");
    let second_report = run_reliability_failure_drill(&second).expect("second failure drill runs");

    assert_eq!(first_report["test"], RELIABILITY_TEST_ID);
    assert_eq!(first_report["surface"], "api");
    assert_eq!(first_report["tool"], "extrude");
    assert_eq!(first_report["recipe_step"], "injected-worker-failure");
    assert_eq!(
        first_report["semantic_feature"],
        "reliability-failure-feature"
    );
    assert_eq!(first_report["screenshots"]["status"], "not_applicable");
    assert_eq!(first_report["cleanup"]["forced"], true);
    assert_eq!(
        first_report["cleanup"]["remaining_processes"],
        serde_json::json!([])
    );
    assert!(first_report["diagnostic"]["artifact_location"].is_string());

    verify_reliability_failure_evidence(&first).expect("first evidence verifies");
    verify_reliability_failure_evidence(&second).expect("second evidence verifies");
    assert_eq!(
        fs::read(first.join("sentinel.txt")).expect("first sentinel reads"),
        b"first-journey"
    );
    assert_eq!(
        fs::read(second.join("sentinel.txt")).expect("second sentinel reads"),
        b"second-journey"
    );
    assert!(first.join("project/manifest.json").is_file());
    assert!(first.join("config/worker.sh").is_file());
    assert!(first.join("logs/worker.stderr").is_file());
    assert!(first.join("evidence/transcript.jsonl").is_file());
    assert!(first.join("artifacts/manifest.json").is_file());
    assert!(
        !second_report["diagnostic"]["artifact_location"]
            .as_str()
            .expect("second artifact location")
            .contains("reliability-first")
    );
}

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_cli::rehearsal::{run_all_adversarial_cases, verify_adversarial_evidence};
use threeterm_occt_worker::OcctWorker;

fn temp_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-rehearsal-package-{suffix}"))
}

#[test]
fn production_adversarial_cases_preserve_canonical_state() {
    if OcctWorker::locate().is_err() {
        eprintln!("rehearsal: no OCCT worker binary found; pinned CI runs this production path");
        return;
    }

    let output_dir = temp_root();
    let manifest = run_all_adversarial_cases(&output_dir).expect("all adversarial cases run");
    assert_eq!(
        manifest["cases"],
        serde_json::json!(["mismatch-cache", "schema-v0", "capability-loss"])
    );
    verify_adversarial_evidence(&output_dir).expect("adversarial evidence verifies");

    for (case, diagnostic) in [
        ("mismatch-cache", "LAYER_1_FINGERPRINT_MISMATCH"),
        ("schema-v0", "SCHEMA_EPOCH_V0_REQUIRES_BACKUP"),
        ("capability-loss", "CAPABILITY_LOSS"),
    ] {
        let report: Value = serde_json::from_slice(
            &fs::read(output_dir.join(case).join("report.json")).expect("case report reads"),
        )
        .expect("case report is JSON");
        assert_eq!(report["diagnostic"]["code"], diagnostic);
        assert_eq!(report["canonical_byte_equal"], true);
    }

    fs::remove_dir_all(output_dir).expect("temporary evidence removes");
}

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_persistence::bundle::{PUBLICATION_KILL_POINT_ENV, PublicationKillPoint};
use threeterm_persistence::previous_generation_path;

#[derive(Debug, Clone, PartialEq, Eq)]
struct GenerationHashes {
    feature_graph: String,
    revision: String,
}

impl GenerationHashes {
    fn from_response(value: &Value, context: &str) -> Self {
        Self {
            feature_graph: value["feature_graph_hash"]
                .as_str()
                .unwrap_or_else(|| panic!("{context}: feature graph hash is missing"))
                .to_string(),
            revision: value["revision_hash"]
                .as_str()
                .unwrap_or_else(|| panic!("{context}: revision hash is missing"))
                .to_string(),
        }
    }
}

fn unique_scenario(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-recovery-{label}-{suffix}"))
}

fn run_save(root: &Path, feature_id: &str, kill_point: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_threeterm"));
    command.args(["--machine", "save"]).arg(root).args([
        "--feature-id",
        feature_id,
        "--kind",
        "box",
    ]);
    if let Some(kill_point) = kill_point {
        command.env(PUBLICATION_KILL_POINT_ENV, kill_point);
    }
    command.output().expect("save process runs")
}

fn run_load(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "load"])
        .arg(root)
        .output()
        .expect("load process runs")
}

fn response(output: &Output, operation: &str) -> Value {
    assert!(
        output.status.success(),
        "{operation} failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("command response is JSON")
}

#[test]
fn interrupted_save_at_staged_files_reopens_the_pre_save_generation() {
    let scenario = unique_scenario("staged-files");
    let root = scenario.join("project");

    response(&run_save(&root, "box-1", None), "initial save");
    let before = response(&run_save(&root, "box-2", None), "second save");

    let interrupted = run_save(&root, "box-3", Some("staged-files"));
    assert!(
        !interrupted.status.success(),
        "staged-files: interrupted save unexpectedly succeeded"
    );
    assert!(
        interrupted.stdout.is_empty(),
        "staged-files: interrupted save emitted a success response: {}",
        String::from_utf8_lossy(&interrupted.stdout)
    );

    let loaded = response(&run_load(&root), "load after staged-files interruption");
    assert_eq!(loaded["feature_graph_hash"], before["feature_graph_hash"]);
    assert_eq!(loaded["revision_hash"], before["revision_hash"]);

    let _ = fs::remove_dir_all(scenario);
}

#[test]
fn interrupted_save_at_every_publication_boundary_reopens_only_a_complete_generation() {
    let control_scenario = unique_scenario("control");
    let control_root = control_scenario.join("project");
    let older_control = GenerationHashes::from_response(
        &response(
            &run_save(&control_root, "box-1", None),
            "control initial save",
        ),
        "control older generation",
    );
    let before_control = GenerationHashes::from_response(
        &response(
            &run_save(&control_root, "box-2", None),
            "control second save",
        ),
        "control pre-save generation",
    );
    let candidate_control = GenerationHashes::from_response(
        &response(
            &run_save(&control_root, "box-3", None),
            "control candidate save",
        ),
        "control candidate generation",
    );

    for point in PublicationKillPoint::ALL {
        let scenario = unique_scenario(point.as_str());
        let root = scenario.join("project");
        let older = GenerationHashes::from_response(
            &response(&run_save(&root, "box-1", None), "case initial save"),
            "case older generation",
        );
        let before = GenerationHashes::from_response(
            &response(&run_save(&root, "box-2", None), "case second save"),
            "case pre-save generation",
        );
        assert_eq!(
            older, older_control,
            "{point:?}: setup older generation differs"
        );
        assert_eq!(
            before, before_control,
            "{point:?}: setup pre-save generation differs"
        );

        let interrupted = run_save(&root, "box-3", Some(point.as_str()));
        assert!(
            !interrupted.status.success(),
            "{point:?}: interrupted save unexpectedly succeeded"
        );
        assert!(
            interrupted.stdout.is_empty(),
            "{point:?}: interrupted save emitted a success response: {}",
            String::from_utf8_lossy(&interrupted.stdout)
        );

        let loaded = response(
            &run_load(&root),
            &format!("{point:?}: load after interruption"),
        );
        let observed =
            GenerationHashes::from_response(&loaded, &format!("{point:?}: recovered generation"));
        let (expected_name, expected) = match point {
            PublicationKillPoint::PromoteStaging
            | PublicationKillPoint::ParentSync
            | PublicationKillPoint::RetiredCleanup => ("candidate", &candidate_control),
            _ => ("pre-save", &before),
        };
        assert_eq!(
            observed, *expected,
            "{point:?}: recovered {expected_name} generation does not match the complete generation"
        );

        let recovered_from_previous = loaded["recovered_from_previous"]
            .as_bool()
            .unwrap_or_else(|| panic!("{point:?}: load response lacks recovery status"));
        assert_eq!(
            recovered_from_previous,
            point == PublicationKillPoint::ReplaceCurrent,
            "{point:?}: recovery status does not identify the selected slot"
        );

        let previous = previous_generation_path(&root);
        assert!(
            previous.is_dir(),
            "{point:?}: immediately preceding recovery slot is missing"
        );
        let previous_loaded = response(
            &run_load(&previous),
            &format!("{point:?}: load previous recovery slot"),
        );
        let expected_previous = match point {
            PublicationKillPoint::ReplaceCurrent
            | PublicationKillPoint::PromoteStaging
            | PublicationKillPoint::ParentSync
            | PublicationKillPoint::RetiredCleanup => &before,
            _ => &older,
        };
        assert_eq!(
            GenerationHashes::from_response(
                &previous_loaded,
                &format!("{point:?}: previous generation"),
            ),
            *expected_previous,
            "{point:?}: previous recovery slot is not the immediately preceding complete generation"
        );

        let _ = fs::remove_dir_all(scenario);
    }

    let _ = fs::remove_dir_all(control_scenario);
}

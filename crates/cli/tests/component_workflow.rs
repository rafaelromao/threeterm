use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_persistence::load;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(args)
        .output()
        .expect("threeterm binary runs")
}

fn run_ok(args: &[&str]) -> Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("response is JSON")
}

fn unique_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-component-workflow-{suffix}"))
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("temporary path is UTF-8")
}

#[test]
fn reusable_definition_and_two_instances_reopen_from_the_canonical_log() {
    let root = unique_root();
    run_ok(&["new-project", path_text(&root)]);

    let define = json!({
        "definition_id": "definition-l-bracket",
        "features": [{
            "id": "feature-l-bracket",
            "kind": "l-bracket",
            "parameters": {
                "height_mm": 40,
                "thickness_mm": 4,
                "width_mm": 30
            },
            "references": []
        }]
    });
    run_ok(&[
        "--machine",
        "define-component",
        path_text(&root),
        &define.to_string(),
    ]);

    for (instance_id, translation, rotation_degrees) in [
        ("instance-one", [0, 0, 0], [0, 0, 0]),
        ("instance-two", [60, 0, 0], [0, 0, 90]),
    ] {
        let place = json!({
            "definition_id": "definition-l-bracket",
            "instance_id": instance_id,
            "transform": {
                "rotation_degrees": rotation_degrees,
                "translation_micrometers": translation
            }
        });
        run_ok(&[
            "--machine",
            "place-instance",
            path_text(&root),
            &place.to_string(),
        ]);
    }

    let loaded = load(&root).expect("component bundle reopens");
    let revision = loaded.generation.current_revision();
    assert_eq!(loaded.manifest.transaction_count, 3);
    assert_eq!(loaded.manifest.revision_count, 4);
    assert_eq!(revision.component_graph.definitions.len(), 1);
    assert_eq!(revision.component_graph.instances.len(), 2);
    assert!(
        revision
            .component_graph
            .instances
            .iter()
            .all(|instance| instance.definition_id.as_str() == "definition-l-bracket")
    );
    assert_eq!(revision.component_graph.definitions[0].features.len(), 1);

    let _ = fs::remove_dir_all(root);
}

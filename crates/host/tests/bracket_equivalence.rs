use serde_json::Value;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::bracket_equivalence::{
    GeometrySignature, JourneyMetadata, MismatchKind, ProbeSample, SourceIdentity, SurfaceSample,
    TolerancePolicy, TopologySignature, compare_signatures, compare_three_reports,
    configured_run_id, evidence_root, new_journey_evidence_report, publish_journey_evidence_report,
    write_aggregate_failure, write_aggregate_report,
};

fn signature() -> GeometrySignature {
    GeometrySignature {
        dimensions: [60.0, 60.0, 20.0],
        material_volume: 20_000.0,
        topology: TopologySignature {
            shell_count: 2,
            material_shell_count: 1,
            cavity_shell_count: 1,
        },
        voids: vec![ProbeSample {
            name: "cavity".to_string(),
            occupied: false,
            intersections: vec![],
        }],
        landmarks: vec![ProbeSample {
            name: "collar".to_string(),
            occupied: true,
            intersections: vec![0.0, 20.0],
        }],
        surface_samples: vec![SurfaceSample {
            name: "arm-x".to_string(),
            intersections: vec![0.0, 8.0, 12.0],
        }],
    }
}

fn tolerances() -> TolerancePolicy {
    TolerancePolicy {
        linear_mm: 0.05,
        placement_mm: 0.10,
        volume_fraction: 0.05,
    }
}

type SignatureMutation = fn(&mut GeometrySignature);

#[test]
fn signature_comparison_ignores_mesh_order_and_raw_bytes() {
    let mut reordered = signature();
    reordered.surface_samples[0].intersections.reverse();

    compare_signatures(
        "api",
        &signature(),
        "mcp",
        &reordered,
        tolerances(),
        20_000.0,
    )
    .expect("geometrically equivalent signatures pass");
}

#[test]
fn signature_comparison_reports_volume_mismatches_at_the_frozen_boundary() {
    let mut changed = signature();
    changed.material_volume = 21_100.0;

    let failure = compare_signatures("api", &signature(), "tui", &changed, tolerances(), 20_000.0)
        .expect_err("volume mismatch must fail");
    assert_eq!(failure.kind, MismatchKind::Volume);
}

#[test]
fn signature_comparison_identifies_non_volume_mismatch_categories() {
    let cases: [(MismatchKind, SignatureMutation); 5] = [
        (
            MismatchKind::Dimensions,
            |changed: &mut GeometrySignature| {
                changed.dimensions[0] += 0.06;
            },
        ),
        (MismatchKind::Topology, |changed: &mut GeometrySignature| {
            changed.topology.shell_count += 1;
        }),
        (MismatchKind::Voids, |changed: &mut GeometrySignature| {
            changed.voids[0].occupied = true;
        }),
        (
            MismatchKind::Landmarks,
            |changed: &mut GeometrySignature| {
                changed.landmarks[0].intersections[1] += 0.11;
            },
        ),
        (
            MismatchKind::SurfaceSamples,
            |changed: &mut GeometrySignature| {
                changed.surface_samples[0].intersections[1] += 0.06;
            },
        ),
    ];

    for (expected_kind, change) in cases {
        let mut changed = signature();
        change(&mut changed);
        let failure =
            compare_signatures("api", &signature(), "mcp", &changed, tolerances(), 20_000.0)
                .expect_err("changed geometry must fail");
        assert_eq!(failure.kind, expected_kind);
    }
}

#[test]
fn retained_journey_publication_copies_and_hashes_the_artifact() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-equivalence-report-{suffix}"));
    let source = root.join("source.stl");
    let recipe: Value = serde_json::from_str(include_str!("data/bracket_complete_recipe.v1.json"))
        .expect("complete recipe parses");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/research/rehearsal-evidence/l-bracket/run-2/export/l-bracket.stl");
    fs::create_dir_all(&root).expect("report fixture root creates");
    fs::copy(fixture, &source).expect("report fixture copies");
    let report = new_journey_evidence_report(
        "run-report-test",
        "api",
        "e2e_stl_api_all_tools_l_bracket",
        &recipe,
        JourneyMetadata {
            source: SourceIdentity {
                commit: "a".repeat(40),
                dirty: false,
            },
            schemas: serde_json::json!({"command_registry": "registry"}),
            workers: serde_json::json!({"occt": "worker"}),
            runtime: serde_json::json!({"adapter": "test"}),
            evidence: serde_json::json!({"executions": []}),
        },
        "revision",
    );
    let report_path = publish_journey_evidence_report(&root, report, &source)
        .expect("report publication succeeds");
    let retained: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).expect("retained report reads"))
            .expect("retained report parses");
    assert_eq!(retained["artifact"]["path"], "api/complete-bracket.stl");
    assert!(retained["artifact"]["bytes"].as_u64().unwrap_or_default() > 0);
    assert!(root.join("api/complete-bracket.stl").is_file());
    fs::remove_dir_all(root).expect("report fixture root removes");
}

#[test]
#[ignore = "requires retained API, MCP, and qualified TUI journey evidence"]
fn e2e_stl_three_surface_geometric_equivalence() {
    let run_id = configured_run_id().expect("THREETERM_JOURNEY_RUN_ID stages one run");
    let recipe: Value = serde_json::from_str(include_str!("data/bracket_complete_recipe.v1.json"))
        .expect("complete recipe parses");
    let root = evidence_root();
    match compare_three_reports(&root, &run_id, &recipe) {
        Ok(report) => {
            write_aggregate_report(&root, &report).expect("aggregate evidence publishes");
            assert_eq!(report.comparisons.len(), 3);
        }
        Err(error) => {
            write_aggregate_failure(&root, &run_id, &error.to_string())
                .expect("aggregate failure evidence publishes");
            panic!("three-surface bracket equivalence failed: {error}");
        }
    }
}

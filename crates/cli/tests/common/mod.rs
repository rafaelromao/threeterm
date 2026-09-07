use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, new_request_id};
use threeterm_protocol::artifact::sha256_hex;

pub fn extrude_canonical(root: &Path, feature_id: &str, profile: Value, height: f64) {
    let profile = serde_json::from_value::<Vec<(f64, f64)>>(profile)
        .expect("profile schema contains coordinate pairs");
    let worker = OcctWorker::locate().expect("OCCT worker locates");
    Host::new()
        .extrude(
            root,
            ExtrudeRequest::new(new_request_id(), profile, height)
                .with_output_path(root.join("test-stage"), format!("{feature_id}.brep"))
                .with_feature_id(feature_id),
            &worker,
        )
        .expect("canonical fixture extrude succeeds");
}

#[allow(dead_code)]
pub fn selected_edge_file(
    root: &Path,
    source_feature_id: &str,
    source_revision: &str,
    label: &str,
) -> PathBuf {
    let worker = OcctWorker::locate().expect("OCCT worker locates");
    let inspection = worker
        .inspect_edges(
            new_request_id(),
            root.join("brep").join(format!("{source_feature_id}.brep")),
            source_feature_id,
            source_revision,
            serde_json::json!({
                "semantic_id": "requested-edge",
                "source_feature_id": source_feature_id,
                "source_revision_id": source_revision,
                "source_edge_id": "requested-edge",
                "role": "outer-perimeter",
                "midpoint": [0.0, 0.0, 0.0],
                "tangent": [1.0, 0.0, 0.0],
                "length": 1.0
            }),
        )
        .expect("edge inspection succeeds");
    let mut semantic_id_counts = HashMap::new();
    for candidate in &inspection.edge_candidates {
        let semantic_id = edge_semantic_id(candidate);
        *semantic_id_counts.entry(semantic_id).or_insert(0) += 1;
    }
    let candidate = inspection
        .edge_candidates
        .iter()
        .find(|candidate| semantic_id_counts[&edge_semantic_id(candidate)] == 1)
        .expect("edge inspection returns an unambiguous candidate");
    let semantic_id = edge_semantic_id(candidate);
    let selected_edge = serde_json::json!({
        "semantic_id": semantic_id,
        "provenance": {
            "source_feature_id": source_feature_id,
            "source_revision_id": source_revision,
            "source_edge_id": "requested-edge"
        },
        "role": candidate.role,
        "evidence": {
            "midpoint": candidate.midpoint,
            "tangent": candidate.tangent,
            "length": candidate.length
        }
    });
    let path = root.join(format!("{label}-selected-edge.json"));
    fs::write(
        &path,
        serde_json::to_vec(&selected_edge).expect("selected edge serializes"),
    )
    .expect("selected edge file writes");
    path
}

fn edge_semantic_id(candidate: &threeterm_occt_worker::EdgeCandidateEvidence) -> String {
    format!(
        "edge-{}",
        sha256_hex(
            &serde_json::to_vec(&(candidate.midpoint, candidate.tangent, candidate.length))
                .expect("edge evidence serializes")
        )
    )
}

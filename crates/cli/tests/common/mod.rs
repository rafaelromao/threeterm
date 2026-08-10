use std::path::Path;

use serde_json::Value;
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, new_request_id};

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

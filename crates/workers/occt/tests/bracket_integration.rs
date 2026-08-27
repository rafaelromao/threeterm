use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_occt_worker::{BracketRequest, OcctWorker, new_request_id};

#[test]
fn production_occt_worker_builds_a_real_l_bracket_from_dimensions() {
    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() {
                panic!(
                    "{{\"code\":\"worker_unavailable\",\"worker\":\"occt\",\"detail\":\"{error}\"}}"
                );
            }
            eprintln!("bracket_integration: OCCT worker unavailable: {error}");
            return;
        }
    };
    let root = std::env::temp_dir().join(format!(
        "threeterm-occt-bracket-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("stage creates");
    let request = BracketRequest::new(new_request_id(), 100.0, 60.0, 40.0, 5.0)
        .with_output_path(&root, "bracket.brep")
        .with_feature_id("l-bracket");
    let result = worker.bracket(&request).expect("OCCT bracket succeeds");
    assert!(result.is_success());
    let bytes = fs::read(&result.brep_path).expect("BREP reads");
    assert!(String::from_utf8_lossy(&bytes[..bytes.len().min(64)]).contains("DBRep_DrawableShape"));
    assert_eq!(bytes.len(), result.brep_bytes);
    assert_ne!(result.brep_sha256, "0".repeat(64));
    let _ = fs::remove_dir_all(root);
}

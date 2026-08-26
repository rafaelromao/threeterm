use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::{Host, HostError, Layer1CacheRecord};
use threeterm_protocol::artifact::{WorkerFingerprint, sha256_hex};

fn temporary_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-layer1-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

#[test]
fn tampered_worker_fingerprint_is_discarded_before_snapshot_replacement() {
    let root = temporary_root();
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("canonical bundle saves");
    let before = host.current().expect("host snapshot exists");
    let artifact = b"non-authoritative cache bytes";
    let cache = root.join("cache");
    fs::create_dir_all(&cache).expect("cache creates");
    fs::write(cache.join("l-bracket.brep"), artifact).expect("cache artifact writes");
    let record = Layer1CacheRecord {
        schema_version: "threeterm.host.layer1-cache/1".to_string(),
        source_revision: before.revision_hash.clone(),
        operation: "bracket".to_string(),
        feature_id: "l-bracket".to_string(),
        worker_fingerprint: WorkerFingerprint {
            worker_kind: "tampered".to_string(),
            worker_schema_version: threeterm_occt_worker::SCHEMA_VERSION.to_string(),
            protocol_schema_version: threeterm_protocol::schema_version().to_string(),
        },
        artifact_name: "l-bracket.brep".to_string(),
        byte_count: artifact.len() as u64,
        sha256: sha256_hex(artifact),
    };
    fs::write(
        cache.join("layer1.json"),
        serde_json::to_vec_pretty(&record).expect("cache record serializes"),
    )
    .expect("cache record writes");

    let error = host
        .load_with_layer1_cache(&root)
        .expect_err("tampered cache must fail closed");

    assert!(matches!(error, HostError::Layer1FingerprintMismatch { .. }));
    assert_eq!(host.current(), Some(before));
    assert!(!cache.join("layer1.json").exists());
    assert!(!cache.join("l-bracket.brep").exists());
    let _ = fs::remove_dir_all(root);
}

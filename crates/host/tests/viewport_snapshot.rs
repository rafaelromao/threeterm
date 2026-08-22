use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-viewport-snapshot-{nanos}"))
}

#[test]
fn presentation_snapshot_is_one_immutable_canonical_projection() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("first feature is persisted");

    let first = host
        .presentation_snapshot()
        .expect("the canonical host projection exists");
    assert_eq!(first.graph.features().count(), 1);
    assert_eq!(
        first.snapshot.revision_hash,
        host.current().unwrap().revision_hash
    );

    host.save(&root, "feature-b", "fillet")
        .expect("second feature is persisted");

    assert_eq!(first.graph.features().count(), 1);
    let current = host
        .presentation_snapshot()
        .expect("the updated canonical host projection exists");
    assert_eq!(current.graph.features().count(), 2);
    assert_ne!(current.snapshot.revision_hash, first.snapshot.revision_hash);

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

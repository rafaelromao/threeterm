//! Asserts the registry's published schema hash is stable across builds.
//!
//! The hash is computed from the canonical JSON encoding of the registry
//! (sorted object keys via `serde_json::Value::Object`'s default
//! `BTreeMap` backing, no whitespace). The constant is recorded once via
//! the first-run commit pattern; if the registry changes unintentionally
//! the test fails with both the actual and the expected hash.

use threeterm_protocol::schema::{LIST_COMMAND_ID, find, registry_hash};

#[test]
fn registry_hash_is_a_64_char_lowercase_hex_sha256() {
    let hash = registry_hash();

    assert_eq!(
        hash.len(),
        64,
        "SHA-256 hex digest is 64 chars; got {hash:?}"
    );
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "hash must be lowercase hex; got {hash:?}"
    );
}

#[test]
fn registry_hash_matches_the_published_constant() {
    assert_eq!(
        registry_hash(),
        "1b4b000d54a8fe2779aae388c9f334d04221374ae03d83a6f2de8f6cf400fd94",
        "registry_hash drifted from the published constant. If the registry \
         changed intentionally, update the constant in this test and rerun."
    );
}

#[test]
fn registry_hash_is_deterministic() {
    let first = registry_hash();
    let second = registry_hash();
    let third = registry_hash();

    assert_eq!(first, second);
    assert_eq!(second, third);
}

#[test]
fn registry_resolves_list_by_command_id() {
    let entry = find(LIST_COMMAND_ID).expect("`list` is the seeded entry");
    assert_eq!(entry.id, LIST_COMMAND_ID);
    assert_eq!(entry.name, "list");
}

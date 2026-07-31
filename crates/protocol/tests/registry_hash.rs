//! Asserts the registry's published schema hash is stable across builds.
//!
//! The hash is computed from the canonical JSON encoding of the registry
//! (sorted object keys, no whitespace). The constant is recorded once via
//! the first-run commit pattern; if the registry changes unintentionally
//! the test fails with both the actual and the expected hash.

use threeterm_protocol::schema::{COMMAND_REGISTRY, LIST_COMMAND_ID, registry_hash};

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
        "13ce09a166bb001959e9d15a50c92187e81e2d06804480625b1a67523ff057ec",
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
fn registry_contains_the_list_command() {
    let list_entries: Vec<_> = COMMAND_REGISTRY
        .iter()
        .filter(|entry| entry.id == LIST_COMMAND_ID)
        .collect();

    assert_eq!(
        list_entries.len(),
        1,
        "the registry must contain exactly one entry with id == `list`"
    );
}

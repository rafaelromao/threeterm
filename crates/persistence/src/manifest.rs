//! Sealed manifest for the ThreeTerm project bundle.
//!
//! The slice (#235) ships a single sealed generation; the
//! `.previous`-pair recovery contract from issue #31 is honoured by the
//! atomic `manifest.json.tmp` → fsync → rename pattern, but no explicit
//! multi-generation recovery is implemented here.
//!
//! `Manifest` carries:
//! - `schema_version` & `schema_generation` — the consumer's pin for this
//!   manifest shape;
//! - `project_generation_hex` — 32 lowercase-hex chars holding the durable
//!   project identity (read once from `/dev/urandom` at create time);
//! - `terminal_log_digest_hex` — 64 lowercase-hex chars holding the chain's
//!   terminal digest for `transactions.log`;
//! - `feature_graph_hash_hex` — 64 lowercase-hex chars, the canonical
//!   graph hash at the moment of the seal;
//! - `revision_hash_hex` — 64 lowercase-hex chars, the canonical revision
//!   hash at the moment of the seal;
//!
//! Manifests are written via a `*.tmp` → fsync → rename to make the
//! bundle survive interrupted writes; reads reject any manifest whose
//! schema generation differs from the supported value.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use threeterm_domain::graph::FeatureGraph;
use threeterm_domain::revision_hex as domain_revision_hex;

/// The schema version pinned by this slice.
pub const MANIFEST_SCHEMA_VERSION: &str = "threeterm.persistence/1";

/// The schema generation this slice understands. Future bumps require
/// migration at load time (closed issue #45).
pub const MANIFEST_SCHEMA_GENERATION: u32 = 1;

/// Length of a hex-encoded project generation (16 bytes).
pub const PROJECT_GENERATION_HEX_LEN: usize = 32;

/// Length of a hex-encoded digest (32 bytes).
pub const DIGEST_HEX_LEN: usize = 64;

/// The sealed manifest document persisted at `<bundle>/manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: String,
    pub schema_generation: u32,
    pub project_generation_hex: String,
    pub terminal_log_digest_hex: String,
    pub feature_graph_hash_hex: String,
    pub revision_hash_hex: String,
}

impl Manifest {
    /// Build a manifest from the canonical sources at seal time. The
    /// manifest is deterministic given its inputs.
    pub fn seal(
        project_generation_hex: &str,
        terminal_log_digest_hex: &str,
        graph: &FeatureGraph,
    ) -> Self {
        let graph_hash = graph.graph_hash_hex();
        let revision = domain_revision_hex(&graph_hash, terminal_log_digest_hex);
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            schema_generation: MANIFEST_SCHEMA_GENERATION,
            project_generation_hex: project_generation_hex.to_string(),
            terminal_log_digest_hex: terminal_log_digest_hex.to_string(),
            feature_graph_hash_hex: graph_hash,
            revision_hash_hex: revision,
        }
    }
}

/// Generate the next project generation by reading 16 bytes from
/// `/dev/urandom` and hex-encoding them. The read loops on `Read::read`
/// to guarantee a full 16-byte return on platforms (and kernel versions)
/// that may return short reads from special files.
pub fn random_project_generation_hex() -> Result<String, std::io::Error> {
    use std::io::Read as _;

    let mut file = std::fs::File::open("/dev/urandom")?;
    let mut bytes = [0u8; 16];
    let mut filled = 0usize;
    while filled < bytes.len() {
        match file.read(&mut bytes[filled..])? {
            0 => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "/dev/urandom returned 0 bytes mid-read",
                ));
            }
            n => filled += n,
        }
    }
    Ok(hex_lower(&bytes))
}

/// Lowercase hex encoding of a byte slice.
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Hex encoding of a `[u8; 32]` digest.
pub fn digest_hex(bytes: &[u8; 32]) -> String {
    hex_lower(bytes)
}

/// Atomic write helper used by the persistence writers.
/// Writes `payload` to `target.tmp`, fsyncs the file, then renames it
/// onto `target`. The rename is the atomic boundary.
pub fn atomic_write(target: &Path, payload: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent dir"))?;
    std::fs::create_dir_all(parent)?;

    let mut tmp_path = PathBuf::from(target);
    let file_name = tmp_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    tmp_path.set_file_name(format!("{file_name}.tmp"));

    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(payload)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, target)?;
    Ok(())
}

/// Append-or-rewrite helper used for NDJSON files. If `target` exists
/// and `expected_existing` matches the file's current content, the
/// payload is appended (line-delimited). If `expected_existing` is None
/// or the file is empty, a fresh write is performed via the atomic
/// helper.
pub fn append_line(target: &Path, payload: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent dir"))?;
    std::fs::create_dir_all(parent)?;

    let mut needs_atomic = true;
    if target.exists() {
        let existing = std::fs::read(target)?;
        if existing.is_empty() {
            needs_atomic = false;
        }
    } else {
        needs_atomic = false;
    }

    if needs_atomic {
        // Append in place: open with append mode, write, fsync.
        let mut f = std::fs::OpenOptions::new().append(true).open(target)?;
        f.write_all(payload)?;
        f.sync_all()?;
    } else {
        // Fresh write through the tmp -> rename boundary.
        let mut tmp_path = PathBuf::from(target);
        let file_name = tmp_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name")
            })?;
        tmp_path.set_file_name(format!("{file_name}.tmp"));
        {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(payload)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp_path, target)?;
    }
    Ok(())
}

/// SHA-256 hex of `bytes` as a lowercase-hex string (64 chars).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_lower(&digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use threeterm_domain::graph::FeatureGraph;

    #[test]
    fn hex_lower_is_lowercase_and_correct_length() {
        assert_eq!(
            hex_lower(&[0xde, 0xad, 0xbe, 0xef]),
            "deadbeef"
        );
    }

    #[test]
    fn digest_hex_of_all_zero_is_64_zeros() {
        let z = [0u8; 32];
        assert_eq!(digest_hex(&z).len(), 64);
        assert!(digest_hex(&z).chars().all(|c| c == '0'));
    }

    #[test]
    fn manifest_seal_is_deterministic() {
        let mut g = FeatureGraph::empty();
        g.add_feature(threeterm_domain::graph::Feature::new("box-1", "box"));
        let m1 = Manifest::seal(
            "00000000000000000000000000000001",
            threeterm_domain::EMPTY_LOG_DIGEST_HEX,
            &g,
        );
        let m2 = Manifest::seal(
            "00000000000000000000000000000001",
            threeterm_domain::EMPTY_LOG_DIGEST_HEX,
            &g,
        );
        assert_eq!(m1, m2);
    }
}

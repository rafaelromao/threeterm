//! Staged binary artifact promotion.
//!
//! The worker emits a `Artifact` envelope carrying a base64-encoded
//! payload and the SHA-256 it claims for the decoded bytes. The host
//! validates the hash, persists the bytes to a `.partial` staging path,
//! and atomically renames the file to its final filename on `promote`.
//!
//! A force-terminated run calls `discard` so the staged entry never
//! competes with the authoritative Revision Snapshot. Mirrors the
//! pattern in `crates/persistence/src/lib.rs::staging_path`; the slices
//! will converge in a future cleanup.
//!
//! See `artifact::Stage` for the public API.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};

use crate::worker::MAX_ARTIFACT_BYTES;

/// Handle returned by `Stage::write`. Holds the staging path, the final
/// destination, and the validated SHA-256 of the decoded bytes.
#[derive(Debug, Clone)]
pub struct StagedArtifact {
    pub staging_name: String,
    pub final_path: PathBuf,
    pub sha256: String,
}

/// A staging directory rooted at a host-chosen path. Every artifact
/// promoted by this `Stage` lives under `root/<staging_name>.partial`
/// until `promote` renames it to `root/<staging_name>`.
#[derive(Debug)]
pub struct Stage {
    root: PathBuf,
}

impl Stage {
    /// Open a staging directory at `root`, creating it if it doesn't
    /// exist. The directory is the namespace under which the host
    /// accumulates `.partial` files until `promote` or `discard`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ArtifactError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(ArtifactError::Io)?;
        Ok(Self { root })
    }

    /// Returns the staging root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Decode `bytes_b64`, validate `advertised_sha256` against the
    /// decoded bytes, and persist them to
    /// `<root>/<staging_name>.partial`. The returned handle carries the
    /// final path (`<root>/<staging_name>`) and the validated hash; pass
    /// it to `promote` or `discard`.
    ///
    /// Validates the payload size against `MAX_ARTIFACT_BYTES` so a
    /// hostile worker cannot exhaust the host's memory.
    pub fn write(
        &self,
        staging_name: &str,
        bytes_b64: &str,
        advertised_sha256: &str,
    ) -> Result<StagedArtifact, ArtifactError> {
        if staging_name.is_empty() {
            return Err(ArtifactError::InvalidName(String::new()));
        }
        if staging_name.contains('/') || staging_name.contains('\\') || staging_name.contains('\0')
        {
            return Err(ArtifactError::InvalidName(staging_name.to_string()));
        }

        let max_encoded = MAX_ARTIFACT_BYTES
            .saturating_add(2)
            .saturating_mul(4)
            .div_ceil(3);
        if bytes_b64.len() > max_encoded {
            return Err(ArtifactError::PayloadTooLarge {
                size: bytes_b64.len(),
                max: max_encoded,
            });
        }
        let bytes = BASE64
            .decode(bytes_b64)
            .map_err(|error| ArtifactError::Decode(error.to_string()))?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(ArtifactError::PayloadTooLarge {
                size: bytes.len(),
                max: MAX_ARTIFACT_BYTES,
            });
        }

        let sha256 = sha256_hex(&bytes);
        if sha256 != advertised_sha256 {
            return Err(ArtifactError::HashMismatch {
                expected: advertised_sha256.to_string(),
                actual: sha256,
            });
        }

        let final_path = self.root.join(staging_name);
        let staging_path = self.root.join(format!("{staging_name}.partial"));
        let mut file = fs::File::create(&staging_path).map_err(ArtifactError::Io)?;
        file.write_all(&bytes).map_err(ArtifactError::Io)?;
        file.sync_all().map_err(ArtifactError::Io)?;

        Ok(StagedArtifact {
            staging_name: staging_name.to_string(),
            final_path,
            sha256,
        })
    }

    /// Atomically rename `<root>/<staging_name>.partial` to
    /// `<root>/<staging_name>`. After `promote` returns, the partial is
    /// gone and the final path exists with the validated bytes.
    pub fn promote(&self, artifact: StagedArtifact) -> Result<PathBuf, ArtifactError> {
        let staging_path = self.root.join(format!("{}.partial", artifact.staging_name));
        if !staging_path.exists() {
            return Err(ArtifactError::NotStaged(artifact.staging_name));
        }
        fs::rename(&staging_path, &artifact.final_path).map_err(ArtifactError::Rename)?;
        Ok(artifact.final_path)
    }

    /// Remove the staging directory and every `.partial` file it
    /// contains. Called by the supervisor on force-terminate so the
    /// host never holds an authoritative-looking staged entry. The
    /// returned `Stage` is consumed; create a fresh one if needed.
    pub fn discard(self) -> Result<(), ArtifactError> {
        fs::remove_dir_all(&self.root).map_err(ArtifactError::Io)?;
        Ok(())
    }
}

/// SHA-256 hex digest of `bytes`, lowercase, 64 characters.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

/// Errors emitted by `Stage`. Every variant is a structured, presentation-
/// neutral identifier the supervisor routes into the diagnostic taxonomy.
#[derive(Debug)]
pub enum ArtifactError {
    /// The worker's advertised SHA-256 did not match the decoded bytes.
    HashMismatch { expected: String, actual: String },
    /// The base64 payload could not be decoded.
    Decode(String),
    /// The decoded payload exceeded `MAX_ARTIFACT_BYTES`.
    PayloadTooLarge { size: usize, max: usize },
    /// The staging name was empty or contained a path separator.
    InvalidName(String),
    /// `promote` was called for an artifact whose `.partial` file no
    /// longer exists (e.g. after `discard`).
    NotStaged(String),
    /// Filesystem error during write or discard.
    Io(std::io::Error),
    /// Filesystem error during the atomic rename.
    Rename(std::io::Error),
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HashMismatch { expected, actual } => write!(
                formatter,
                "staged artifact hash mismatch: worker advertised {expected:?}, host computed {actual:?}"
            ),
            Self::Decode(detail) => {
                write!(formatter, "staged artifact base64 decode failed: {detail}")
            }
            Self::PayloadTooLarge { size, max } => write!(
                formatter,
                "staged artifact exceeds maximum size: {size} > {max}"
            ),
            Self::InvalidName(name) => {
                write!(formatter, "staged artifact name is invalid: {name:?}")
            }
            Self::NotStaged(name) => write!(formatter, "no staged artifact named {name:?}"),
            Self::Io(error) => write!(formatter, "staged artifact io error: {error}"),
            Self::Rename(error) => write!(formatter, "staged artifact rename error: {error}"),
        }
    }
}

impl std::error::Error for ArtifactError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("threeterm-stage-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn write_promotes_a_validated_artifact_to_its_final_path() {
        let root = temp_root("promote");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";
        let sha = sha256_hex(bytes);
        let encoded = BASE64.encode(bytes);

        let artifact = stage
            .write("sketch-1.brep", &encoded, &sha)
            .expect("artifact stages");
        let final_path = stage.promote(artifact).expect("artifact promotes");

        assert_eq!(final_path, root.join("sketch-1.brep"));
        let promoted = fs::read(&final_path).expect("promoted file reads");
        assert_eq!(promoted, bytes);
        assert!(
            !root.join("sketch-1.brep.partial").exists(),
            "partial must be removed after promotion"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_rejects_an_artifact_whose_advertised_hash_does_not_match() {
        let root = temp_root("mismatch");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";
        let encoded = BASE64.encode(bytes);

        let error = stage
            .write("sketch-1.brep", &encoded, "deadbeef")
            .expect_err("hash mismatch must reject the artifact");
        match error {
            ArtifactError::HashMismatch { expected, actual } => {
                assert_eq!(expected, "deadbeef");
                assert_eq!(actual, sha256_hex(bytes));
            }
            other => panic!("expected HashMismatch; got {other:?}"),
        }

        assert!(
            !root.join("sketch-1.brep.partial").exists(),
            "rejected artifacts must not leave a partial behind"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_rejects_a_payload_that_decodes_to_more_than_max_artifact_bytes() {
        let root = temp_root("oversize");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = vec![0u8; MAX_ARTIFACT_BYTES + 1];
        let sha = sha256_hex(&bytes);
        let encoded = BASE64.encode(&bytes);

        let error = stage
            .write("oversize.brep", &encoded, &sha)
            .expect_err("oversize artifact must be rejected");
        match error {
            ArtifactError::PayloadTooLarge { size, max } => {
                assert_eq!(size, bytes.len());
                assert_eq!(max, MAX_ARTIFACT_BYTES);
            }
            other => panic!("expected PayloadTooLarge; got {other:?}"),
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_rejects_a_staging_name_that_contains_a_path_separator() {
        let root = temp_root("path-separator");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";
        let sha = sha256_hex(bytes);
        let encoded = BASE64.encode(bytes);

        let error = stage
            .write("nested/file.brep", &encoded, &sha)
            .expect_err("separator-bearing name must be rejected");
        assert!(
            matches!(error, ArtifactError::InvalidName(_)),
            "expected InvalidName; got {error:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn discard_removes_the_staging_directory_and_every_partial() {
        let root = temp_root("discard");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";
        let sha = sha256_hex(bytes);
        let encoded = BASE64.encode(bytes);
        let _ = stage
            .write("sketch-1.brep", &encoded, &sha)
            .expect("artifact stages");

        stage.discard().expect("discard succeeds");
        assert!(
            !root.exists(),
            "staging directory must be gone after discard"
        );
    }
}

//! Staged binary artifact promotion.
//!
//! The worker writes bytes to a host-chosen private `.partial` path and
//! emits an `Artifact` header declaring identity, byte count, and SHA-256.
//! The host validates the staged file independently and atomically renames
//! it to its final filename on `promote`.
//!
//! A force-terminated run calls `discard` so the staged entry never
//! competes with the authoritative Revision Snapshot. Mirrors the
//! pattern in `crates/persistence/src/lib.rs::staging_path`; the slices
//! will converge in a future cleanup.
//!
//! See `artifact::Stage` for the public API.

use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::worker::MAX_ARTIFACT_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkerFingerprint {
    pub worker_kind: String,
    pub worker_schema_version: String,
    pub protocol_schema_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer1ArtifactRequest {
    pub request_id: String,
    pub source_revision_id: String,
    pub artifact_kind: String,
    pub staging_name: String,
    pub semantic_input_sha256: String,
    pub deterministic_settings_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Layer1CacheKey {
    pub source_revision_id: String,
    pub worker_fingerprint: WorkerFingerprint,
    pub artifact_kind: String,
    pub semantic_input_sha256: String,
    pub deterministic_settings_sha256: String,
}

impl Layer1CacheKey {
    pub fn issue(request: &Layer1ArtifactRequest, worker_fingerprint: &WorkerFingerprint) -> Self {
        Self {
            source_revision_id: request.source_revision_id.clone(),
            worker_fingerprint: worker_fingerprint.clone(),
            artifact_kind: request.artifact_kind.clone(),
            semantic_input_sha256: request.semantic_input_sha256.clone(),
            deterministic_settings_sha256: request.deterministic_settings_sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactHeader {
    pub request_id: String,
    pub source_revision_id: String,
    pub cache_key: Layer1CacheKey,
    pub worker_fingerprint: WorkerFingerprint,
    pub artifact_kind: String,
    pub staging_name: String,
    pub byte_count: u64,
    pub sha256: String,
}

/// Metadata returned after worker bytes are staged.
#[derive(Debug, Clone)]
pub struct StagedArtifact {
    pub staging_name: String,
    pub sha256: String,
    pub byte_count: u64,
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
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ArtifactError::InvalidRoot(root));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&root).map_err(ArtifactError::Io)?;
            }
            Err(error) => return Err(ArtifactError::Io(error)),
        }
        let metadata = fs::symlink_metadata(&root).map_err(ArtifactError::Io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ArtifactError::InvalidRoot(root));
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(ArtifactError::Io)?;
        Ok(Self { root })
    }

    /// Returns the staging root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn stage_bytes(
        &self,
        staging_name: &str,
        bytes: &[u8],
    ) -> Result<StagedArtifact, ArtifactError> {
        if staging_name.is_empty()
            || staging_name.contains('/')
            || staging_name.contains('\\')
            || staging_name.contains('\0')
        {
            return Err(ArtifactError::InvalidName(staging_name.to_string()));
        }
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(ArtifactError::PayloadTooLarge {
                size: bytes.len(),
                max: MAX_ARTIFACT_BYTES,
            });
        }
        let sha256 = sha256_hex(bytes);
        let staging_path = self.root.join(format!("{staging_name}.partial"));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(0o400000)
            .open(&staging_path)
            .map_err(ArtifactError::Io)?;
        file.write_all(bytes).map_err(ArtifactError::Io)?;
        file.sync_all().map_err(ArtifactError::Io)?;
        Ok(StagedArtifact {
            staging_name: staging_name.to_string(),
            sha256,
            byte_count: bytes.len() as u64,
        })
    }

    pub fn validate_and_promote(&self, header: &ArtifactHeader) -> Result<PathBuf, ArtifactError> {
        if header.staging_name.is_empty()
            || header.staging_name.contains('/')
            || header.staging_name.contains('\\')
            || header.staging_name.contains('\0')
        {
            return Err(ArtifactError::InvalidName(header.staging_name.clone()));
        }
        let staging_path = self.root.join(format!("{}.partial", header.staging_name));
        let verified_path = self.root.join(format!(".{}.verified", header.staging_name));
        let final_path = self.root.join(&header.staging_name);
        let result = (|| {
            let metadata = fs::symlink_metadata(&staging_path).map_err(ArtifactError::Io)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ArtifactError::NotRegularFile(header.staging_name.clone()));
            }
            if metadata.len() > MAX_ARTIFACT_BYTES as u64 {
                return Err(ArtifactError::PayloadTooLarge {
                    size: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
                    max: MAX_ARTIFACT_BYTES,
                });
            }
            let mut source = OpenOptions::new()
                .read(true)
                .custom_flags(0o400000)
                .open(&staging_path)
                .map_err(ArtifactError::Io)?;
            let mut verified = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .custom_flags(0o400000)
                .open(&verified_path)
                .map_err(ArtifactError::Io)?;
            let mut digest = Sha256::new();
            let mut byte_count = 0u64;
            let mut buffer = [0u8; 8192];
            loop {
                let read = source.read(&mut buffer).map_err(ArtifactError::Io)?;
                if read == 0 {
                    break;
                }
                byte_count = byte_count.saturating_add(read as u64);
                if byte_count > MAX_ARTIFACT_BYTES as u64 {
                    return Err(ArtifactError::PayloadTooLarge {
                        size: usize::try_from(byte_count).unwrap_or(usize::MAX),
                        max: MAX_ARTIFACT_BYTES,
                    });
                }
                digest.update(&buffer[..read]);
                verified
                    .write_all(&buffer[..read])
                    .map_err(ArtifactError::Io)?;
            }
            verified.sync_all().map_err(ArtifactError::Io)?;
            if byte_count != header.byte_count {
                return Err(ArtifactError::ByteCountMismatch {
                    expected: header.byte_count,
                    actual: byte_count,
                });
            }
            let digest = digest.finalize();
            let sha256 = hex_digest(&digest);
            if sha256 != header.sha256 {
                return Err(ArtifactError::HashMismatch {
                    expected: header.sha256.clone(),
                    actual: sha256,
                });
            }
            fs::remove_file(&staging_path).map_err(ArtifactError::Io)?;
            fs::rename(&verified_path, &final_path).map_err(ArtifactError::Rename)?;
            Ok(final_path.clone())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&verified_path);
            let _ = fs::remove_file(&staging_path);
        }
        result
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
    hex_digest(&digest)
}

fn hex_digest(digest: &[u8]) -> String {
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
    /// The worker's advertised SHA-256 did not match the staged bytes.
    HashMismatch {
        expected: String,
        actual: String,
    },
    ByteCountMismatch {
        expected: u64,
        actual: u64,
    },
    /// The staged payload exceeded `MAX_ARTIFACT_BYTES`.
    PayloadTooLarge {
        size: usize,
        max: usize,
    },
    /// The staging name was empty or contained a path separator.
    InvalidName(String),
    InvalidRoot(PathBuf),
    NotRegularFile(String),
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
            Self::ByteCountMismatch { expected, actual } => write!(
                formatter,
                "staged artifact byte count mismatch: worker advertised {expected}, host computed {actual}"
            ),
            Self::PayloadTooLarge { size, max } => write!(
                formatter,
                "staged artifact exceeds maximum size: {size} > {max}"
            ),
            Self::InvalidName(name) => {
                write!(formatter, "staged artifact name is invalid: {name:?}")
            }
            Self::InvalidRoot(path) => {
                write!(
                    formatter,
                    "staged artifact root is not private: {}",
                    path.display()
                )
            }
            Self::NotRegularFile(name) => {
                write!(formatter, "staged artifact is not a regular file: {name:?}")
            }
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
        let _ = fs::remove_file(&root);
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn header(staged: &StagedArtifact, sha256: String) -> ArtifactHeader {
        let worker_fingerprint = WorkerFingerprint {
            worker_kind: "occt".to_string(),
            worker_schema_version: "threeterm.workers.occt/1".to_string(),
            protocol_schema_version: crate::schema_version().to_string(),
        };
        ArtifactHeader {
            request_id: "request-1".to_string(),
            source_revision_id: "revision-1".to_string(),
            cache_key: Layer1CacheKey {
                source_revision_id: "revision-1".to_string(),
                worker_fingerprint: worker_fingerprint.clone(),
                artifact_kind: "brep".to_string(),
                semantic_input_sha256: "11".repeat(32),
                deterministic_settings_sha256: "22".repeat(32),
            },
            worker_fingerprint,
            artifact_kind: "brep".to_string(),
            staging_name: staged.staging_name.clone(),
            byte_count: staged.byte_count,
            sha256,
        }
    }

    #[test]
    fn validated_staged_bytes_promote_to_the_final_path() {
        let root = temp_root("promote");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";
        let staged = stage
            .stage_bytes("sketch-1.brep", bytes)
            .expect("artifact stages");
        let final_path = stage
            .validate_and_promote(&header(&staged, staged.sha256.clone()))
            .expect("artifact validates and promotes");

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
    fn validate_rejects_an_artifact_whose_advertised_hash_does_not_match() {
        let root = temp_root("mismatch");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";
        let staged = stage
            .stage_bytes("sketch-1.brep", bytes)
            .expect("artifact stages");

        let error = stage
            .validate_and_promote(&header(&staged, "deadbeef".to_string()))
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
    fn stage_bytes_rejects_a_payload_over_the_maximum() {
        let root = temp_root("oversize");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = vec![0u8; MAX_ARTIFACT_BYTES + 1];

        let error = stage
            .stage_bytes("oversize.brep", &bytes)
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
    fn stage_bytes_rejects_a_name_that_contains_a_path_separator() {
        let root = temp_root("path-separator");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";

        let error = stage
            .stage_bytes("nested/file.brep", bytes)
            .expect_err("separator-bearing name must be rejected");
        assert!(
            matches!(error, ArtifactError::InvalidName(_)),
            "expected InvalidName; got {error:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn open_rejects_a_symlinked_staging_root() {
        let target = temp_root("root-target");
        let link = temp_root("root-link");
        fs::create_dir_all(&target).expect("target creates");
        std::os::unix::fs::symlink(&target, &link).expect("root symlink creates");

        let error = Stage::open(&link).expect_err("symlinked root is rejected");

        assert!(matches!(error, ArtifactError::InvalidRoot(path) if path == link));
        let _ = fs::remove_file(link);
        let _ = fs::remove_dir_all(target);
    }

    #[test]
    fn promotion_rejects_a_symlinked_staged_file() {
        let root = temp_root("file-symlink");
        let target = temp_root("file-target");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"outside bytes";
        fs::write(&target, bytes).expect("target writes");
        std::os::unix::fs::symlink(&target, root.join("sketch-1.brep.partial"))
            .expect("artifact symlink creates");
        let staged = StagedArtifact {
            staging_name: "sketch-1.brep".to_string(),
            sha256: sha256_hex(bytes),
            byte_count: bytes.len() as u64,
        };

        let error = stage
            .validate_and_promote(&header(&staged, staged.sha256.clone()))
            .expect_err("symlinked artifact is rejected");

        assert!(matches!(error, ArtifactError::NotRegularFile(_)));
        assert_eq!(fs::read(&target).expect("target reads"), bytes);
        assert!(!root.join("sketch-1.brep.partial").exists());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(target);
    }

    #[test]
    fn promotion_rejects_an_oversized_worker_file_before_reading_it() {
        let root = temp_root("worker-oversize");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = vec![0u8; MAX_ARTIFACT_BYTES + 1];
        fs::write(root.join("sketch-1.brep.partial"), &bytes).expect("worker file writes");
        let staged = StagedArtifact {
            staging_name: "sketch-1.brep".to_string(),
            sha256: sha256_hex(&bytes),
            byte_count: bytes.len() as u64,
        };

        let error = stage
            .validate_and_promote(&header(&staged, staged.sha256.clone()))
            .expect_err("oversized worker file is rejected");

        assert!(matches!(error, ArtifactError::PayloadTooLarge { .. }));
        assert!(!root.join("sketch-1.brep.partial").exists());
        assert!(!root.join("sketch-1.brep").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discard_removes_the_staging_directory_and_every_partial() {
        let root = temp_root("discard");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"hello, worker";
        let _ = stage
            .stage_bytes("sketch-1.brep", bytes)
            .expect("artifact stages");

        stage.discard().expect("discard succeeds");
        assert!(
            !root.exists(),
            "staging directory must be gone after discard"
        );
    }
}

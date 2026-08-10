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

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{Mode, mkdirat};
use nix::unistd::{UnlinkatFlags, linkat, unlinkat};
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
    pub operation: String,
    pub feature_id: String,
    pub artifact_kind: String,
    pub staging_name: String,
    pub semantic_input_sha256: String,
    pub deterministic_settings_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Layer1CacheKey {
    pub source_revision_id: String,
    pub worker_fingerprint: WorkerFingerprint,
    pub operation: String,
    pub feature_id: String,
    pub artifact_kind: String,
    pub semantic_input_sha256: String,
    pub deterministic_settings_sha256: String,
}

impl Layer1CacheKey {
    pub fn issue(request: &Layer1ArtifactRequest, worker_fingerprint: &WorkerFingerprint) -> Self {
        Self {
            source_revision_id: request.source_revision_id.clone(),
            worker_fingerprint: worker_fingerprint.clone(),
            operation: request.operation.clone(),
            feature_id: request.feature_id.clone(),
            artifact_kind: request.artifact_kind.clone(),
            semantic_input_sha256: request.semantic_input_sha256.clone(),
            deterministic_settings_sha256: request.deterministic_settings_sha256.clone(),
        }
    }

    /// Returns the stable final filename for this cache identity.
    pub fn final_artifact_name(&self) -> String {
        let mut identity = Vec::new();
        for field in [
            &self.source_revision_id,
            &self.worker_fingerprint.worker_kind,
            &self.worker_fingerprint.worker_schema_version,
            &self.worker_fingerprint.protocol_schema_version,
            &self.operation,
            &self.feature_id,
            &self.artifact_kind,
            &self.semantic_input_sha256,
            &self.deterministic_settings_sha256,
        ] {
            let bytes = field.as_bytes();
            identity.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            identity.extend_from_slice(bytes);
        }
        format!("derived-{}", sha256_hex(&identity))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactHeader {
    pub request_id: String,
    pub source_revision_id: String,
    pub operation: String,
    pub feature_id: String,
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
    parent: fs::File,
    root_name: String,
    root_dir: fs::File,
    verified_files: RefCell<HashMap<String, VerifiedFile>>,
}

#[derive(Debug)]
struct VerifiedFile {
    file: fs::File,
    anchor_name: String,
}

impl Stage {
    /// Open a staging directory at `root`, creating it if it doesn't
    /// exist. The directory is the namespace under which the host
    /// accumulates `.partial` files until `promote` or `discard`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ArtifactError> {
        let root = root.into();
        let (parent_path, root_name) = root_parts(&root)?;
        let parent = open_directory_tree(&parent_path, true)?;
        let root_dir = match openat_directory(&parent, &root_name) {
            Ok(root_dir) => root_dir,
            Err(ArtifactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                mkdir_child(&parent, &root_name)?;
                open_root_child(&parent, &root_name, &root)?
            }
            Err(error) => return Err(map_root_open_error(error, &root)),
        };
        root_dir
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(ArtifactError::Io)?;
        Self::from_pinned_root(root, parent, root_name, root_dir)
    }

    /// Open an existing staging namespace without creating any missing
    /// component. This is used to validate a previously published cache
    /// result without binding a new path into the filesystem.
    pub fn open_existing(root: impl Into<PathBuf>) -> Result<Self, ArtifactError> {
        let root = root.into();
        let (parent_path, root_name) = root_parts(&root)?;
        let parent = open_directory_tree(&parent_path, false)?;
        let root_dir = open_root_child(&parent, &root_name, &root)?;
        Self::from_pinned_root(root, parent, root_name, root_dir)
    }

    /// Create a new private staging directory without reusing an existing
    /// path. The caller owns the returned directory until it is discarded or
    /// a validated result is deliberately retained there.
    pub fn create_fresh(parent: impl Into<PathBuf>, prefix: &str) -> Result<Self, ArtifactError> {
        validate_name(prefix)?;
        let parent = parent.into();
        let parent_dir = open_directory_tree(&parent, true)?;
        parent_dir
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(ArtifactError::Io)?;

        for attempt in 0..32 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            let candidate =
                parent.join(format!("{prefix}-{}-{nanos}-{attempt}", std::process::id()));
            let candidate_name = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| ArtifactError::InvalidRoot(parent.clone()))?
                .to_string();
            match mkdir_child(&parent_dir, &candidate_name) {
                Ok(()) => {
                    let root_dir = match open_root_child(&parent_dir, &candidate_name, &candidate) {
                        Ok(root_dir) => root_dir,
                        Err(error) => {
                            let _ = unlink_child(&parent_dir, &candidate_name);
                            return Err(error);
                        }
                    };
                    if let Err(error) = root_dir
                        .set_permissions(fs::Permissions::from_mode(0o700))
                        .map_err(ArtifactError::Io)
                    {
                        let _ = unlink_child(&parent_dir, &candidate_name);
                        return Err(error);
                    }
                    return Self::from_pinned_root(candidate, parent_dir, candidate_name, root_dir);
                }
                Err(ArtifactError::Io(error))
                    if error.kind() == std::io::ErrorKind::AlreadyExists =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        Err(ArtifactError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a fresh staging directory after 32 attempts",
        )))
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
        let mut file = open_child(
            &self.root_dir,
            &format!("{staging_name}.partial"),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC | OFlag::O_NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        )?;
        file.write_all(bytes).map_err(ArtifactError::Io)?;
        file.sync_all().map_err(ArtifactError::Io)?;
        Ok(StagedArtifact {
            staging_name: staging_name.to_string(),
            sha256,
            byte_count: bytes.len() as u64,
        })
    }

    pub fn validate_and_promote(&self, header: &ArtifactHeader) -> Result<PathBuf, ArtifactError> {
        self.verify(header)?;
        self.publish_verified(&header.staging_name, &header.staging_name)
    }

    /// Copy a staged artifact into a private verified file after checking its
    /// advertised byte count and digest. Verification never publishes bytes.
    pub fn verify(&self, header: &ArtifactHeader) -> Result<(), ArtifactError> {
        validate_name(&header.staging_name)?;
        let staging_name = format!("{}.partial", header.staging_name);
        let verified_name = format!(".{}.verified", header.staging_name);
        let result = (|| -> Result<VerifiedFile, ArtifactError> {
            let mut source = match open_child(
                &self.root_dir,
                &staging_name,
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
                Mode::empty(),
            ) {
                Err(ArtifactError::Io(error))
                    if error.raw_os_error() == Some(nix::errno::Errno::ELOOP as i32) =>
                {
                    return Err(ArtifactError::NotRegularFile(header.staging_name.clone()));
                }
                result => result?,
            };
            let opened_metadata = source.metadata().map_err(ArtifactError::Io)?;
            if !opened_metadata.is_file() {
                return Err(ArtifactError::NotRegularFile(header.staging_name.clone()));
            }
            if opened_metadata.len() > MAX_ARTIFACT_BYTES as u64 {
                return Err(ArtifactError::PayloadTooLarge {
                    size: usize::try_from(opened_metadata.len()).unwrap_or(usize::MAX),
                    max: MAX_ARTIFACT_BYTES,
                });
            }
            let mut verified = open_child(
                &self.root_dir,
                &verified_name,
                OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW,
                Mode::from_bits_truncate(0o600),
            )?;
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
            let verified_handle = open_child(
                &self.root_dir,
                &verified_name,
                OFlag::O_PATH | OFlag::O_NOFOLLOW,
                Mode::empty(),
            )?;
            let verified_metadata = verified.metadata().map_err(ArtifactError::Io)?;
            let handle_metadata = verified_handle.metadata().map_err(ArtifactError::Io)?;
            if !same_file(&verified_metadata, &handle_metadata) {
                return Err(ArtifactError::StagedFileChanged(verified_name.clone()));
            }
            let anchor_name = format!("{verified_name}-anchor");
            linkat(
                &verified_handle,
                "",
                &self.root_dir,
                anchor_name.as_str(),
                AtFlags::AT_EMPTY_PATH,
            )
            .map_err(|error| ArtifactError::Io(std::io::Error::from_raw_os_error(error as i32)))?;
            unlink_child(&self.root_dir, &staging_name)?;
            Ok(VerifiedFile {
                file: verified_handle,
                anchor_name,
            })
        })();
        match result {
            Ok(verified) => {
                self.verified_files
                    .borrow_mut()
                    .insert(header.staging_name.clone(), verified);
                Ok(())
            }
            Err(error) => {
                let _ = unlink_child(&self.root_dir, &verified_name);
                let _ = unlink_child(&self.root_dir, &format!("{verified_name}-anchor"));
                let _ = unlink_child(&self.root_dir, &staging_name);
                Err(error)
            }
        }
    }

    /// Publish a verified artifact under a cache-derived final filename.
    /// `hard_link` atomically refuses to replace an existing final artifact.
    pub fn publish_verified(
        &self,
        staging_name: &str,
        final_name: &str,
    ) -> Result<PathBuf, ArtifactError> {
        validate_name(staging_name)?;
        validate_name(final_name)?;
        let final_path = self.root.join(final_name);
        let verified_name = format!(".{staging_name}.verified");
        let verified = self
            .verified_files
            .borrow_mut()
            .remove(staging_name)
            .ok_or_else(|| {
                ArtifactError::Io(std::io::Error::other(
                    "verified artifact handle is no longer available",
                ))
            })?;
        let result = linkat(
            &verified.file,
            "",
            &self.root_dir,
            final_name,
            AtFlags::AT_EMPTY_PATH,
        )
        .map_err(|error| ArtifactError::Io(std::io::Error::from_raw_os_error(error as i32)))
        .map_err(|error| match error {
            ArtifactError::Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                ArtifactError::FinalPathExists(final_path.clone())
            }
            ArtifactError::Io(error) => ArtifactError::Rename(error),
            error => error,
        });
        let _ = unlink_child(&self.root_dir, &verified_name);
        let _ = unlink_child(&self.root_dir, &verified.anchor_name);
        result.map(|()| final_path)
    }

    /// Check that a published artifact is still the regular file recorded by
    /// its result metadata.
    pub fn published_matches(
        &self,
        final_name: &str,
        byte_count: u64,
        sha256: &str,
    ) -> Result<bool, ArtifactError> {
        validate_name(final_name)?;
        let mut file = match open_child(
            &self.root_dir,
            final_name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
            Mode::empty(),
        ) {
            Ok(file) => file,
            Err(ArtifactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let opened_metadata = file.metadata().map_err(ArtifactError::Io)?;
        if !opened_metadata.is_file()
            || opened_metadata.len() > MAX_ARTIFACT_BYTES as u64
            || opened_metadata.len() != byte_count
        {
            return Ok(false);
        }
        let mut digest = Sha256::new();
        let mut actual_bytes = 0u64;
        let mut buffer = [0u8; 8192];
        loop {
            let read = file.read(&mut buffer).map_err(ArtifactError::Io)?;
            if read == 0 {
                break;
            }
            actual_bytes = actual_bytes.saturating_add(read as u64);
            if actual_bytes > MAX_ARTIFACT_BYTES as u64 {
                return Ok(false);
            }
            digest.update(&buffer[..read]);
        }
        Ok(actual_bytes == byte_count && hex_digest(&digest.finalize()) == sha256)
    }

    /// Remove an invalid cache-owned final artifact before recovering it from
    /// a newly verified staging file.
    pub fn discard_final(&self, final_name: &str) -> Result<(), ArtifactError> {
        validate_name(final_name)?;
        match unlink_child(&self.root_dir, final_name) {
            Ok(()) => Ok(()),
            Err(ArtifactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Remove a verified artifact after the Host accepts an existing cache hit.
    pub fn discard_verified(&self, staging_name: &str) {
        if validate_name(staging_name).is_ok() {
            if let Some(verified) = self.verified_files.borrow_mut().remove(staging_name) {
                let _ = unlink_child(&self.root_dir, &verified.anchor_name);
            }
            let _ = unlink_child(&self.root_dir, &format!(".{staging_name}.verified"));
        }
    }

    /// Remove only the request-owned staged entries, preserving other files
    /// already published in this shared cache directory.
    pub fn discard_staged(&self, staging_name: &str) {
        if validate_name(staging_name).is_ok() {
            let _ = unlink_child(&self.root_dir, &format!("{staging_name}.partial"));
            self.discard_verified(staging_name);
        }
    }

    /// Remove the staging directory and every `.partial` file it
    /// contains. Called by the supervisor on force-terminate so the
    /// host never holds an authoritative-looking staged entry. The
    /// returned `Stage` is consumed; create a fresh one if needed.
    pub fn discard(self) -> Result<(), ArtifactError> {
        self.verified_files.borrow_mut().clear();
        let expected = self.root_dir.metadata().map_err(ArtifactError::Io)?;
        let current = openat_directory(&self.parent, &self.root_name)?;
        if !same_file(&expected, &current.metadata().map_err(ArtifactError::Io)?) {
            return Err(ArtifactError::StagedFileChanged(
                self.root.display().to_string(),
            ));
        }
        let stable_root = PathBuf::from(format!("/proc/self/fd/{}", self.root_dir.as_raw_fd()));
        for entry in fs::read_dir(stable_root).map_err(ArtifactError::Io)? {
            let entry = entry.map_err(ArtifactError::Io)?;
            if entry.file_type().map_err(ArtifactError::Io)?.is_dir() {
                fs::remove_dir_all(entry.path()).map_err(ArtifactError::Io)?;
            } else {
                unlink_child(&self.root_dir, &entry.file_name().to_string_lossy())?;
            }
        }
        match unlinkat(
            &self.parent,
            self.root_name.as_str(),
            UnlinkatFlags::RemoveDir,
        ) {
            Ok(()) => Ok(()),
            Err(nix::errno::Errno::ENOENT) => Ok(()),
            Err(error) => Err(ArtifactError::Io(std::io::Error::from_raw_os_error(
                error as i32,
            ))),
        }
    }
}

impl Stage {
    fn from_pinned_root(
        root: PathBuf,
        parent: fs::File,
        root_name: String,
        root_dir: fs::File,
    ) -> Result<Self, ArtifactError> {
        Ok(Self {
            root,
            parent,
            root_name,
            root_dir,
            verified_files: RefCell::new(HashMap::new()),
        })
    }
}

fn open_directory(path: &Path) -> Result<fs::File, ArtifactError> {
    OpenOptions::new()
        .read(true)
        .custom_flags(0o200000 | 0o400000)
        .open(path)
        .map_err(ArtifactError::Io)
}

fn root_parts(root: &Path) -> Result<(PathBuf, String), ArtifactError> {
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ArtifactError::InvalidRoot(root.to_path_buf()))?
        .to_string();
    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    Ok((parent, root_name))
}

fn open_directory_tree(path: &Path, create_missing: bool) -> Result<fs::File, ArtifactError> {
    let mut current = if path.is_absolute() {
        open_directory(Path::new("/"))?
    } else {
        open_directory(Path::new("."))?
    };
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(ArtifactError::InvalidRoot(path.to_path_buf()));
        }
        let Component::Normal(name) = component else {
            continue;
        };
        let Some(name) = name.to_str() else {
            return Err(ArtifactError::InvalidRoot(path.to_path_buf()));
        };
        current = match openat_directory(&current, name) {
            Ok(directory) => Ok(directory),
            Err(ArtifactError::Io(error))
                if create_missing && error.kind() == std::io::ErrorKind::NotFound =>
            {
                match mkdir_child(&current, name) {
                    Ok(()) => openat_directory(&current, name),
                    Err(ArtifactError::Io(error))
                        if error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        openat_directory(&current, name)
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(map_root_open_error(error, path)),
        }
        .map_err(|error| map_root_open_error(error, path))?;
    }
    Ok(current)
}

fn open_root_child(parent: &fs::File, name: &str, root: &Path) -> Result<fs::File, ArtifactError> {
    openat_directory(parent, name).map_err(|error| map_root_open_error(error, root))
}

fn map_root_open_error(error: ArtifactError, root: &Path) -> ArtifactError {
    let invalid = matches!(
        &error,
        ArtifactError::Io(error)
            if matches!(
                error.raw_os_error(),
                Some(code)
                    if code == nix::errno::Errno::ELOOP as i32
                        || code == nix::errno::Errno::ENOTDIR as i32
            )
    );
    if invalid {
        ArtifactError::InvalidRoot(root.to_path_buf())
    } else {
        error
    }
}

fn mkdir_child(directory: &fs::File, name: &str) -> Result<(), ArtifactError> {
    let name = CString::new(name).map_err(|_| ArtifactError::InvalidName(name.to_string()))?;
    mkdirat(directory, name.as_c_str(), Mode::from_bits_truncate(0o700))
        .map_err(|error| ArtifactError::Io(std::io::Error::from_raw_os_error(error as i32)))
}

fn openat_directory(parent: &fs::File, name: &str) -> Result<fs::File, ArtifactError> {
    open_child(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
}

fn open_child(
    directory: &fs::File,
    name: &str,
    flags: OFlag,
    mode: Mode,
) -> Result<fs::File, ArtifactError> {
    openat(directory, name, flags, mode)
        .map(fs::File::from)
        .map_err(|error| ArtifactError::Io(std::io::Error::from_raw_os_error(error as i32)))
}

fn unlink_child(directory: &fs::File, name: &str) -> Result<(), ArtifactError> {
    unlinkat(directory, name, UnlinkatFlags::NoRemoveDir)
        .map_err(|error| ArtifactError::Io(std::io::Error::from_raw_os_error(error as i32)))
}

/// SHA-256 hex digest of `bytes`, lowercase, 64 characters.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_digest(&digest)
}

fn validate_name(name: &str) -> Result<(), ArtifactError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(ArtifactError::InvalidName(name.to_string()));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
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
    StagedFileChanged(String),
    FinalPathExists(PathBuf),
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
            Self::StagedFileChanged(name) => {
                write!(
                    formatter,
                    "staged artifact changed while being verified: {name:?}"
                )
            }
            Self::FinalPathExists(path) => {
                write!(
                    formatter,
                    "final artifact already exists: {}",
                    path.display()
                )
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
            operation: "extrude".to_string(),
            feature_id: "sketch-1".to_string(),
            cache_key: Layer1CacheKey {
                source_revision_id: "revision-1".to_string(),
                worker_fingerprint: worker_fingerprint.clone(),
                operation: "extrude".to_string(),
                feature_id: "sketch-1".to_string(),
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
    fn cache_key_derives_a_stable_filesystem_safe_final_name() {
        let worker_fingerprint = WorkerFingerprint {
            worker_kind: "occt".to_string(),
            worker_schema_version: "threeterm.workers.occt/1".to_string(),
            protocol_schema_version: crate::schema_version().to_string(),
        };
        let cache_key = Layer1CacheKey {
            source_revision_id: "revision-1".to_string(),
            worker_fingerprint: worker_fingerprint.clone(),
            operation: "extrude".to_string(),
            feature_id: "sketch-1".to_string(),
            artifact_kind: "brep".to_string(),
            semantic_input_sha256: "11".repeat(32),
            deterministic_settings_sha256: "22".repeat(32),
        };

        let name = cache_key.final_artifact_name();

        assert_eq!(
            name,
            "derived-cf86824e370dcff05e492e7eb0e02c8b4aa6ee06310a3a21a0174a9c7a6f7942"
        );
        assert_eq!(name, cache_key.final_artifact_name());
        assert!(name.starts_with("derived-"));
        assert_eq!(name.len(), "derived-".len() + 64);
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        );

        for changed_key in [
            Layer1CacheKey {
                source_revision_id: "revision-2".to_string(),
                ..cache_key.clone()
            },
            Layer1CacheKey {
                worker_fingerprint: WorkerFingerprint {
                    worker_kind: "slvs".to_string(),
                    ..worker_fingerprint.clone()
                },
                ..cache_key.clone()
            },
            Layer1CacheKey {
                worker_fingerprint: WorkerFingerprint {
                    worker_schema_version: "threeterm.workers.occt/2".to_string(),
                    ..worker_fingerprint.clone()
                },
                ..cache_key.clone()
            },
            Layer1CacheKey {
                worker_fingerprint: WorkerFingerprint {
                    protocol_schema_version: "threeterm.protocol/2".to_string(),
                    ..worker_fingerprint
                },
                ..cache_key.clone()
            },
            Layer1CacheKey {
                operation: "boolean_fuse".to_string(),
                ..cache_key.clone()
            },
            Layer1CacheKey {
                feature_id: "sketch-2".to_string(),
                ..cache_key.clone()
            },
            Layer1CacheKey {
                artifact_kind: "mesh".to_string(),
                ..cache_key.clone()
            },
            Layer1CacheKey {
                semantic_input_sha256: "33".repeat(32),
                ..cache_key.clone()
            },
            Layer1CacheKey {
                deterministic_settings_sha256: "44".repeat(32),
                ..cache_key
            },
        ] {
            assert_ne!(name, changed_key.final_artifact_name());
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
    fn verified_staged_bytes_publish_at_a_separate_final_name_without_replacing() {
        let root = temp_root("separate-final-name");
        let stage = Stage::open(&root).expect("stage opens");
        let staged = stage
            .stage_bytes("requested-name.brep", b"first bytes")
            .expect("first artifact stages");
        let first_header = header(&staged, staged.sha256.clone());
        let final_name = first_header.cache_key.final_artifact_name();

        stage
            .verify(&first_header)
            .expect("first artifact verifies");
        let path = stage
            .publish_verified(&first_header.staging_name, &final_name)
            .expect("first artifact publishes");
        assert_eq!(path, root.join(&final_name));
        assert_eq!(
            fs::read(&path).expect("first artifact reads"),
            b"first bytes"
        );

        let staged = stage
            .stage_bytes("requested-name.brep", b"replacement bytes")
            .expect("second artifact stages");
        let second_header = header(&staged, staged.sha256.clone());
        stage
            .verify(&second_header)
            .expect("second artifact verifies");
        let error = stage
            .publish_verified(&second_header.staging_name, &final_name)
            .expect_err("existing final artifact is not replaced");

        assert!(
            matches!(error, ArtifactError::FinalPathExists(path) if path == root.join(&final_name))
        );
        assert_eq!(
            fs::read(&path).expect("first artifact re-reads"),
            b"first bytes"
        );
        assert!(!root.join("requested-name.brep.partial").exists());
        assert!(!root.join(".requested-name.brep.verified").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publication_uses_the_verified_inode_after_its_path_is_replaced() {
        let root = temp_root("verified-path-replacement");
        let replacement = temp_root("verified-path-replacement-file");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"verified inode bytes";
        let staged = stage
            .stage_bytes("requested-name.brep", bytes)
            .expect("artifact stages");
        let artifact_header = header(&staged, staged.sha256.clone());
        stage.verify(&artifact_header).expect("artifact verifies");

        let verified_path = root.join(".requested-name.brep.verified");
        fs::write(&replacement, b"replacement path bytes").expect("replacement writes");
        fs::remove_file(&verified_path).expect("verified path removes");
        fs::rename(&replacement, &verified_path).expect("replacement path installs");

        let final_path = stage
            .publish_verified(&artifact_header.staging_name, "derived-final")
            .expect("retained verified inode publishes");

        assert_eq!(fs::read(final_path).expect("published bytes read"), bytes);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(replacement);
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
    fn create_fresh_makes_unique_private_directories() {
        let parent = temp_root("fresh-parent");
        let first = Stage::create_fresh(&parent, "extrude").expect("first stage creates");
        let second = Stage::create_fresh(&parent, "extrude").expect("second stage creates");

        assert_ne!(first.root(), second.root());
        assert!(first.root().is_dir());
        assert!(second.root().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(first.root())
                    .expect("first metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(second.root())
                    .expect("second metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }

        first.discard().expect("first stage discards");
        second.discard().expect("second stage discards");
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn create_fresh_rejects_a_symlinked_parent() {
        let target = temp_root("fresh-parent-target");
        let link = temp_root("fresh-parent-link");
        fs::create_dir_all(&target).expect("target creates");
        std::os::unix::fs::symlink(&target, &link).expect("parent symlink creates");

        let error = Stage::create_fresh(&link, "extrude").expect_err("symlinked parent rejects");

        assert!(matches!(error, ArtifactError::InvalidRoot(path) if path == link));
        let _ = fs::remove_file(link);
        let _ = fs::remove_dir_all(target);
    }

    #[test]
    fn create_fresh_rejects_a_symlinked_ancestor_before_creating_outside() {
        let base = temp_root("fresh-ancestor-base");
        let target = temp_root("fresh-ancestor-target");
        let redirect = base.join("redirect");
        fs::create_dir_all(&base).expect("base creates");
        fs::create_dir_all(&target).expect("target creates");
        std::os::unix::fs::symlink(&target, &redirect).expect("ancestor symlink creates");
        let parent = redirect.join("stages");

        let error = Stage::create_fresh(&parent, "extrude")
            .expect_err("symlinked ancestor must reject before creating a stage");

        assert!(matches!(error, ArtifactError::InvalidRoot(path) if path == parent));
        assert!(!target.join("stages").exists());
        let _ = fs::remove_dir_all(base);
        let _ = fs::remove_dir_all(target);
    }

    #[test]
    fn stable_stage_handle_does_not_follow_a_replaced_ancestor() {
        let parent = temp_root("ancestor-parent");
        let outside = temp_root("ancestor-outside");
        let moved = temp_root("ancestor-moved");
        fs::create_dir_all(&outside).expect("outside creates");
        let stage = Stage::create_fresh(&parent, "extrude").expect("stage creates");
        let staged = stage
            .stage_bytes("sketch-1.brep", b"private bytes")
            .expect("artifact stages");
        let header = header(&staged, staged.sha256.clone());
        let stage_name = stage.root().file_name().unwrap().to_owned();

        fs::rename(&parent, &moved).expect("parent moves");
        std::os::unix::fs::symlink(&outside, &parent).expect("ancestor symlink creates");

        stage.verify(&header).expect("stable handle verifies");
        assert!(!outside.join(&stage_name).exists());

        stage.discard().expect("stable stage discards");
        assert!(!moved.join(&stage_name).exists());
        let _ = fs::remove_file(parent);
        let _ = fs::remove_dir_all(outside);
        let _ = fs::remove_dir_all(moved);
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
    fn promotion_rejects_a_fifo_without_waiting_for_a_writer() {
        let root = temp_root("file-fifo");
        let stage = Stage::open(&root).expect("stage opens");
        let fifo = root.join("sketch-1.brep.partial");
        nix::unistd::mkfifo(&fifo, Mode::from_bits_truncate(0o600)).expect("artifact fifo creates");
        let staged = StagedArtifact {
            staging_name: "sketch-1.brep".to_string(),
            sha256: sha256_hex(b"fifo bytes"),
            byte_count: b"fifo bytes".len() as u64,
        };

        let error = stage
            .validate_and_promote(&header(&staged, staged.sha256.clone()))
            .expect_err("fifo artifact is rejected");

        assert!(matches!(error, ArtifactError::NotRegularFile(_)));
        assert!(!fifo.exists(), "rejected fifo must be removed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verification_does_not_truncate_a_hard_linked_host_path() {
        let root = temp_root("verified-hard-link");
        let protected = temp_root("verified-hard-link-protected");
        let stage = Stage::open(&root).expect("stage opens");
        let bytes = b"protected canonical bytes";
        fs::write(&protected, bytes).expect("protected file writes");
        let staged = stage
            .stage_bytes("sketch-1.brep", b"worker bytes")
            .expect("artifact stages");
        fs::hard_link(&protected, root.join(".sketch-1.brep.verified"))
            .expect("verified path hard link creates");

        let error = stage
            .verify(&header(&staged, staged.sha256.clone()))
            .expect_err("pre-existing verified path rejects");

        assert!(matches!(error, ArtifactError::Io(_)));
        assert_eq!(fs::read(&protected).expect("protected file reads"), bytes);
        assert!(!root.join("sketch-1.brep.partial").exists());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(protected);
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

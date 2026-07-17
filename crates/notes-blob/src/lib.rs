//! Synchronous, content-addressed blob storage shared by notes-rs hosts.
//!
//! A caller must know the expected SHA-256 before installation. That lets the
//! store create its temporary file in the final shard and publish the verified
//! bytes atomically without ever overwriting an existing blob.

use std::{
    fmt,
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use tempfile::Builder;
use thiserror::Error;

const SHA256_BYTES: usize = 32;
const SHA256_HEX_LEN: usize = SHA256_BYTES * 2;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const TEMP_PREFIX: &str = ".incoming-";

/// A canonical, lowercase SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobHash([u8; SHA256_BYTES]);

impl BlobHash {
    /// Constructs a hash from its binary representation.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; SHA256_BYTES]) -> Self {
        Self(bytes)
    }

    /// Returns the binary digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; SHA256_BYTES] {
        &self.0
    }

    /// Hashes an in-memory byte slice.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        let digest: [u8; SHA256_BYTES] = Sha256::digest(bytes).into();
        Self(digest)
    }

    fn write_hex(self, output: &mut [u8; SHA256_HEX_LEN]) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (index, byte) in self.0.into_iter().enumerate() {
            output[index * 2] = HEX[usize::from(byte >> 4)];
            output[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
        }
    }
}

impl AsRef<[u8]> for BlobHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut encoded = [0_u8; SHA256_HEX_LEN];
        self.write_hex(&mut encoded);
        let encoded = std::str::from_utf8(&encoded).expect("hex encoding is always valid ASCII");
        formatter.write_str(encoded)
    }
}

impl fmt::Debug for BlobHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl FromStr for BlobHash {
    type Err = ParseBlobHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != SHA256_HEX_LEN {
            return Err(ParseBlobHashError::WrongLength {
                actual: value.len(),
            });
        }
        if value.bytes().any(|byte| matches!(byte, b'A'..=b'F')) {
            return Err(ParseBlobHashError::Uppercase);
        }

        let mut decoded = [0_u8; SHA256_BYTES];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high =
                decode_nibble(pair[0]).ok_or(ParseBlobHashError::NonHex { index: index * 2 })?;
            let low = decode_nibble(pair[1]).ok_or(ParseBlobHashError::NonHex {
                index: index * 2 + 1,
            })?;
            decoded[index] = (high << 4) | low;
        }
        Ok(Self(decoded))
    }
}

impl Serialize for BlobHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for BlobHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BlobHashVisitor;

        impl de::Visitor<'_> for BlobHashVisitor {
            type Value = BlobHash;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a canonical lowercase 64-character SHA-256 digest")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(BlobHashVisitor)
    }
}

fn decode_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Why a textual blob hash was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ParseBlobHashError {
    /// SHA-256 must have exactly 64 hexadecimal characters.
    #[error("blob hash must contain exactly 64 characters, got {actual}")]
    WrongLength { actual: usize },
    /// Uppercase hexadecimal is deliberately non-canonical.
    #[error("blob hash must use lowercase hexadecimal")]
    Uppercase,
    /// The string contained a non-hexadecimal byte.
    #[error("blob hash contains a non-hexadecimal character at byte {index}")]
    NonHex { index: usize },
}

/// Whether an installation published new bytes or reused verified bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// This call atomically published a new blob.
    Installed,
    /// The same verified blob was already installed.
    AlreadyPresent,
}

/// Metadata returned after installing or verifying a blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobInfo {
    /// Canonical content digest.
    pub hash: BlobHash,
    /// Number of stored bytes.
    pub size: u64,
    /// Canonical filesystem location.
    pub path: PathBuf,
}

/// Result of a successful installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    /// Installed blob metadata.
    pub blob: BlobInfo,
    /// Whether this call published the filesystem entry.
    pub outcome: InstallOutcome,
}

/// A synchronous content-addressed filesystem store.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Creates a store rooted at `root` without touching the filesystem.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the configured store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the canonical path `blobs/<first-two-hex>/<full-hash>`.
    #[must_use]
    pub fn path_for(&self, hash: BlobHash) -> PathBuf {
        let canonical = hash.to_string();
        self.root
            .join("blobs")
            .join(&canonical[..2])
            .join(canonical)
    }

    /// Streams a file into the store while enforcing `max_bytes`.
    pub fn install_file(
        &self,
        source: impl AsRef<Path>,
        expected_hash: BlobHash,
        max_bytes: u64,
    ) -> Result<InstallResult, BlobStoreError> {
        let source = source.as_ref();
        let file = File::open(source).map_err(|source_error| BlobStoreError::Io {
            operation: "open source file",
            path: source.to_path_buf(),
            source: source_error,
        })?;
        self.install_reader(file, expected_hash, max_bytes)
    }

    /// Streams a reader into a same-shard temporary file, verifies the digest,
    /// and atomically publishes it without replacing an existing target.
    pub fn install_reader(
        &self,
        mut reader: impl Read,
        expected_hash: BlobHash,
        max_bytes: u64,
    ) -> Result<InstallResult, BlobStoreError> {
        let target = self.path_for(expected_hash);
        let shard = target
            .parent()
            .expect("a canonical blob path always has a shard parent");
        self.ensure_shard(shard)?;

        let mut temporary = Builder::new()
            .prefix(TEMP_PREFIX)
            .tempfile_in(shard)
            .map_err(|source| BlobStoreError::Io {
                operation: "create temporary blob",
                path: shard.to_path_buf(),
                source,
            })?;

        let temporary_path = temporary.path().to_path_buf();
        let (actual_hash, size) = copy_and_hash(
            &mut reader,
            temporary.as_file_mut(),
            max_bytes,
            &temporary_path,
        )?;
        if actual_hash != expected_hash {
            return Err(BlobStoreError::HashMismatch {
                expected: expected_hash,
                actual: actual_hash,
            });
        }

        temporary
            .as_file_mut()
            .flush()
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| BlobStoreError::Io {
                operation: "sync temporary blob",
                path: temporary.path().to_path_buf(),
                source,
            })?;

        if target.try_exists().map_err(|source| BlobStoreError::Io {
            operation: "inspect blob target",
            path: target.clone(),
            source,
        })? {
            self.verify_existing(&target, expected_hash, size)?;
            return Ok(InstallResult {
                blob: BlobInfo {
                    hash: expected_hash,
                    size,
                    path: target,
                },
                outcome: InstallOutcome::AlreadyPresent,
            });
        }

        match fs::hard_link(temporary.path(), &target) {
            Ok(()) => {
                sync_directory(shard)?;
                Ok(InstallResult {
                    blob: BlobInfo {
                        hash: expected_hash,
                        size,
                        path: target,
                    },
                    outcome: InstallOutcome::Installed,
                })
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                self.verify_existing(&target, expected_hash, size)?;
                Ok(InstallResult {
                    blob: BlobInfo {
                        hash: expected_hash,
                        size,
                        path: target,
                    },
                    outcome: InstallOutcome::AlreadyPresent,
                })
            }
            Err(source) => Err(BlobStoreError::Io {
                operation: "publish blob",
                path: target,
                source,
            }),
        }
    }

    /// Opens an installed blob after rejecting symlinks and non-files.
    pub fn open(&self, hash: BlobHash) -> Result<File, BlobStoreError> {
        let path = self.path_for(hash);
        ensure_regular_file(&path)?;
        File::open(&path).map_err(|source| BlobStoreError::Io {
            operation: "open blob",
            path,
            source,
        })
    }

    /// Reads and hashes an installed blob to verify its content-addressed name.
    pub fn verify(&self, hash: BlobHash) -> Result<BlobInfo, BlobStoreError> {
        let path = self.path_for(hash);
        let file = self.open(hash)?;
        let (actual_hash, size) = hash_reader(BufReader::new(file), &path)?;
        if actual_hash != hash {
            return Err(BlobStoreError::CorruptBlob {
                path,
                expected: hash,
                actual: actual_hash,
            });
        }
        Ok(BlobInfo { hash, size, path })
    }

    fn ensure_shard(&self, shard: &Path) -> Result<(), BlobStoreError> {
        ensure_directory(&self.root)?;
        let blobs = self.root.join("blobs");
        ensure_directory(&blobs)?;
        ensure_directory(shard)
    }

    fn verify_existing(
        &self,
        path: &Path,
        expected_hash: BlobHash,
        expected_size: u64,
    ) -> Result<(), BlobStoreError> {
        let metadata = ensure_regular_file(path)?;
        if metadata.len() != expected_size {
            return Err(BlobStoreError::ExistingSizeMismatch {
                path: path.to_path_buf(),
                expected: expected_size,
                actual: metadata.len(),
            });
        }
        let file = File::open(path).map_err(|source| BlobStoreError::Io {
            operation: "open existing blob",
            path: path.to_path_buf(),
            source,
        })?;
        let (actual_hash, actual_size) = hash_reader(BufReader::new(file), path)?;
        if actual_size != expected_size {
            return Err(BlobStoreError::ExistingSizeMismatch {
                path: path.to_path_buf(),
                expected: expected_size,
                actual: actual_size,
            });
        }
        if actual_hash != expected_hash {
            return Err(BlobStoreError::CorruptBlob {
                path: path.to_path_buf(),
                expected: expected_hash,
                actual: actual_hash,
            });
        }
        Ok(())
    }
}

fn ensure_directory(path: &Path) -> Result<(), BlobStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(BlobStoreError::UnsafeFilesystemEntry {
                path: path.to_path_buf(),
                expected: "a non-symlink directory",
            })
        }
        Ok(_) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(path).map_err(|source| BlobStoreError::Io {
                    operation: "inspect concurrently created directory",
                    path: path.to_path_buf(),
                    source,
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    Err(BlobStoreError::UnsafeFilesystemEntry {
                        path: path.to_path_buf(),
                        expected: "a non-symlink directory",
                    })
                } else {
                    Ok(())
                }
            }
            Err(source) => Err(BlobStoreError::Io {
                operation: "create blob directory",
                path: path.to_path_buf(),
                source,
            }),
        },
        Err(source) => Err(BlobStoreError::Io {
            operation: "inspect blob directory",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_regular_file(path: &Path) -> Result<fs::Metadata, BlobStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            BlobStoreError::NotFound {
                path: path.to_path_buf(),
            }
        } else {
            BlobStoreError::Io {
                operation: "inspect blob",
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BlobStoreError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            expected: "a regular non-symlink file",
        });
    }
    Ok(metadata)
}

fn copy_and_hash(
    reader: &mut impl Read,
    writer: &mut impl Write,
    max_bytes: u64,
    temporary_path: &Path,
) -> Result<(BlobHash, u64), BlobStoreError> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|source| BlobStoreError::Io {
                operation: "read blob source",
                path: temporary_path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        let next_size = size
            .checked_add(count as u64)
            .ok_or(BlobStoreError::TooLarge { limit: max_bytes })?;
        if next_size > max_bytes {
            return Err(BlobStoreError::TooLarge { limit: max_bytes });
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|source| BlobStoreError::Io {
                operation: "write temporary blob",
                path: temporary_path.to_path_buf(),
                source,
            })?;
        hasher.update(&buffer[..count]);
        size = next_size;
    }
    Ok((BlobHash::from_bytes(hasher.finalize().into()), size))
}

fn hash_reader(mut reader: impl Read, path: &Path) -> Result<(BlobHash, u64), BlobStoreError> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|source| BlobStoreError::Io {
                operation: "read stored blob",
                path: path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or(BlobStoreError::StoredBlobTooLarge {
                path: path.to_path_buf(),
            })?;
        hasher.update(&buffer[..count]);
    }
    Ok((BlobHash::from_bytes(hasher.finalize().into()), size))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), BlobStoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| BlobStoreError::Io {
            operation: "sync blob directory",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), BlobStoreError> {
    Ok(())
}

/// Filesystem and validation failures from [`BlobStore`].
#[derive(Debug, Error)]
pub enum BlobStoreError {
    /// A filesystem or stream operation failed.
    #[error("failed to {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The source exceeded the caller's explicit limit.
    #[error("blob exceeds the {limit}-byte limit")]
    TooLarge { limit: u64 },
    /// The streamed source did not match its declared content hash.
    #[error("blob hash mismatch: expected {expected}, got {actual}")]
    HashMismatch {
        expected: BlobHash,
        actual: BlobHash,
    },
    /// An existing blob has the wrong byte length.
    #[error("existing blob at {path} has size {actual}, expected {expected}")]
    ExistingSizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    /// An existing blob's bytes do not match its content-addressed name.
    #[error("corrupt blob at {path}: expected {expected}, got {actual}")]
    CorruptBlob {
        path: PathBuf,
        expected: BlobHash,
        actual: BlobHash,
    },
    /// A path component could redirect access or is not the expected type.
    #[error("unsafe filesystem entry at {path}: expected {expected}")]
    UnsafeFilesystemEntry {
        path: PathBuf,
        expected: &'static str,
    },
    /// The addressed blob is absent.
    #[error("blob not found at {path}")]
    NotFound { path: PathBuf },
    /// A stored file was too large to represent its length.
    #[error("stored blob at {path} is too large")]
    StoredBlobTooLarge { path: PathBuf },
}

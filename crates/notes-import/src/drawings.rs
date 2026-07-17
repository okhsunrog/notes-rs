use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

use crate::Sha256Digest;

pub const DRAWING_CONVERSION_SCHEMA_VERSION: u32 = 1;
pub const DRAWING_CONVERSION_BUNDLE_FILENAME: &str = "conversion-bundle.json";
const DEFAULT_MAX_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;

/// Deterministic, versioned hand-off from the sandboxed Excalidraw renderer.
///
/// This format deliberately contains neither absolute paths nor note content.
/// The complete source manifest digest binds it to one prepared graph, while
/// every entry independently binds an output to the exact source drawing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawingConversionBundle {
    pub schema_version: u32,
    pub converter: DrawingConverterIdentity,
    pub source_root: DrawingConversionSourceRoot,
    pub drawings: Vec<DrawingConversionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawingConverterIdentity {
    pub name: String,
    pub version: String,
    pub excalidraw_version: String,
    pub playwright_version: String,
    pub browser_name: DrawingConversionBrowserName,
    pub browser_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingConversionBrowserName {
    Chromium,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawingConversionSourceRoot {
    pub kind: DrawingConversionSourceRootKind,
    pub manifest_sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingConversionSourceRootKind {
    Redacted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawingConversionEntry {
    pub source: DrawingConversionSource,
    pub status: DrawingConversionStatus,
    pub output: Option<DrawingConversionOutput>,
    pub error: Option<DrawingConversionFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawingConversionSource {
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingConversionStatus {
    Converted,
    SkippedEmpty,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawingConversionOutput {
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
    pub width: u32,
    pub height: u32,
    pub mime_type: DrawingConversionMime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawingConversionMime {
    #[serde(rename = "image/png")]
    ImagePng,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawingConversionFailure {
    pub code: DrawingConversionFailureCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingConversionFailureCode {
    InvalidExcalidrawJson,
    UnsupportedExcalidrawSchema,
    ExternalAssetBlocked,
    ResourceLimitExceeded,
    RenderFailed,
    OutputEncodeFailed,
}

impl DrawingConversionFailureCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidExcalidrawJson => "invalid_excalidraw_json",
            Self::UnsupportedExcalidrawSchema => "unsupported_excalidraw_schema",
            Self::ExternalAssetBlocked => "external_asset_blocked",
            Self::ResourceLimitExceeded => "resource_limit_exceeded",
            Self::RenderFailed => "render_failed",
            Self::OutputEncodeFailed => "output_encode_failed",
        }
    }
}

/// A syntactically validated publication whose root has already been
/// canonicalized without following a root or bundle-file symlink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawingConversionPublication {
    root: PathBuf,
    bundle: DrawingConversionBundle,
}

impl DrawingConversionPublication {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub const fn bundle(&self) -> &DrawingConversionBundle {
        &self.bundle
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadDrawingConversionErrorCode {
    RootNotFound,
    RootNotDirectory,
    RootSymlinkNotAllowed,
    BundleNotFound,
    BundleNotRegularFile,
    BundleSymlinkNotAllowed,
    BundleTooLarge,
    BundleUnreadable,
    InvalidJson,
    UnsupportedSchemaVersion,
    InvalidMetadata,
    InvalidRelativePath,
    DuplicateSource,
    DuplicateOutput,
}

#[derive(Debug, Error)]
pub enum LoadDrawingConversionError {
    #[error("drawing conversion publication root does not exist: {path}")]
    RootNotFound { path: PathBuf },
    #[error("drawing conversion publication root is not a directory: {path}")]
    RootNotDirectory { path: PathBuf },
    #[error("drawing conversion publication root must not be a symbolic link: {path}")]
    RootSymlinkNotAllowed { path: PathBuf },
    #[error("drawing conversion bundle does not exist: {path}")]
    BundleNotFound { path: PathBuf },
    #[error("drawing conversion bundle is not a regular file: {path}")]
    BundleNotRegularFile { path: PathBuf },
    #[error("drawing conversion bundle must not be a symbolic link: {path}")]
    BundleSymlinkNotAllowed { path: PathBuf },
    #[error("drawing conversion bundle exceeds {limit_bytes} bytes")]
    BundleTooLarge { limit_bytes: u64 },
    #[error("drawing conversion bundle could not be read: {path}: {source}")]
    BundleUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("drawing conversion bundle is not valid schema JSON: {0}")]
    InvalidJson(#[source] serde_json::Error),
    #[error("unsupported drawing conversion schema version {actual}; expected {expected}")]
    UnsupportedSchemaVersion { actual: u32, expected: u32 },
    #[error("drawing conversion bundle contains empty converter or failure metadata")]
    InvalidMetadata,
    #[error("drawing conversion bundle contains an unsafe or non-canonical relative path: {path}")]
    InvalidRelativePath { path: String },
    #[error("drawing conversion bundle repeats source path {path}")]
    DuplicateSource { path: String },
    #[error("drawing conversion bundle repeats output path {path}")]
    DuplicateOutput { path: String },
}

impl LoadDrawingConversionError {
    #[must_use]
    pub const fn code(&self) -> LoadDrawingConversionErrorCode {
        match self {
            Self::RootNotFound { .. } => LoadDrawingConversionErrorCode::RootNotFound,
            Self::RootNotDirectory { .. } => LoadDrawingConversionErrorCode::RootNotDirectory,
            Self::RootSymlinkNotAllowed { .. } => {
                LoadDrawingConversionErrorCode::RootSymlinkNotAllowed
            }
            Self::BundleNotFound { .. } => LoadDrawingConversionErrorCode::BundleNotFound,
            Self::BundleNotRegularFile { .. } => {
                LoadDrawingConversionErrorCode::BundleNotRegularFile
            }
            Self::BundleSymlinkNotAllowed { .. } => {
                LoadDrawingConversionErrorCode::BundleSymlinkNotAllowed
            }
            Self::BundleTooLarge { .. } => LoadDrawingConversionErrorCode::BundleTooLarge,
            Self::BundleUnreadable { .. } => LoadDrawingConversionErrorCode::BundleUnreadable,
            Self::InvalidJson(_) => LoadDrawingConversionErrorCode::InvalidJson,
            Self::UnsupportedSchemaVersion { .. } => {
                LoadDrawingConversionErrorCode::UnsupportedSchemaVersion
            }
            Self::InvalidMetadata => LoadDrawingConversionErrorCode::InvalidMetadata,
            Self::InvalidRelativePath { .. } => LoadDrawingConversionErrorCode::InvalidRelativePath,
            Self::DuplicateSource { .. } => LoadDrawingConversionErrorCode::DuplicateSource,
            Self::DuplicateOutput { .. } => LoadDrawingConversionErrorCode::DuplicateOutput,
        }
    }
}

pub fn load_drawing_conversion_publication(
    root: impl AsRef<Path>,
) -> Result<DrawingConversionPublication, LoadDrawingConversionError> {
    load_drawing_conversion_publication_with_limit(root, DEFAULT_MAX_BUNDLE_BYTES)
}

pub fn load_drawing_conversion_publication_with_limit(
    root: impl AsRef<Path>,
    max_bundle_bytes: u64,
) -> Result<DrawingConversionPublication, LoadDrawingConversionError> {
    let root = root.as_ref();
    let metadata = fs::symlink_metadata(root).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => LoadDrawingConversionError::RootNotFound {
            path: root.to_owned(),
        },
        _ => LoadDrawingConversionError::BundleUnreadable {
            path: root.to_owned(),
            source: error,
        },
    })?;
    if metadata.file_type().is_symlink() {
        return Err(LoadDrawingConversionError::RootSymlinkNotAllowed {
            path: root.to_owned(),
        });
    }
    if !metadata.is_dir() {
        return Err(LoadDrawingConversionError::RootNotDirectory {
            path: root.to_owned(),
        });
    }
    let root =
        fs::canonicalize(root).map_err(|source| LoadDrawingConversionError::BundleUnreadable {
            path: root.to_owned(),
            source,
        })?;
    let bundle_path = root.join(DRAWING_CONVERSION_BUNDLE_FILENAME);
    let metadata = fs::symlink_metadata(&bundle_path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => LoadDrawingConversionError::BundleNotFound {
            path: bundle_path.clone(),
        },
        _ => LoadDrawingConversionError::BundleUnreadable {
            path: bundle_path.clone(),
            source: error,
        },
    })?;
    if metadata.file_type().is_symlink() {
        return Err(LoadDrawingConversionError::BundleSymlinkNotAllowed { path: bundle_path });
    }
    if !metadata.is_file() {
        return Err(LoadDrawingConversionError::BundleNotRegularFile { path: bundle_path });
    }
    if max_bundle_bytes == 0 || metadata.len() > max_bundle_bytes {
        return Err(LoadDrawingConversionError::BundleTooLarge {
            limit_bytes: max_bundle_bytes,
        });
    }
    let mut file = File::open(&bundle_path).map_err(|source| {
        LoadDrawingConversionError::BundleUnreadable {
            path: bundle_path.clone(),
            source,
        }
    })?;
    verify_open_file_beneath_root(&file, &root).map_err(|source| {
        LoadDrawingConversionError::BundleUnreadable {
            path: bundle_path.clone(),
            source,
        }
    })?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bundle_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| LoadDrawingConversionError::BundleUnreadable {
            path: bundle_path.clone(),
            source,
        })?;
    if bytes.len() as u64 > max_bundle_bytes {
        return Err(LoadDrawingConversionError::BundleTooLarge {
            limit_bytes: max_bundle_bytes,
        });
    }
    let final_metadata =
        file.metadata()
            .map_err(|source| LoadDrawingConversionError::BundleUnreadable {
                path: bundle_path,
                source,
            })?;
    if !final_metadata.is_file() || final_metadata.len() != bytes.len() as u64 {
        return Err(LoadDrawingConversionError::BundleUnreadable {
            path: root.join(DRAWING_CONVERSION_BUNDLE_FILENAME),
            source: io::Error::other("bundle changed while it was being read"),
        });
    }
    let bundle: DrawingConversionBundle =
        serde_json::from_slice(&bytes).map_err(LoadDrawingConversionError::InvalidJson)?;
    validate_bundle(&bundle)?;
    Ok(DrawingConversionPublication { root, bundle })
}

fn validate_bundle(bundle: &DrawingConversionBundle) -> Result<(), LoadDrawingConversionError> {
    if bundle.schema_version != DRAWING_CONVERSION_SCHEMA_VERSION {
        return Err(LoadDrawingConversionError::UnsupportedSchemaVersion {
            actual: bundle.schema_version,
            expected: DRAWING_CONVERSION_SCHEMA_VERSION,
        });
    }
    let converter = &bundle.converter;
    if [
        &converter.name,
        &converter.version,
        &converter.excalidraw_version,
        &converter.playwright_version,
        &converter.browser_version,
    ]
    .into_iter()
    .any(|value| !is_safe_metadata(value, 128))
    {
        return Err(LoadDrawingConversionError::InvalidMetadata);
    }

    let mut sources = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for entry in &bundle.drawings {
        validate_relative_path(&entry.source.relative_path)?;
        if !entry.source.relative_path.starts_with("draws/") {
            return Err(LoadDrawingConversionError::InvalidRelativePath {
                path: entry.source.relative_path.clone(),
            });
        }
        if !sources.insert(entry.source.relative_path.as_str()) {
            return Err(LoadDrawingConversionError::DuplicateSource {
                path: entry.source.relative_path.clone(),
            });
        }
        match (entry.status, &entry.output, &entry.error) {
            (DrawingConversionStatus::Converted, Some(output), None) => {
                validate_relative_path(&output.relative_path)?;
                if !output.relative_path.starts_with("artifacts/")
                    || !output.relative_path.ends_with(".png")
                {
                    return Err(LoadDrawingConversionError::InvalidRelativePath {
                        path: output.relative_path.clone(),
                    });
                }
                if !outputs.insert(output.relative_path.as_str()) {
                    return Err(LoadDrawingConversionError::DuplicateOutput {
                        path: output.relative_path.clone(),
                    });
                }
                if output.size_bytes == 0 || output.width == 0 || output.height == 0 {
                    return Err(LoadDrawingConversionError::InvalidMetadata);
                }
            }
            (DrawingConversionStatus::SkippedEmpty, None, None) => {}
            (DrawingConversionStatus::Failed, None, Some(error)) => {
                if !is_safe_metadata(&error.message, 1_024) {
                    return Err(LoadDrawingConversionError::InvalidMetadata);
                }
            }
            _ => return Err(LoadDrawingConversionError::InvalidMetadata),
        }
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), LoadDrawingConversionError> {
    let parsed = Path::new(path);
    let canonical_text = path.nfc().collect::<String>();
    if path.is_empty()
        || path.len() > 4_096
        || path != canonical_text
        || path.contains('\\')
        || parsed.is_absolute()
        || parsed.components().any(|component| {
            !matches!(component, Component::Normal(_))
                || component
                    .as_os_str()
                    .to_str()
                    .is_none_or(|text| text.is_empty() || text.contains('/'))
        })
    {
        return Err(LoadDrawingConversionError::InvalidRelativePath {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn is_safe_metadata(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_bytes
        && value.chars().all(|character| !character.is_control())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_open_file_beneath_root(file: &File, root: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let opened = fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    if !opened.starts_with(root) {
        return Err(io::Error::other("opened bundle escaped publication root"));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn verify_open_file_beneath_root(_file: &File, _root: &Path) -> io::Result<()> {
    Ok(())
}

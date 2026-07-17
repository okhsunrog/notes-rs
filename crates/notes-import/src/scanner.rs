use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::config_not_found_diagnostic;
use crate::{
    DiagnosticCode, DocumentFormat, GraphManifest, ImportDiagnostic, LogseqConfig, ManifestEntry,
    RelativeDirectory, Sha256Digest, SourceKind, parse_logseq_config,
};

const CONFIG_PATH: &str = "logseq/config.edn";
const DRAWINGS_DIRECTORY: &str = "draws";

/// Resource bounds for an untrusted graph scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanLimits {
    /// Maximum number of filesystem entries inspected below source roots,
    /// including `logseq/config.edn` and directories that are then skipped.
    pub max_entries: u64,
    /// Maximum combined byte size of every manifest entry.
    pub max_total_bytes: u64,
    /// Maximum byte size of any individual source file.
    pub max_file_bytes: u64,
    /// Maximum byte size of a page or journal document. This deliberately
    /// does not constrain assets or drawings.
    pub max_document_bytes: u64,
    /// Maximum combined byte size of page and journal documents. Assets and
    /// drawings use only the independent file/global scan budgets.
    pub max_total_document_bytes: u64,
    /// Additional, tighter limit for `logseq/config.edn`.
    pub max_config_bytes: u64,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_total_bytes: 256 * 1024 * 1024 * 1024,
            max_file_bytes: 16 * 1024 * 1024 * 1024,
            max_document_bytes: 64 * 1024 * 1024,
            max_total_document_bytes: 1024 * 1024 * 1024,
            max_config_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Default)]
struct ScanBudget {
    inspected_entries: u64,
    total_bytes: u64,
    total_document_bytes: u64,
}

#[derive(Debug, Default)]
struct ScanAccumulator {
    budget: ScanBudget,
    entries: Vec<ManifestEntry>,
    diagnostics: Vec<ImportDiagnostic>,
}

impl ScanBudget {
    fn ensure_entry_available(&self, limits: &ScanLimits) -> Result<(), ScanError> {
        if self.inspected_entries >= limits.max_entries {
            return Err(ScanError::EntryLimitExceeded {
                limit: limits.max_entries,
            });
        }
        Ok(())
    }

    fn record_entry(&mut self) {
        self.inspected_entries += 1;
    }

    fn check_candidate(
        &self,
        relative_path: &str,
        size_bytes: u64,
        config_limit: Option<u64>,
        limits: &ScanLimits,
    ) -> Result<(), ScanError> {
        if config_limit.is_some_and(|limit| size_bytes > limit) {
            return Err(ScanError::ConfigTooLarge {
                limit_bytes: config_limit.expect("checked as some"),
            });
        }
        if size_bytes > limits.max_file_bytes {
            return Err(ScanError::FileTooLarge {
                relative_path: relative_path.to_owned(),
                limit_bytes: limits.max_file_bytes,
            });
        }
        if self
            .total_bytes
            .checked_add(size_bytes)
            .is_none_or(|total| total > limits.max_total_bytes)
        {
            return Err(ScanError::TotalBytesLimitExceeded {
                limit_bytes: limits.max_total_bytes,
            });
        }
        Ok(())
    }

    fn check_document_candidate(
        &self,
        relative_path: &str,
        size_bytes: u64,
        kind: SourceKind,
        limits: &ScanLimits,
    ) -> Result<(), ScanError> {
        if matches!(kind, SourceKind::Page | SourceKind::Journal)
            && size_bytes > limits.max_document_bytes
        {
            return Err(ScanError::DocumentTooLarge {
                relative_path: relative_path.to_owned(),
                limit_bytes: limits.max_document_bytes,
            });
        }
        if matches!(kind, SourceKind::Page | SourceKind::Journal)
            && self
                .total_document_bytes
                .checked_add(size_bytes)
                .is_none_or(|total| total > limits.max_total_document_bytes)
        {
            return Err(ScanError::TotalDocumentBytesLimitExceeded {
                limit_bytes: limits.max_total_document_bytes,
            });
        }
        Ok(())
    }

    fn record_bytes(&mut self, size_bytes: u64) {
        self.total_bytes += size_bytes;
    }

    fn record_document_bytes(&mut self, kind: SourceKind, size_bytes: u64) {
        if matches!(kind, SourceKind::Page | SourceKind::Journal) {
            self.total_document_bytes += size_bytes;
        }
    }

    fn remaining_bytes(&self, limits: &ScanLimits) -> u64 {
        limits.max_total_bytes.saturating_sub(self.total_bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphScanReport {
    pub manifest: GraphManifest,
    pub diagnostics: Vec<ImportDiagnostic>,
}

/// Result of re-scanning a source immediately before a later commit boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ManifestVerification {
    Exact {
        current: GraphScanReport,
    },
    Changed {
        expected_sha256: Sha256Digest,
        current: GraphScanReport,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanErrorCode {
    RootNotFound,
    RootNotDirectory,
    SymlinkNotAllowed,
    PathEscapesRoot,
    DirectoryOverlap,
    NonUtf8Path,
    NonPortablePath,
    UnsupportedFileType,
    SourceChanged,
    EntryLimitExceeded,
    TotalBytesLimitExceeded,
    FileTooLarge,
    DocumentTooLarge,
    TotalDocumentBytesLimitExceeded,
    ConfigTooLarge,
    InvalidConfig,
    Io,
}

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("graph root does not exist: {path}")]
    RootNotFound { path: PathBuf },
    #[error("graph root is not a directory: {path}")]
    RootNotDirectory { path: PathBuf },
    #[error("symbolic links are not allowed in import sources: {relative_path}")]
    SymlinkNotAllowed { relative_path: String },
    #[error("source path escapes the selected graph root: {path}")]
    PathEscapesRoot { path: PathBuf },
    #[error("source directories overlap: {first} and {second}")]
    DirectoryOverlap { first: String, second: String },
    #[error("source path is not valid UTF-8: {path}")]
    NonUtf8Path { path: PathBuf },
    #[error("source path contains control characters and cannot be represented safely")]
    NonPortablePath,
    #[error("unsupported source file type: {relative_path}")]
    UnsupportedFileType { relative_path: String },
    #[error("source changed while it was being scanned: {relative_path}")]
    SourceChanged { relative_path: String },
    #[error("source contains more than the configured limit of {limit} files")]
    EntryLimitExceeded { limit: u64 },
    #[error("source exceeds the configured total size limit of {limit_bytes} bytes")]
    TotalBytesLimitExceeded { limit_bytes: u64 },
    #[error("source file {relative_path} exceeds the {limit_bytes}-byte limit")]
    FileTooLarge {
        relative_path: String,
        limit_bytes: u64,
    },
    #[error("source document {relative_path} exceeds the {limit_bytes}-byte parse safety limit")]
    DocumentTooLarge {
        relative_path: String,
        limit_bytes: u64,
    },
    #[error("source documents exceed the configured total size limit of {limit_bytes} bytes")]
    TotalDocumentBytesLimitExceeded { limit_bytes: u64 },
    #[error("Logseq config exceeds the {limit_bytes}-byte safety limit")]
    ConfigTooLarge { limit_bytes: u64 },
    #[error("invalid Logseq config: {source}")]
    InvalidConfig {
        #[source]
        source: crate::ConfigError,
    },
    #[error("could not {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl ScanError {
    pub const fn code(&self) -> ScanErrorCode {
        match self {
            Self::RootNotFound { .. } => ScanErrorCode::RootNotFound,
            Self::RootNotDirectory { .. } => ScanErrorCode::RootNotDirectory,
            Self::SymlinkNotAllowed { .. } => ScanErrorCode::SymlinkNotAllowed,
            Self::PathEscapesRoot { .. } => ScanErrorCode::PathEscapesRoot,
            Self::DirectoryOverlap { .. } => ScanErrorCode::DirectoryOverlap,
            Self::NonUtf8Path { .. } => ScanErrorCode::NonUtf8Path,
            Self::NonPortablePath => ScanErrorCode::NonPortablePath,
            Self::UnsupportedFileType { .. } => ScanErrorCode::UnsupportedFileType,
            Self::SourceChanged { .. } => ScanErrorCode::SourceChanged,
            Self::EntryLimitExceeded { .. } => ScanErrorCode::EntryLimitExceeded,
            Self::TotalBytesLimitExceeded { .. } => ScanErrorCode::TotalBytesLimitExceeded,
            Self::FileTooLarge { .. } => ScanErrorCode::FileTooLarge,
            Self::DocumentTooLarge { .. } => ScanErrorCode::DocumentTooLarge,
            Self::TotalDocumentBytesLimitExceeded { .. } => {
                ScanErrorCode::TotalDocumentBytesLimitExceeded
            }
            Self::ConfigTooLarge { .. } => ScanErrorCode::ConfigTooLarge,
            Self::InvalidConfig { .. } => ScanErrorCode::InvalidConfig,
            Self::Io { .. } => ScanErrorCode::Io,
        }
    }
}

/// Scan a Logseq graph without following links or modifying source files.
///
/// The returned manifest is independent of the graph's absolute location and
/// filesystem enumeration order.
pub fn scan_logseq_graph(root: impl AsRef<Path>) -> Result<GraphScanReport, ScanError> {
    scan_logseq_graph_with_limits(root, &ScanLimits::default())
}

/// Scan with caller-supplied resource limits.
pub fn scan_logseq_graph_with_limits(
    root: impl AsRef<Path>,
    limits: &ScanLimits,
) -> Result<GraphScanReport, ScanError> {
    let root = root.as_ref();
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ScanError::RootNotFound {
                path: root.to_path_buf(),
            });
        }
        Err(source) => return Err(io_error("inspect", root, source)),
    };
    if root_metadata.file_type().is_symlink() {
        return Err(ScanError::SymlinkNotAllowed {
            relative_path: ".".to_owned(),
        });
    }
    if !root_metadata.is_dir() {
        return Err(ScanError::RootNotDirectory {
            path: root.to_path_buf(),
        });
    }
    let canonical_root =
        fs::canonicalize(root).map_err(|error| io_error("resolve", root, error))?;

    let mut accumulator = ScanAccumulator::default();
    let config = read_config(root, &canonical_root, limits, &mut accumulator)?;
    validate_directories_do_not_overlap(&config)?;

    scan_source_directory(
        root,
        &canonical_root,
        &config.pages_directory,
        SourceKind::Page,
        true,
        limits,
        &mut accumulator,
    )?;
    scan_source_directory(
        root,
        &canonical_root,
        &RelativeDirectory::new(DRAWINGS_DIRECTORY)
            .expect("the built-in drawings directory must be safe"),
        SourceKind::Drawing,
        false,
        limits,
        &mut accumulator,
    )?;
    scan_source_directory(
        root,
        &canonical_root,
        &config.journals_directory,
        SourceKind::Journal,
        true,
        limits,
        &mut accumulator,
    )?;
    scan_source_directory(
        root,
        &canonical_root,
        &config.assets_directory,
        SourceKind::Asset,
        true,
        limits,
        &mut accumulator,
    )?;

    accumulator.diagnostics.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(left.code.cmp(&right.code))
    });
    Ok(GraphScanReport {
        manifest: GraphManifest::build(config, accumulator.entries),
        diagnostics: accumulator.diagnostics,
    })
}

/// Re-scan exact source bytes and compare them with an earlier immutable
/// manifest. A host must call this after dry-run and before applying an import.
pub fn verify_logseq_manifest(
    root: impl AsRef<Path>,
    expected: &GraphManifest,
) -> Result<ManifestVerification, ScanError> {
    verify_logseq_manifest_with_limits(root, expected, &ScanLimits::default())
}

/// Re-scan with the same explicit bounds used for the original dry-run.
pub fn verify_logseq_manifest_with_limits(
    root: impl AsRef<Path>,
    expected: &GraphManifest,
    limits: &ScanLimits,
) -> Result<ManifestVerification, ScanError> {
    let current = scan_logseq_graph_with_limits(root, limits)?;
    if current.manifest == *expected {
        Ok(ManifestVerification::Exact { current })
    } else {
        Ok(ManifestVerification::Changed {
            expected_sha256: expected.sha256(),
            current,
        })
    }
}

fn read_config(
    root: &Path,
    canonical_root: &Path,
    limits: &ScanLimits,
    accumulator: &mut ScanAccumulator,
) -> Result<LogseqConfig, ScanError> {
    let relative_path = Path::new(CONFIG_PATH);
    let config_path = root.join(relative_path);
    ensure_existing_prefixes_are_not_symlinks(root, relative_path)?;
    match fs::symlink_metadata(&config_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            accumulator.diagnostics.push(config_not_found_diagnostic());
            return Ok(LogseqConfig::default());
        }
        Err(source) => return Err(io_error("inspect", &config_path, source)),
        Ok(_) => {}
    }

    ensure_path_chain_is_safe(root, canonical_root, relative_path)?;
    let metadata =
        fs::metadata(&config_path).map_err(|error| io_error("inspect", &config_path, error))?;
    if !metadata.is_file() {
        return Err(ScanError::UnsupportedFileType {
            relative_path: CONFIG_PATH.to_owned(),
        });
    }
    accumulator.budget.ensure_entry_available(limits)?;
    accumulator.budget.check_candidate(
        CONFIG_PATH,
        metadata.len(),
        Some(limits.max_config_bytes),
        limits,
    )?;
    let bytes = read_config_bytes(root, &config_path, limits, &accumulator.budget)?;
    let text = std::str::from_utf8(&bytes).map_err(|error| ScanError::InvalidConfig {
        source: crate::ConfigError::Malformed {
            offset: error.valid_up_to(),
            message: "config is not valid UTF-8".to_owned(),
        },
    })?;
    let parsed = parse_logseq_config(text).map_err(|source| ScanError::InvalidConfig { source })?;
    accumulator.diagnostics.extend(parsed.diagnostics);
    accumulator.entries.push(ManifestEntry {
        kind: SourceKind::Config,
        document_format: None,
        relative_path: CONFIG_PATH.to_owned(),
        size_bytes: bytes.len() as u64,
        sha256: digest_bytes(&bytes),
    });
    accumulator.budget.record_entry();
    accumulator.budget.record_bytes(bytes.len() as u64);
    Ok(parsed.config)
}

fn read_config_bytes(
    root: &Path,
    path: &Path,
    limits: &ScanLimits,
    budget: &ScanBudget,
) -> Result<Vec<u8>, ScanError> {
    let before = fs::symlink_metadata(path).map_err(|error| io_error("inspect", path, error))?;
    if before.file_type().is_symlink() {
        return Err(ScanError::SymlinkNotAllowed {
            relative_path: CONFIG_PATH.to_owned(),
        });
    }
    budget.check_candidate(
        CONFIG_PATH,
        before.len(),
        Some(limits.max_config_bytes),
        limits,
    )?;

    let read_limit = limits
        .max_config_bytes
        .min(limits.max_file_bytes)
        .min(budget.remaining_bytes(limits));
    let file = File::open(path).map_err(|error| io_error("open", path, error))?;
    let mut bytes = Vec::with_capacity(before.len().min(64 * 1024) as usize);
    file.take(read_limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read", path, error))?;
    budget.check_candidate(
        CONFIG_PATH,
        bytes.len() as u64,
        Some(limits.max_config_bytes),
        limits,
    )?;

    let after = fs::symlink_metadata(path).map_err(|error| io_error("inspect", path, error))?;
    if after.file_type().is_symlink()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || bytes.len() as u64 != after.len()
    {
        return Err(ScanError::SourceChanged {
            relative_path: portable_relative_path(root, path)?,
        });
    }
    Ok(bytes)
}

fn validate_directories_do_not_overlap(config: &LogseqConfig) -> Result<(), ScanError> {
    let drawings_directory = RelativeDirectory::new(DRAWINGS_DIRECTORY)
        .expect("the built-in drawings directory must be safe");
    let directories = [
        &config.pages_directory,
        &config.journals_directory,
        &config.assets_directory,
        &drawings_directory,
    ];
    for (index, left) in directories.iter().enumerate() {
        for right in directories.iter().skip(index + 1) {
            let left_path = left.to_path_buf();
            let right_path = right.to_path_buf();
            if left_path == right_path
                || left_path.starts_with(&right_path)
                || right_path.starts_with(&left_path)
            {
                return Err(ScanError::DirectoryOverlap {
                    first: left.to_string(),
                    second: right.to_string(),
                });
            }
        }
        if Path::new(CONFIG_PATH).starts_with(left.to_path_buf()) {
            return Err(ScanError::DirectoryOverlap {
                first: left.to_string(),
                second: CONFIG_PATH.to_owned(),
            });
        }
    }
    Ok(())
}

fn scan_source_directory(
    root: &Path,
    canonical_root: &Path,
    relative_directory: &RelativeDirectory,
    kind: SourceKind,
    warn_when_missing: bool,
    limits: &ScanLimits,
    accumulator: &mut ScanAccumulator,
) -> Result<(), ScanError> {
    let relative_path = relative_directory.to_path_buf();
    let source_path = root.join(&relative_path);
    ensure_existing_prefixes_are_not_symlinks(root, &relative_path)?;
    match fs::symlink_metadata(&source_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if warn_when_missing {
                accumulator.diagnostics.push(ImportDiagnostic::warning(
                    DiagnosticCode::SourceDirectoryNotFound,
                    Some(relative_directory.to_string()),
                    format!("{} source directory was not found", source_kind_name(kind)),
                    Some("Verify the configured graph directories before importing".to_owned()),
                ));
            }
            return Ok(());
        }
        Err(source) => return Err(io_error("inspect", &source_path, source)),
        Ok(_) => {}
    }

    ensure_path_chain_is_safe(root, canonical_root, &relative_path)?;
    let metadata =
        fs::metadata(&source_path).map_err(|error| io_error("inspect", &source_path, error))?;
    if !metadata.is_dir() {
        return Err(ScanError::UnsupportedFileType {
            relative_path: relative_directory.to_string(),
        });
    }

    let mut pending = vec![source_path];
    while let Some(directory) = pending.pop() {
        let read_directory = fs::read_dir(&directory)
            .map_err(|error| io_error("read directory", &directory, error))?;
        let mut children = Vec::new();
        for child in read_directory {
            let child =
                child.map_err(|error| io_error("read directory entry in", &directory, error))?;
            accumulator.budget.ensure_entry_available(limits)?;
            accumulator.budget.record_entry();
            children.push(child);
        }
        children.sort_by_key(std::fs::DirEntry::file_name);

        for child in children {
            let path = child.path();
            let relative = portable_relative_path(root, &path)?;
            let file_type = child
                .file_type()
                .map_err(|error| io_error("inspect", &path, error))?;
            if file_type.is_symlink() {
                return Err(ScanError::SymlinkNotAllowed {
                    relative_path: relative,
                });
            }
            if file_type.is_dir() {
                if is_known_ignored_directory(&child.file_name()) {
                    continue;
                }
                ensure_canonical_path_is_within_root(canonical_root, &path)?;
                pending.push(path);
                continue;
            }
            if !file_type.is_file() {
                return Err(ScanError::UnsupportedFileType {
                    relative_path: relative,
                });
            }
            ensure_canonical_path_is_within_root(canonical_root, &path)?;

            let document_format = match kind {
                SourceKind::Page | SourceKind::Journal => {
                    let format = document_format(&path);
                    if format != DocumentFormat::Markdown {
                        accumulator.diagnostics.push(ImportDiagnostic::warning(
                            DiagnosticCode::UnsupportedDocumentFormat,
                            Some(relative.clone()),
                            format!(
                                "{} is not a supported Markdown document",
                                source_kind_name(kind)
                            ),
                            Some("The file is included in the manifest but will require a later format adapter".to_owned()),
                        ));
                    }
                    Some(format)
                }
                SourceKind::Config | SourceKind::Asset | SourceKind::Drawing => None,
            };
            let (size_bytes, sha256) =
                hash_file(root, &path, &relative, kind, limits, &accumulator.budget)?;
            accumulator.entries.push(ManifestEntry {
                kind,
                document_format,
                relative_path: relative,
                size_bytes,
                sha256,
            });
            accumulator.budget.record_bytes(size_bytes);
            accumulator.budget.record_document_bytes(kind, size_bytes);
        }
    }
    Ok(())
}

fn ensure_existing_prefixes_are_not_symlinks(
    root: &Path,
    relative_path: &Path,
) -> Result<(), ScanError> {
    let mut current = root.to_path_buf();
    for component in relative_path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ScanError::SymlinkNotAllowed {
                    relative_path: portable_relative_path(root, &current)?,
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error("inspect", &current, source)),
        }
    }
    Ok(())
}

fn ensure_path_chain_is_safe(
    root: &Path,
    canonical_root: &Path,
    relative_path: &Path,
) -> Result<(), ScanError> {
    let mut current = root.to_path_buf();
    for component in relative_path.components() {
        current.push(component.as_os_str());
        let metadata =
            fs::symlink_metadata(&current).map_err(|error| io_error("inspect", &current, error))?;
        if metadata.file_type().is_symlink() {
            return Err(ScanError::SymlinkNotAllowed {
                relative_path: portable_relative_path(root, &current)?,
            });
        }
    }
    ensure_canonical_path_is_within_root(canonical_root, &current)
}

fn ensure_canonical_path_is_within_root(
    canonical_root: &Path,
    path: &Path,
) -> Result<(), ScanError> {
    let canonical = fs::canonicalize(path).map_err(|error| io_error("resolve", path, error))?;
    if !canonical.starts_with(canonical_root) {
        return Err(ScanError::PathEscapesRoot {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn portable_relative_path(root: &Path, path: &Path) -> Result<String, ScanError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ScanError::PathEscapesRoot {
            path: path.to_path_buf(),
        })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let value = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| ScanError::NonUtf8Path {
                path: path.to_path_buf(),
            })?;
        if value.chars().any(char::is_control) {
            return Err(ScanError::NonPortablePath);
        }
        parts.push(value);
    }
    Ok(parts.join("/"))
}

fn hash_file(
    root: &Path,
    path: &Path,
    relative_path: &str,
    kind: SourceKind,
    limits: &ScanLimits,
    budget: &ScanBudget,
) -> Result<(u64, Sha256Digest), ScanError> {
    let before = fs::symlink_metadata(path).map_err(|error| io_error("inspect", path, error))?;
    if before.file_type().is_symlink() {
        return Err(ScanError::SymlinkNotAllowed {
            relative_path: portable_relative_path(root, path)?,
        });
    }
    budget.check_candidate(relative_path, before.len(), None, limits)?;
    budget.check_document_candidate(relative_path, before.len(), kind, limits)?;
    let mut file = File::open(path).map_err(|error| io_error("open", path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let readable_bytes = limits
            .max_file_bytes
            .saturating_sub(size)
            .min(budget.remaining_bytes(limits).saturating_sub(size));
        let read_size = readable_bytes.saturating_add(1).min(buffer.len() as u64) as usize;
        let read = file
            .read(&mut buffer[..read_size])
            .map_err(|error| io_error("read", path, error))?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| io_error("hash", path, io::Error::other("file is too large")))?;
        budget.check_candidate(relative_path, size, None, limits)?;
        budget.check_document_candidate(relative_path, size, kind, limits)?;
        hasher.update(&buffer[..read]);
    }
    let after = fs::symlink_metadata(path).map_err(|error| io_error("inspect", path, error))?;
    if after.file_type().is_symlink()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || size != after.len()
    {
        return Err(ScanError::SourceChanged {
            relative_path: portable_relative_path(root, path)?,
        });
    }
    Ok((size, Sha256Digest::from_bytes(hasher.finalize().into())))
}

fn is_known_ignored_directory(file_name: &OsStr) -> bool {
    matches!(
        file_name.to_str(),
        Some(
            ".git"
                | ".obsidian"
                | ".recycle"
                | "backup"
                | "bak"
                | "version-history"
                | "version-files"
        )
    )
}

fn digest_bytes(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}

fn document_format(path: &Path) -> DocumentFormat {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md" | "markdown") => DocumentFormat::Markdown,
        Some("org") => DocumentFormat::Org,
        _ => DocumentFormat::Unknown,
    }
}

const fn source_kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Config => "config",
        SourceKind::Page => "page",
        SourceKind::Journal => "journal",
        SourceKind::Asset => "asset",
        SourceKind::Drawing => "drawing",
    }
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> ScanError {
    ScanError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

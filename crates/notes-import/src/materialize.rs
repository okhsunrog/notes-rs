use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use data_url::DataUrl;
use data_url::forgiving_base64::DecodeError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ImportBlockSource, ImportInlineImageMime, ImportMarkdownRange, ImportMediaKind,
    ImportMediaOwner, ImportMediaReference, ImportMediaResolution, PreparedImport, Sha256Digest,
    SourceRange,
};

const ATTACHMENT_SCHEME: &str = "notes-attachment:";
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Independent resource bounds for the commit-adjacent media read.
///
/// These limits deliberately do not trust the earlier scanner limits. The
/// materializer retains bytes in memory until a host installs them in its blob
/// store, so its default total is much tighter than the graph scanner's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaMaterializationLimits {
    pub max_blob_bytes: u64,
    pub max_total_blob_bytes: u64,
    pub max_blobs: u64,
    pub max_attachments: u64,
    pub max_rewrites: u64,
}

impl Default for MediaMaterializationLimits {
    fn default() -> Self {
        Self {
            max_blob_bytes: 64 * 1024 * 1024,
            max_total_blob_bytes: 1024 * 1024 * 1024,
            max_blobs: 100_000,
            max_attachments: 250_000,
            max_rewrites: 2_000_000,
        }
    }
}

/// Canonical content-addressed bytes. Equal hashes occur only once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedMediaBlob {
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
    pub bytes: Arc<[u8]>,
}

/// Metadata for one owner/blob pair. Its UUID intentionally matches
/// `notes_core::attachment_uuid`, without making this pure import crate depend
/// on persistence and application services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedMediaAttachment {
    pub attachment_uuid: Uuid,
    pub owner: ImportMediaOwner,
    pub sha256: Sha256Digest,
    pub filename: String,
    pub mime: String,
    pub size_bytes: u64,
}

/// A checked, deterministic full-token replacement.
///
/// Rewrites are sorted by block UUID and then by descending byte offset, so a
/// caller can apply each block's entries in order without invalidating later
/// ranges. The caller must use `reference_index` to compare the corresponding
/// `PreparedImport::media_references` spelling immediately before applying it;
/// the potentially very large source token is deliberately not copied here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRewrite {
    pub reference_index: u64,
    pub attachment_uuid: Uuid,
    pub attachment_uri: String,
    pub attachment_owner: ImportMediaOwner,
    pub markdown_block_uuid: Uuid,
    pub markdown_range: ImportMarkdownRange,
    pub replacement_markdown: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaMaterializationIssue {
    InvalidPreparedReference,
    PathEscape,
    SymlinkNotAllowed,
    SourceNotFound,
    SourceNotRegularFile,
    SourceUnreadable,
    SourceChanged,
    BlobTooLarge,
    TotalBlobBytesExceeded,
    BlobLimitExceeded,
    AttachmentLimitExceeded,
    RewriteLimitExceeded,
    InvalidInlineData,
    ConflictingRewrite,
    DeferredExcalidrawConversion,
}

/// A non-fatal failure. The corresponding source token remains unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaMaterializationDiagnostic {
    pub reference_index: u64,
    pub source_document_path: String,
    pub source_range: SourceRange,
    pub issue: MediaMaterializationIssue,
    pub message: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservedMediaReason {
    AlreadyClassified,
    DeferredConversion,
}

/// A lightweight pointer to a reference whose exact source spellings remain in
/// `PreparedImport::media_references` and must be retained unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreservedMediaReference {
    pub reference_index: u64,
    pub owner: ImportMediaOwner,
    pub reason: PreservedMediaReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaMaterializationPlan {
    pub blobs: Vec<MaterializedMediaBlob>,
    pub attachments: Vec<MaterializedMediaAttachment>,
    pub rewrites: Vec<MediaRewrite>,
    pub preserved_references: Vec<PreservedMediaReference>,
    pub diagnostics: Vec<MediaMaterializationDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializeMediaErrorCode {
    RootNotFound,
    RootNotDirectory,
    RootSymlinkNotAllowed,
    InvalidLimits,
    InvalidPreparedPlan,
    ReferenceFailed,
    Io,
}

#[derive(Debug, Error)]
pub enum MaterializeMediaError {
    #[error("graph root does not exist: {path}")]
    RootNotFound { path: PathBuf },
    #[error("graph root is not a directory: {path}")]
    RootNotDirectory { path: PathBuf },
    #[error("graph root must not be a symbolic link: {path}")]
    RootSymlinkNotAllowed { path: PathBuf },
    #[error("media materialization limits must all be non-zero")]
    InvalidLimits,
    #[error("prepared import contains inconsistent page or block identities")]
    InvalidPreparedPlan,
    #[error("media reference {reference_index} failed materialization: {issue:?}")]
    ReferenceFailed {
        reference_index: u64,
        source_document_path: String,
        source_range: SourceRange,
        issue: MediaMaterializationIssue,
    },
    #[error("could not inspect graph root {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl MaterializeMediaError {
    #[must_use]
    pub const fn code(&self) -> MaterializeMediaErrorCode {
        match self {
            Self::RootNotFound { .. } => MaterializeMediaErrorCode::RootNotFound,
            Self::RootNotDirectory { .. } => MaterializeMediaErrorCode::RootNotDirectory,
            Self::RootSymlinkNotAllowed { .. } => MaterializeMediaErrorCode::RootSymlinkNotAllowed,
            Self::InvalidLimits => MaterializeMediaErrorCode::InvalidLimits,
            Self::InvalidPreparedPlan => MaterializeMediaErrorCode::InvalidPreparedPlan,
            Self::ReferenceFailed { .. } => MaterializeMediaErrorCode::ReferenceFailed,
            Self::Io { .. } => MaterializeMediaErrorCode::Io,
        }
    }

    #[must_use]
    pub const fn reference_issue(&self) -> Option<MediaMaterializationIssue> {
        match self {
            Self::ReferenceFailed { issue, .. } => Some(*issue),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct CanonicalMedia {
    sha256: Sha256Digest,
    bytes: Arc<[u8]>,
    filename: String,
    mime: String,
}

impl CanonicalMedia {
    fn size_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }
}

#[derive(Debug, Default)]
struct MaterializationState {
    blobs: BTreeMap<Sha256Digest, MaterializedMediaBlob>,
    attachments: BTreeMap<Uuid, MaterializedMediaAttachment>,
    rewrites: Vec<MediaRewrite>,
    preserved_references: Vec<PreservedMediaReference>,
    diagnostics: Vec<MediaMaterializationDiagnostic>,
    total_blob_bytes: u64,
}

struct MarkdownOwnerIndex<'prepared> {
    blocks: HashMap<Uuid, &'prepared str>,
    preamble_blocks: HashMap<Uuid, Uuid>,
}

/// Materializes classified local and inline media using conservative defaults.
pub fn materialize_source_media(
    prepared: &PreparedImport,
    source_root: impl AsRef<Path>,
) -> Result<MediaMaterializationPlan, MaterializeMediaError> {
    materialize_source_media_with_limits(prepared, source_root, &Default::default())
}

/// Re-reads only media that `PreparedImport` already classified as local or
/// inline, revalidates all byte evidence, and returns an inert installation and
/// rewrite plan. It never mutates source files or the prepared page Markdown.
pub fn materialize_source_media_with_limits(
    prepared: &PreparedImport,
    source_root: impl AsRef<Path>,
    limits: &MediaMaterializationLimits,
) -> Result<MediaMaterializationPlan, MaterializeMediaError> {
    validate_limits(limits)?;
    let source_root = validate_root(source_root.as_ref())?;
    let owners = MarkdownOwnerIndex::new(prepared)?;
    let mut state = MaterializationState::default();

    for (index, reference) in prepared.media_references.iter().enumerate() {
        let reference_index = index as u64;
        if reference.kind == ImportMediaKind::LegacyExcalidraw {
            preserve(
                &mut state,
                reference_index,
                reference,
                PreservedMediaReason::DeferredConversion,
            );
            state.diagnostics.push(MediaMaterializationDiagnostic {
                reference_index,
                source_document_path: reference.relative_path.clone(),
                source_range: reference.source_range,
                issue: MediaMaterializationIssue::DeferredExcalidrawConversion,
                message: issue_message(MediaMaterializationIssue::DeferredExcalidrawConversion),
            });
            continue;
        }
        let result = match &reference.resolution {
            ImportMediaResolution::LocalManifest {
                relative_path,
                size_bytes,
                sha256,
            } => {
                preflight_reference(reference, *sha256, *size_bytes, limits, &state)
                    .map_err(|issue| reference_error(reference_index, reference, issue))?;
                let existing = state.blobs.get(sha256).map(|blob| Arc::clone(&blob.bytes));
                materialize_local(
                    &source_root,
                    relative_path,
                    *size_bytes,
                    *sha256,
                    reference.kind,
                    limits,
                    existing,
                )
            }
            ImportMediaResolution::InlineData {
                mime,
                decoded_size_bytes,
                decoded_sha256,
            } => {
                preflight_reference(
                    reference,
                    *decoded_sha256,
                    *decoded_size_bytes,
                    limits,
                    &state,
                )
                .map_err(|issue| reference_error(reference_index, reference, issue))?;
                let existing = state
                    .blobs
                    .get(decoded_sha256)
                    .map(|blob| Arc::clone(&blob.bytes));
                materialize_inline(
                    reference,
                    *mime,
                    *decoded_size_bytes,
                    *decoded_sha256,
                    limits,
                    existing,
                )
            }
            ImportMediaResolution::RemoteBlocked { .. }
            | ImportMediaResolution::Missing { .. }
            | ImportMediaResolution::Blocked { .. }
            | ImportMediaResolution::Unsupported { .. } => {
                preserve(
                    &mut state,
                    reference_index,
                    reference,
                    PreservedMediaReason::AlreadyClassified,
                );
                continue;
            }
        };

        let media = result.map_err(|issue| reference_error(reference_index, reference, issue))?;
        add_materialized_reference(&owners, reference_index, reference, media, &mut state)
            .map_err(|issue| reference_error(reference_index, reference, issue))?;
    }

    validate_non_overlapping_rewrites(&state.rewrites).map_err(|reference_index| {
        reference_error(
            reference_index,
            &prepared.media_references[reference_index as usize],
            MediaMaterializationIssue::ConflictingRewrite,
        )
    })?;
    state.rewrites.sort_by(|left, right| {
        left.markdown_block_uuid
            .cmp(&right.markdown_block_uuid)
            .then(
                right
                    .markdown_range
                    .start_byte
                    .cmp(&left.markdown_range.start_byte),
            )
            .then(left.reference_index.cmp(&right.reference_index))
    });

    Ok(MediaMaterializationPlan {
        blobs: state.blobs.into_values().collect(),
        attachments: state.attachments.into_values().collect(),
        rewrites: state.rewrites,
        preserved_references: state.preserved_references,
        diagnostics: state.diagnostics,
    })
}

impl<'prepared> MarkdownOwnerIndex<'prepared> {
    fn new(prepared: &'prepared PreparedImport) -> Result<Self, MaterializeMediaError> {
        let mut blocks = HashMap::new();
        let mut preamble_blocks = HashMap::new();
        let mut object_uuids = HashSet::new();
        for page in &prepared.pages {
            if !object_uuids.insert(page.uuid) {
                return Err(MaterializeMediaError::InvalidPreparedPlan);
            }
            let mut preamble = None;
            for block in &page.blocks {
                if block.page_uuid != page.uuid
                    || !object_uuids.insert(block.uuid)
                    || blocks.insert(block.uuid, block.markdown.as_str()).is_some()
                {
                    return Err(MaterializeMediaError::InvalidPreparedPlan);
                }
                if matches!(block.provenance.source, ImportBlockSource::Preamble)
                    && preamble.replace(block.uuid).is_some()
                {
                    return Err(MaterializeMediaError::InvalidPreparedPlan);
                }
            }
            if let Some(block_uuid) = preamble {
                preamble_blocks.insert(page.uuid, block_uuid);
            }
        }
        Ok(Self {
            blocks,
            preamble_blocks,
        })
    }

    fn markdown_block(
        &self,
        owner: ImportMediaOwner,
    ) -> Result<(Uuid, &'prepared str), MediaMaterializationIssue> {
        let block_uuid = match owner {
            ImportMediaOwner::Block { block_uuid } => block_uuid,
            ImportMediaOwner::Page { page_uuid } => *self
                .preamble_blocks
                .get(&page_uuid)
                .ok_or(MediaMaterializationIssue::InvalidPreparedReference)?,
        };
        self.blocks
            .get(&block_uuid)
            .copied()
            .map(|markdown| (block_uuid, markdown))
            .ok_or(MediaMaterializationIssue::InvalidPreparedReference)
    }
}

fn validate_limits(limits: &MediaMaterializationLimits) -> Result<(), MaterializeMediaError> {
    if limits.max_blob_bytes == 0
        || limits.max_total_blob_bytes == 0
        || limits.max_blobs == 0
        || limits.max_attachments == 0
        || limits.max_rewrites == 0
    {
        return Err(MaterializeMediaError::InvalidLimits);
    }
    Ok(())
}

fn preflight_reference(
    reference: &ImportMediaReference,
    sha256: Sha256Digest,
    size_bytes: u64,
    limits: &MediaMaterializationLimits,
    state: &MaterializationState,
) -> Result<(), MediaMaterializationIssue> {
    if size_bytes > limits.max_blob_bytes {
        return Err(MediaMaterializationIssue::BlobTooLarge);
    }
    if !state.blobs.contains_key(&sha256) {
        if state.blobs.len() as u64 >= limits.max_blobs {
            return Err(MediaMaterializationIssue::BlobLimitExceeded);
        }
        let next_total = state
            .total_blob_bytes
            .checked_add(size_bytes)
            .ok_or(MediaMaterializationIssue::TotalBlobBytesExceeded)?;
        if next_total > limits.max_total_blob_bytes {
            return Err(MediaMaterializationIssue::TotalBlobBytesExceeded);
        }
    }
    let attachment_uuid = materialized_attachment_uuid(reference.owner, sha256);
    if !state.attachments.contains_key(&attachment_uuid)
        && state.attachments.len() as u64 >= limits.max_attachments
    {
        return Err(MediaMaterializationIssue::AttachmentLimitExceeded);
    }
    if state.rewrites.len() as u64 >= limits.max_rewrites {
        return Err(MediaMaterializationIssue::RewriteLimitExceeded);
    }
    Ok(())
}

fn reference_error(
    reference_index: u64,
    reference: &ImportMediaReference,
    issue: MediaMaterializationIssue,
) -> MaterializeMediaError {
    MaterializeMediaError::ReferenceFailed {
        reference_index,
        source_document_path: reference.relative_path.clone(),
        source_range: reference.source_range,
        issue,
    }
}

fn validate_root(source_root: &Path) -> Result<PathBuf, MaterializeMediaError> {
    let metadata = match fs::symlink_metadata(source_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(MaterializeMediaError::RootNotFound {
                path: source_root.to_owned(),
            });
        }
        Err(source) => {
            return Err(MaterializeMediaError::Io {
                path: source_root.to_owned(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(MaterializeMediaError::RootSymlinkNotAllowed {
            path: source_root.to_owned(),
        });
    }
    if !metadata.is_dir() {
        return Err(MaterializeMediaError::RootNotDirectory {
            path: source_root.to_owned(),
        });
    }
    fs::canonicalize(source_root).map_err(|source| MaterializeMediaError::Io {
        path: source_root.to_owned(),
        source,
    })
}

#[allow(clippy::too_many_arguments)]
fn materialize_local(
    source_root: &Path,
    relative_path: &str,
    expected_size: u64,
    expected_sha256: Sha256Digest,
    kind: ImportMediaKind,
    limits: &MediaMaterializationLimits,
    existing_blob: Option<Arc<[u8]>>,
) -> Result<CanonicalMedia, MediaMaterializationIssue> {
    read_local(
        source_root,
        relative_path,
        expected_size,
        expected_sha256,
        kind,
        limits,
        existing_blob,
    )
}

fn read_local(
    source_root: &Path,
    relative_path: &str,
    expected_size: u64,
    expected_sha256: Sha256Digest,
    kind: ImportMediaKind,
    limits: &MediaMaterializationLimits,
    existing_blob: Option<Arc<[u8]>>,
) -> Result<CanonicalMedia, MediaMaterializationIssue> {
    if expected_size > limits.max_blob_bytes {
        return Err(MediaMaterializationIssue::BlobTooLarge);
    }
    let components = safe_relative_components(relative_path)?;
    let mut candidate = source_root.to_owned();
    for component in &components {
        candidate.push(component);
        let metadata = fs::symlink_metadata(&candidate).map_err(map_source_io)?;
        if metadata.file_type().is_symlink() {
            return Err(MediaMaterializationIssue::SymlinkNotAllowed);
        }
    }
    let canonical = fs::canonicalize(&candidate).map_err(map_source_io)?;
    if !canonical.starts_with(source_root) {
        return Err(MediaMaterializationIssue::PathEscape);
    }
    let metadata = fs::metadata(&canonical).map_err(map_source_io)?;
    if !metadata.is_file() {
        return Err(MediaMaterializationIssue::SourceNotRegularFile);
    }
    if metadata.len() != expected_size {
        return Err(MediaMaterializationIssue::SourceChanged);
    }
    let mut file = File::open(&canonical).map_err(map_source_io)?;
    verify_open_file_beneath_root(&file, source_root)?;
    let bytes = read_bounded(&mut file, limits.max_blob_bytes)?;
    if bytes.len() as u64 != expected_size {
        return Err(MediaMaterializationIssue::SourceChanged);
    }
    let sha256 = digest(&bytes);
    if sha256 != expected_sha256 {
        return Err(MediaMaterializationIssue::SourceChanged);
    }
    let final_metadata = file.metadata().map_err(map_source_io)?;
    if !final_metadata.is_file() || final_metadata.len() != expected_size {
        return Err(MediaMaterializationIssue::SourceChanged);
    }
    let filename = components
        .last()
        .cloned()
        .ok_or(MediaMaterializationIssue::PathEscape)?;
    let bytes = existing_blob.unwrap_or_else(|| Arc::from(bytes));
    Ok(CanonicalMedia {
        sha256,
        mime: canonical_mime(kind, &filename, &bytes).to_owned(),
        filename,
        bytes,
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_open_file_beneath_root(
    file: &File,
    source_root: &Path,
) -> Result<(), MediaMaterializationIssue> {
    use std::os::fd::AsRawFd;

    let opened_path =
        fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(map_source_io)?;
    if !opened_path.starts_with(source_root) {
        return Err(MediaMaterializationIssue::PathEscape);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn verify_open_file_beneath_root(
    _file: &File,
    _source_root: &Path,
) -> Result<(), MediaMaterializationIssue> {
    Ok(())
}

fn safe_relative_components(relative_path: &str) -> Result<Vec<String>, MediaMaterializationIssue> {
    let path = Path::new(relative_path);
    if relative_path.is_empty() || path.is_absolute() {
        return Err(MediaMaterializationIssue::PathEscape);
    }
    let mut output = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => output.push(
                value
                    .to_str()
                    .ok_or(MediaMaterializationIssue::PathEscape)?
                    .to_owned(),
            ),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(MediaMaterializationIssue::PathEscape);
            }
        }
    }
    if output.is_empty() {
        return Err(MediaMaterializationIssue::PathEscape);
    }
    Ok(output)
}

fn map_source_io(error: io::Error) -> MediaMaterializationIssue {
    match error.kind() {
        io::ErrorKind::NotFound => MediaMaterializationIssue::SourceNotFound,
        io::ErrorKind::PermissionDenied => MediaMaterializationIssue::SourceUnreadable,
        _ => MediaMaterializationIssue::SourceUnreadable,
    }
}

fn read_bounded(
    reader: &mut impl Read,
    maximum: u64,
) -> Result<Vec<u8>, MediaMaterializationIssue> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(maximum.saturating_add(1));
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = limited.read(&mut buffer).map_err(map_source_io)?;
        if read == 0 {
            break;
        }
        bytes
            .try_reserve(read)
            .map_err(|_| MediaMaterializationIssue::BlobTooLarge)?;
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() as u64 > maximum {
            return Err(MediaMaterializationIssue::BlobTooLarge);
        }
    }
    Ok(bytes)
}

fn materialize_inline(
    reference: &ImportMediaReference,
    expected_mime: ImportInlineImageMime,
    expected_size: u64,
    expected_sha256: Sha256Digest,
    limits: &MediaMaterializationLimits,
    existing_blob: Option<Arc<[u8]>>,
) -> Result<CanonicalMedia, MediaMaterializationIssue> {
    if expected_size > limits.max_blob_bytes {
        return Err(MediaMaterializationIssue::BlobTooLarge);
    }
    let destination = direct_destination_spelling(reference)
        .ok_or(MediaMaterializationIssue::InvalidInlineData)?;
    let data_url =
        DataUrl::process(destination).map_err(|_| MediaMaterializationIssue::InvalidInlineData)?;
    let actual_mime = if data_url.mime_type().matches("image", "png") {
        ImportInlineImageMime::Png
    } else if data_url.mime_type().matches("image", "jpeg") {
        ImportInlineImageMime::Jpeg
    } else {
        return Err(MediaMaterializationIssue::InvalidInlineData);
    };
    if actual_mime != expected_mime {
        return Err(MediaMaterializationIssue::SourceChanged);
    }
    let mut bytes = Vec::new();
    let fragment = data_url
        .decode(|chunk| {
            let size = (bytes.len() as u64)
                .checked_add(chunk.len() as u64)
                .ok_or(())?;
            if size > limits.max_blob_bytes {
                return Err(());
            }
            bytes.try_reserve(chunk.len()).map_err(|_| ())?;
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .map_err(|error| match error {
            DecodeError::InvalidBase64(_) => MediaMaterializationIssue::InvalidInlineData,
            DecodeError::WriteError(()) => MediaMaterializationIssue::BlobTooLarge,
        })?;
    if fragment.is_some() {
        return Err(MediaMaterializationIssue::InvalidInlineData);
    }
    let sha256 = digest(&bytes);
    if bytes.len() as u64 != expected_size || sha256 != expected_sha256 {
        return Err(MediaMaterializationIssue::SourceChanged);
    }
    let (extension, mime) = match expected_mime {
        ImportInlineImageMime::Png => ("png", "image/png"),
        ImportInlineImageMime::Jpeg => ("jpg", "image/jpeg"),
    };
    let bytes = existing_blob.unwrap_or_else(|| Arc::from(bytes));
    Ok(CanonicalMedia {
        sha256,
        filename: format!("inline-{}.{}", &sha256.to_hex()[..16], extension),
        mime: mime.to_owned(),
        bytes,
    })
}

fn direct_destination_spelling(reference: &ImportMediaReference) -> Option<&str> {
    let range = reference.owner_markdown_destination_range?;
    let start = range
        .start_byte
        .checked_sub(reference.owner_markdown_range.start_byte)?;
    let end = range
        .end_byte
        .checked_sub(reference.owner_markdown_range.start_byte)?;
    reference
        .owner_markdown_spelling
        .get(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
}

fn add_materialized_reference(
    owners: &MarkdownOwnerIndex<'_>,
    reference_index: u64,
    reference: &ImportMediaReference,
    media: CanonicalMedia,
    state: &mut MaterializationState,
) -> Result<(), MediaMaterializationIssue> {
    let is_new_blob = !state.blobs.contains_key(&media.sha256);
    let attachment_uuid = materialized_attachment_uuid(reference.owner, media.sha256);
    let is_new_attachment = !state.attachments.contains_key(&attachment_uuid);
    let (markdown_block_uuid, markdown) = owners.markdown_block(reference.owner)?;
    validate_expected_markdown(markdown, reference)?;
    let attachment_uri = format!("{ATTACHMENT_SCHEME}{attachment_uuid}");
    let replacement_markdown = replacement_markdown(reference, &media.filename, &attachment_uri)
        .ok_or(MediaMaterializationIssue::InvalidPreparedReference)?;

    if is_new_blob {
        state.total_blob_bytes = state
            .total_blob_bytes
            .checked_add(media.size_bytes())
            .ok_or(MediaMaterializationIssue::TotalBlobBytesExceeded)?;
        state.blobs.insert(
            media.sha256,
            MaterializedMediaBlob {
                sha256: media.sha256,
                size_bytes: media.size_bytes(),
                bytes: Arc::clone(&media.bytes),
            },
        );
    }
    if is_new_attachment {
        let size_bytes = media.size_bytes();
        state.attachments.insert(
            attachment_uuid,
            MaterializedMediaAttachment {
                attachment_uuid,
                owner: reference.owner,
                sha256: media.sha256,
                filename: media.filename,
                mime: media.mime,
                size_bytes,
            },
        );
    }
    state.rewrites.push(MediaRewrite {
        reference_index,
        attachment_uuid,
        attachment_uri,
        attachment_owner: reference.owner,
        markdown_block_uuid,
        markdown_range: reference.owner_markdown_range,
        replacement_markdown,
    });
    Ok(())
}

fn validate_expected_markdown(
    markdown: &str,
    reference: &ImportMediaReference,
) -> Result<(), MediaMaterializationIssue> {
    let start = usize::try_from(reference.owner_markdown_range.start_byte)
        .map_err(|_| MediaMaterializationIssue::InvalidPreparedReference)?;
    let end = usize::try_from(reference.owner_markdown_range.end_byte)
        .map_err(|_| MediaMaterializationIssue::InvalidPreparedReference)?;
    if start > end || markdown.get(start..end) != Some(reference.owner_markdown_spelling.as_str()) {
        return Err(MediaMaterializationIssue::InvalidPreparedReference);
    }
    Ok(())
}

fn replacement_markdown(
    reference: &ImportMediaReference,
    filename: &str,
    attachment_uri: &str,
) -> Option<String> {
    let alt = match reference.kind {
        ImportMediaKind::MarkdownImage => {
            if let Some(rewritten) = rewrite_direct_image_destination(reference, attachment_uri) {
                return Some(rewritten);
            }
            let title = canonical_title_suffix(&reference.title);
            return Some(format!(
                "![{}]({attachment_uri}{title})",
                raw_image_alt(&reference.owner_markdown_spelling)?
            ));
        }
        ImportMediaKind::LegacyExcalidraw => {
            escape_markdown_label(filename.strip_suffix(".excalidraw").unwrap_or(filename))
        }
    };
    Some(format!("![{alt}]({attachment_uri})"))
}

fn rewrite_direct_image_destination(
    reference: &ImportMediaReference,
    attachment_uri: &str,
) -> Option<String> {
    let range = reference.owner_markdown_destination_range?;
    let start = usize::try_from(
        range
            .start_byte
            .checked_sub(reference.owner_markdown_range.start_byte)?,
    )
    .ok()?;
    let end = usize::try_from(
        range
            .end_byte
            .checked_sub(reference.owner_markdown_range.start_byte)?,
    )
    .ok()?;
    reference.owner_markdown_spelling.get(start..end)?;
    Some(replace_range(
        &reference.owner_markdown_spelling,
        start,
        end,
        attachment_uri,
    ))
}

fn canonical_title_suffix(title: &str) -> String {
    if title.is_empty() {
        return String::new();
    }
    let mut output = String::with_capacity(title.len() + 3);
    output.push_str(" \"");
    for character in title.chars() {
        if matches!(character, '\\' | '"') {
            output.push('\\');
        }
        output.push(character);
    }
    output.push('"');
    output
}

fn replace_range(markdown: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut output = String::with_capacity(markdown.len() - (end - start) + replacement.len());
    output.push_str(&markdown[..start]);
    output.push_str(replacement);
    output.push_str(&markdown[end..]);
    output
}

fn raw_image_alt(markdown: &str) -> Option<String> {
    let end = image_label_end(markdown)?;
    markdown.get(2..end).map(str::to_owned)
}

fn image_label_end(markdown: &str) -> Option<usize> {
    let bytes = markdown.as_bytes();
    if bytes.get(..2) != Some(b"![") {
        return None;
    }
    let mut escaped = false;
    let mut depth = 0_u64;
    for (index, byte) in bytes.iter().copied().enumerate().skip(2) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'[' => depth += 1,
            b']' if depth == 0 => return Some(index),
            b']' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn escape_markdown_label(label: &str) -> String {
    let mut output = String::with_capacity(label.len());
    for character in label.chars() {
        if matches!(character, '\\' | '[' | ']') {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

/// Derives the attachment identity used by the domain layer from import-native
/// owner and SHA-256 types. Keep this covered by a notes-core contract test.
#[must_use]
pub fn materialized_attachment_uuid(owner: ImportMediaOwner, sha256: Sha256Digest) -> Uuid {
    let (kind, uuid) = match owner {
        ImportMediaOwner::Page { page_uuid } => ("page", page_uuid),
        ImportMediaOwner::Block { block_uuid } => ("block", block_uuid),
    };
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("notes-rs:attachment:{kind}:{uuid}:{sha256}").as_bytes(),
    )
}

fn canonical_mime(kind: ImportMediaKind, filename: &str, bytes: &[u8]) -> &'static str {
    if kind == ImportMediaKind::LegacyExcalidraw {
        return "application/vnd.excalidraw+json";
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png";
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return "image/jpeg";
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return "image/webp";
    }
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("svg") {
        "image/svg+xml"
    } else if extension.eq_ignore_ascii_case("avif") {
        "image/avif"
    } else {
        "application/octet-stream"
    }
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}

fn preserve(
    state: &mut MaterializationState,
    reference_index: u64,
    reference: &ImportMediaReference,
    reason: PreservedMediaReason,
) {
    state.preserved_references.push(PreservedMediaReference {
        reference_index,
        owner: reference.owner,
        reason,
    });
}

fn issue_message(issue: MediaMaterializationIssue) -> &'static str {
    match issue {
        MediaMaterializationIssue::InvalidPreparedReference => {
            "prepared media reference no longer matches its owner Markdown"
        }
        MediaMaterializationIssue::PathEscape => {
            "classified media path did not remain inside the selected graph"
        }
        MediaMaterializationIssue::SymlinkNotAllowed => {
            "symbolic links are not allowed while materializing graph media"
        }
        MediaMaterializationIssue::SourceNotFound => {
            "classified media disappeared before materialization"
        }
        MediaMaterializationIssue::SourceNotRegularFile => {
            "classified media is no longer a regular file"
        }
        MediaMaterializationIssue::SourceUnreadable => {
            "classified media could not be read during materialization"
        }
        MediaMaterializationIssue::SourceChanged => {
            "classified media bytes no longer match the prepared manifest"
        }
        MediaMaterializationIssue::BlobTooLarge => {
            "classified media exceeds the materialization per-blob limit"
        }
        MediaMaterializationIssue::TotalBlobBytesExceeded => {
            "materialized media exceeds the total byte limit"
        }
        MediaMaterializationIssue::BlobLimitExceeded => {
            "materialized media exceeds the unique blob limit"
        }
        MediaMaterializationIssue::AttachmentLimitExceeded => {
            "materialized media exceeds the attachment limit"
        }
        MediaMaterializationIssue::RewriteLimitExceeded => {
            "materialized media exceeds the rewrite limit"
        }
        MediaMaterializationIssue::InvalidInlineData => {
            "inline media no longer matches the validated data URL form"
        }
        MediaMaterializationIssue::ConflictingRewrite => {
            "media rewrite overlaps another rewrite in the same Markdown block"
        }
        MediaMaterializationIssue::DeferredExcalidrawConversion => {
            "legacy Excalidraw reference was preserved until image conversion is available"
        }
    }
}

fn validate_non_overlapping_rewrites(rewrites: &[MediaRewrite]) -> Result<(), u64> {
    let mut by_block = BTreeMap::<Uuid, Vec<&MediaRewrite>>::new();
    for rewrite in rewrites {
        by_block
            .entry(rewrite.markdown_block_uuid)
            .or_default()
            .push(rewrite);
    }
    for rewrites in by_block.values_mut() {
        rewrites.sort_by_key(|rewrite| rewrite.markdown_range.start_byte);
        for pair in rewrites.windows(2) {
            if pair[0].markdown_range.end_byte > pair[1].markdown_range.start_byte {
                return Err(pair[0].reference_index.min(pair[1].reference_index));
            }
        }
    }
    Ok(())
}

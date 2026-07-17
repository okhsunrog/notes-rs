use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

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
    pub bytes: Vec<u8>,
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
/// ranges. `expected_markdown` must still be compared immediately before the
/// replacement is applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRewrite {
    pub reference_index: u64,
    pub attachment_uuid: Uuid,
    pub attachment_uri: String,
    pub attachment_owner: ImportMediaOwner,
    pub markdown_block_uuid: Uuid,
    pub markdown_range: ImportMarkdownRange,
    pub expected_markdown: String,
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
    MaterializationFailed,
}

/// Exact source spellings retained for every reference that was not rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreservedMediaReference {
    pub reference_index: u64,
    pub owner: ImportMediaOwner,
    pub raw_spelling: String,
    pub owner_markdown_spelling: String,
    pub resolution: ImportMediaResolution,
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
            Self::Io { .. } => MaterializeMediaErrorCode::Io,
        }
    }
}

#[derive(Debug, Clone)]
struct CanonicalMedia {
    sha256: Sha256Digest,
    bytes: Vec<u8>,
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
    local_cache: BTreeMap<LocalCacheKey, Result<CanonicalMedia, MediaMaterializationIssue>>,
    total_blob_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LocalCacheKey {
    relative_path: String,
    expected_size: u64,
    expected_sha256: Sha256Digest,
    kind: ImportMediaKind,
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
    let mut state = MaterializationState::default();

    for (index, reference) in prepared.media_references.iter().enumerate() {
        let reference_index = index as u64;
        let result = match &reference.resolution {
            ImportMediaResolution::LocalManifest {
                relative_path,
                size_bytes,
                sha256,
            } => materialize_local(
                &source_root,
                relative_path,
                *size_bytes,
                *sha256,
                reference.kind,
                limits,
                &mut state.local_cache,
            ),
            ImportMediaResolution::InlineData {
                mime,
                decoded_size_bytes,
                decoded_sha256,
            } => materialize_inline(
                reference,
                *mime,
                *decoded_size_bytes,
                *decoded_sha256,
                limits,
            ),
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

        let media = match result {
            Ok(media) => media,
            Err(issue) => {
                reject(&mut state, reference_index, reference, issue);
                continue;
            }
        };
        if let Err(issue) = add_materialized_reference(
            prepared,
            reference_index,
            reference,
            media,
            limits,
            &mut state,
        ) {
            reject(&mut state, reference_index, reference, issue);
        }
    }

    remove_conflicting_rewrites(prepared, &mut state);
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
    cache: &mut BTreeMap<LocalCacheKey, Result<CanonicalMedia, MediaMaterializationIssue>>,
) -> Result<CanonicalMedia, MediaMaterializationIssue> {
    let key = LocalCacheKey {
        relative_path: relative_path.to_owned(),
        expected_size,
        expected_sha256,
        kind,
    };
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }
    let result = read_local(
        source_root,
        relative_path,
        expected_size,
        expected_sha256,
        kind,
        limits,
    );
    cache.insert(key, result.clone());
    result
}

fn read_local(
    source_root: &Path,
    relative_path: &str,
    expected_size: u64,
    expected_sha256: Sha256Digest,
    kind: ImportMediaKind,
    limits: &MediaMaterializationLimits,
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
    Ok(CanonicalMedia {
        sha256,
        mime: canonical_mime(kind, &filename, &bytes).to_owned(),
        filename,
        bytes,
    })
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
) -> Result<CanonicalMedia, MediaMaterializationIssue> {
    if expected_size > limits.max_blob_bytes {
        return Err(MediaMaterializationIssue::BlobTooLarge);
    }
    let destination = inline_destination(&reference.owner_markdown_spelling)
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
    Ok(CanonicalMedia {
        sha256,
        filename: format!("inline-{}.{}", &sha256.to_hex()[..16], extension),
        mime: mime.to_owned(),
        bytes,
    })
}

fn inline_destination(markdown: &str) -> Option<&str> {
    let open = markdown.find("](")?.checked_add(2)?;
    let tail = markdown.get(open..)?;
    if let Some(tail) = tail.strip_prefix('<') {
        let end = tail.find('>')?;
        return tail.get(..end);
    }
    let end = tail
        .char_indices()
        .find_map(|(index, character)| character.is_ascii_whitespace().then_some(index))
        .unwrap_or_else(|| tail.find(')').unwrap_or(tail.len()));
    tail.get(..end)
}

fn add_materialized_reference(
    prepared: &PreparedImport,
    reference_index: u64,
    reference: &ImportMediaReference,
    media: CanonicalMedia,
    limits: &MediaMaterializationLimits,
    state: &mut MaterializationState,
) -> Result<(), MediaMaterializationIssue> {
    let is_new_blob = !state.blobs.contains_key(&media.sha256);
    let next_total = if is_new_blob {
        if state.blobs.len() as u64 >= limits.max_blobs {
            return Err(MediaMaterializationIssue::BlobLimitExceeded);
        }
        let next_total = state
            .total_blob_bytes
            .checked_add(media.size_bytes())
            .ok_or(MediaMaterializationIssue::TotalBlobBytesExceeded)?;
        if next_total > limits.max_total_blob_bytes {
            return Err(MediaMaterializationIssue::TotalBlobBytesExceeded);
        }
        Some(next_total)
    } else {
        None
    };

    let attachment_uuid = materialized_attachment_uuid(reference.owner, media.sha256);
    let is_new_attachment = !state.attachments.contains_key(&attachment_uuid);
    if is_new_attachment && state.attachments.len() as u64 >= limits.max_attachments {
        return Err(MediaMaterializationIssue::AttachmentLimitExceeded);
    }
    if state.rewrites.len() as u64 >= limits.max_rewrites {
        return Err(MediaMaterializationIssue::RewriteLimitExceeded);
    }
    let markdown_block_uuid = markdown_block_uuid(prepared, reference)
        .ok_or(MediaMaterializationIssue::InvalidPreparedReference)?;
    validate_expected_markdown(prepared, markdown_block_uuid, reference)?;
    let attachment_uri = format!("{ATTACHMENT_SCHEME}{attachment_uuid}");
    let replacement_markdown = replacement_markdown(reference, &media.filename, &attachment_uri)
        .ok_or(MediaMaterializationIssue::InvalidPreparedReference)?;

    if is_new_blob {
        state.total_blob_bytes = next_total.expect("new blob has a checked total");
        state.blobs.insert(
            media.sha256,
            MaterializedMediaBlob {
                sha256: media.sha256,
                size_bytes: media.size_bytes(),
                bytes: media.bytes.clone(),
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
        expected_markdown: reference.owner_markdown_spelling.clone(),
        replacement_markdown,
    });
    Ok(())
}

fn markdown_block_uuid(
    prepared: &PreparedImport,
    reference: &ImportMediaReference,
) -> Option<Uuid> {
    match reference.owner {
        ImportMediaOwner::Block { block_uuid } => prepared
            .pages
            .iter()
            .flat_map(|page| &page.blocks)
            .any(|block| block.uuid == block_uuid)
            .then_some(block_uuid),
        ImportMediaOwner::Page { page_uuid } => prepared
            .pages
            .iter()
            .find(|page| page.uuid == page_uuid)?
            .blocks
            .iter()
            .find(|block| matches!(block.provenance.source, ImportBlockSource::Preamble))
            .map(|block| block.uuid),
    }
}

fn validate_expected_markdown(
    prepared: &PreparedImport,
    block_uuid: Uuid,
    reference: &ImportMediaReference,
) -> Result<(), MediaMaterializationIssue> {
    let markdown = prepared
        .pages
        .iter()
        .flat_map(|page| &page.blocks)
        .find(|block| block.uuid == block_uuid)
        .map(|block| block.markdown.as_str())
        .ok_or(MediaMaterializationIssue::InvalidPreparedReference)?;
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
            if let Some(rewritten) =
                rewrite_inline_image_destination(&reference.owner_markdown_spelling, attachment_uri)
            {
                return Some(rewritten);
            }
            return Some(format!(
                "![{}]({attachment_uri})",
                raw_image_alt(&reference.owner_markdown_spelling)?
            ));
        }
        ImportMediaKind::LegacyExcalidraw => {
            escape_markdown_label(filename.strip_suffix(".excalidraw").unwrap_or(filename))
        }
    };
    Some(format!("![{alt}]({attachment_uri})"))
}

fn rewrite_inline_image_destination(markdown: &str, attachment_uri: &str) -> Option<String> {
    let label_end = image_label_end(markdown)?;
    let bytes = markdown.as_bytes();
    let mut cursor = label_end.checked_add(1)?;
    if bytes.get(cursor) != Some(&b'(') {
        return None;
    }
    cursor += 1;
    while bytes
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        cursor += 1;
    }
    let destination_start = cursor;
    let destination_end = if bytes.get(cursor) == Some(&b'<') {
        cursor += 1;
        let start = cursor;
        let end = bytes[start..]
            .iter()
            .position(|byte| *byte == b'>')?
            .checked_add(start)?;
        return Some(replace_range(markdown, start, end, attachment_uri));
    } else {
        let mut depth = 0_u64;
        let mut escaped = false;
        loop {
            let byte = *bytes.get(cursor)?;
            if escaped {
                escaped = false;
                cursor += 1;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'(' => depth = depth.checked_add(1)?,
                b')' if depth == 0 => break cursor,
                b')' => depth -= 1,
                byte if byte.is_ascii_whitespace() && depth == 0 => break cursor,
                _ => {}
            }
            cursor += 1;
        }
    };
    (destination_start < destination_end)
        .then(|| replace_range(markdown, destination_start, destination_end, attachment_uri))
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

fn materialized_attachment_uuid(owner: ImportMediaOwner, sha256: Sha256Digest) -> Uuid {
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

fn reject(
    state: &mut MaterializationState,
    reference_index: u64,
    reference: &ImportMediaReference,
    issue: MediaMaterializationIssue,
) {
    state.diagnostics.push(MediaMaterializationDiagnostic {
        reference_index,
        source_document_path: reference.relative_path.clone(),
        source_range: reference.source_range,
        issue,
        message: issue_message(issue),
    });
    preserve(
        state,
        reference_index,
        reference,
        PreservedMediaReason::MaterializationFailed,
    );
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
        raw_spelling: reference.raw_spelling.clone(),
        owner_markdown_spelling: reference.owner_markdown_spelling.clone(),
        resolution: reference.resolution.clone(),
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
    }
}

fn remove_conflicting_rewrites(prepared: &PreparedImport, state: &mut MaterializationState) {
    let mut conflicts = BTreeSet::new();
    let mut by_block = BTreeMap::<Uuid, Vec<&MediaRewrite>>::new();
    for rewrite in &state.rewrites {
        by_block
            .entry(rewrite.markdown_block_uuid)
            .or_default()
            .push(rewrite);
    }
    for rewrites in by_block.values_mut() {
        rewrites.sort_by_key(|rewrite| rewrite.markdown_range.start_byte);
        for pair in rewrites.windows(2) {
            if pair[0].markdown_range.end_byte > pair[1].markdown_range.start_byte {
                conflicts.insert(pair[0].reference_index);
                conflicts.insert(pair[1].reference_index);
            }
        }
    }
    if conflicts.is_empty() {
        return;
    }
    state
        .rewrites
        .retain(|rewrite| !conflicts.contains(&rewrite.reference_index));
    for reference_index in conflicts {
        if let Some(reference) = prepared.media_references.get(reference_index as usize) {
            reject(
                state,
                reference_index,
                reference,
                MediaMaterializationIssue::ConflictingRewrite,
            );
        }
    }
    remove_unreferenced_materialized_entries(state);
}

fn remove_unreferenced_materialized_entries(state: &mut MaterializationState) {
    let attachment_uuids = state
        .rewrites
        .iter()
        .map(|rewrite| rewrite.attachment_uuid)
        .collect::<BTreeSet<_>>();
    state
        .attachments
        .retain(|uuid, _| attachment_uuids.contains(uuid));
    let hashes = state
        .attachments
        .values()
        .map(|attachment| attachment.sha256)
        .collect::<BTreeSet<_>>();
    state.blobs.retain(|hash, _| hashes.contains(hash));
}

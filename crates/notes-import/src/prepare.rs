use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::media::{MediaCollectionError, MediaLimits, MediaOwnerInput, collect_media};
use crate::prepared::{
    IMPORT_PLANNER_VERSION, IdentityContext, ImportBlock, ImportBlockIdentity, ImportBlockMapping,
    ImportBlockProvenance, ImportBlockSource, ImportIdentityMaps, ImportMediaKind,
    ImportMediaOwner, ImportMediaReference, ImportMediaResolution, ImportPage, ImportPageKind,
    ImportPageProvenance, ImportPageSource, ImportProvenance, ImportReference, ImportReferenceKind,
    ImportReferenceOwner, ImportReferenceResolution, ImportReferenceTargetKind,
    ImportReferenceUnresolvedReason, ImportReport, ImportTaskMapping, ImportTaskState,
    LogseqTaskMarker, PreparedImport,
};
use crate::{
    DiagnosticCode, DiagnosticSeverity, DocumentFormat, GraphManifest, ImportDiagnostic,
    LogseqConstructKind, LogseqConstructOwner, LogseqDocumentSource, LogseqMarkdownSourceLine,
    LogseqSourceBlock, ParsedLogseqDocument, Sha256Digest, SourceKind, SourcePosition, SourceRange,
};

const BLOCK_ID_DOMAIN: &[u8] = b"notes-rs/import/logseq/block/v1\0";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareLimits {
    pub max_total_document_bytes: u64,
    pub max_blocks: u64,
    pub max_blocks_per_document: u64,
    pub max_references: u64,
    pub max_references_per_document: u64,
    pub max_reference_bytes: u64,
    pub max_media_references: u64,
    pub max_media_references_per_document: u64,
    pub max_media_reference_bytes: u64,
    pub max_inline_media_bytes: u64,
    pub max_inline_image_width: u32,
    pub max_inline_image_height: u32,
    pub max_inline_image_pixels: u64,
    pub max_diagnostics: u64,
    pub max_nesting_depth: u64,
}

impl Default for PrepareLimits {
    fn default() -> Self {
        Self {
            max_total_document_bytes: 1024 * 1024 * 1024,
            max_blocks: 2_000_000,
            max_blocks_per_document: 1_000_000,
            max_references: 2_000_000,
            max_references_per_document: 1_000_000,
            max_reference_bytes: 64 * 1024,
            max_media_references: 2_000_000,
            max_media_references_per_document: 1_000_000,
            max_media_reference_bytes: 48 * 1024 * 1024,
            max_inline_media_bytes: 32 * 1024 * 1024,
            max_inline_image_width: 8_192,
            max_inline_image_height: 8_192,
            max_inline_image_pixels: 40_000_000,
            max_diagnostics: 2_000_000,
            max_nesting_depth: 1_024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrepareImportErrorCode {
    NilWorkspaceUuid,
    NilImportNamespaceUuid,
    DuplicateManifestDocument,
    MissingParsedDocument,
    DuplicateParsedDocument,
    UnexpectedParsedDocument,
    SourceManifestMismatch,
    SourceKindMismatch,
    InvalidSourceAst,
    ReferenceLimitExceeded,
    TotalReferenceLimitExceeded,
    ReferenceTooLong,
    MediaReferenceLimitExceeded,
    TotalMediaReferenceLimitExceeded,
    MediaReferenceTooLong,
    TotalDocumentBytesLimitExceeded,
    BlockLimitExceeded,
    DocumentBlockLimitExceeded,
    NestingDepthExceeded,
    DiagnosticLimitExceeded,
    PlanSerialization,
}

#[derive(Debug, Error)]
pub enum PrepareImportError {
    #[error("workspace UUID must not be nil")]
    NilWorkspaceUuid,
    #[error("import namespace UUID must not be nil")]
    NilImportNamespaceUuid,
    #[error("manifest contains a duplicate supported document path: {relative_path}")]
    DuplicateManifestDocument { relative_path: String },
    #[error("parsed document is missing for manifest entry: {relative_path}")]
    MissingParsedDocument { relative_path: String },
    #[error("parsed documents contain a duplicate path: {relative_path}")]
    DuplicateParsedDocument { relative_path: String },
    #[error(
        "parsed document is not present as supported Markdown in the manifest: {relative_path}"
    )]
    UnexpectedParsedDocument { relative_path: String },
    #[error("parsed source bytes do not match the manifest: {relative_path}")]
    SourceManifestMismatch { relative_path: String },
    #[error("parsed source kind does not match the manifest: {relative_path}")]
    SourceKindMismatch { relative_path: String },
    #[error("parsed source AST is internally inconsistent: {relative_path}")]
    InvalidSourceAst { relative_path: String },
    #[error("source document exceeds the {limit} reference safety limit: {relative_path}")]
    ReferenceLimitExceeded { relative_path: String, limit: u64 },
    #[error("source reference exceeds the {limit_bytes}-byte safety limit: {relative_path}")]
    ReferenceTooLong {
        relative_path: String,
        limit_bytes: u64,
    },
    #[error("source document exceeds the {limit} media reference safety limit: {relative_path}")]
    MediaReferenceLimitExceeded { relative_path: String, limit: u64 },
    #[error("import exceeds the {limit} media reference preparation safety limit")]
    TotalMediaReferenceLimitExceeded { limit: u64 },
    #[error("source media reference exceeds the {limit_bytes}-byte safety limit: {relative_path}")]
    MediaReferenceTooLong {
        relative_path: String,
        limit_bytes: u64,
    },
    #[error("parsed documents exceed the {limit_bytes}-byte preparation safety limit")]
    TotalDocumentBytesLimitExceeded { limit_bytes: u64 },
    #[error("import exceeds the {limit} block preparation safety limit")]
    BlockLimitExceeded { limit: u64 },
    #[error("source document exceeds the {limit} block safety limit: {relative_path}")]
    DocumentBlockLimitExceeded { relative_path: String, limit: u64 },
    #[error("source document exceeds the nesting depth safety limit of {limit}: {relative_path}")]
    NestingDepthExceeded { relative_path: String, limit: u64 },
    #[error("import exceeds the {limit} reference preparation safety limit")]
    TotalReferenceLimitExceeded { limit: u64 },
    #[error("import exceeds the {limit} diagnostic preparation safety limit")]
    DiagnosticLimitExceeded { limit: u64 },
    #[error("deterministic import plan could not be serialized")]
    PlanSerialization(#[source] serde_json::Error),
}

impl PrepareImportError {
    pub const fn code(&self) -> PrepareImportErrorCode {
        match self {
            Self::NilWorkspaceUuid => PrepareImportErrorCode::NilWorkspaceUuid,
            Self::NilImportNamespaceUuid => PrepareImportErrorCode::NilImportNamespaceUuid,
            Self::DuplicateManifestDocument { .. } => {
                PrepareImportErrorCode::DuplicateManifestDocument
            }
            Self::MissingParsedDocument { .. } => PrepareImportErrorCode::MissingParsedDocument,
            Self::DuplicateParsedDocument { .. } => PrepareImportErrorCode::DuplicateParsedDocument,
            Self::UnexpectedParsedDocument { .. } => {
                PrepareImportErrorCode::UnexpectedParsedDocument
            }
            Self::SourceManifestMismatch { .. } => PrepareImportErrorCode::SourceManifestMismatch,
            Self::SourceKindMismatch { .. } => PrepareImportErrorCode::SourceKindMismatch,
            Self::InvalidSourceAst { .. } => PrepareImportErrorCode::InvalidSourceAst,
            Self::ReferenceLimitExceeded { .. } => PrepareImportErrorCode::ReferenceLimitExceeded,
            Self::TotalReferenceLimitExceeded { .. } => {
                PrepareImportErrorCode::TotalReferenceLimitExceeded
            }
            Self::ReferenceTooLong { .. } => PrepareImportErrorCode::ReferenceTooLong,
            Self::MediaReferenceLimitExceeded { .. } => {
                PrepareImportErrorCode::MediaReferenceLimitExceeded
            }
            Self::TotalMediaReferenceLimitExceeded { .. } => {
                PrepareImportErrorCode::TotalMediaReferenceLimitExceeded
            }
            Self::MediaReferenceTooLong { .. } => PrepareImportErrorCode::MediaReferenceTooLong,
            Self::TotalDocumentBytesLimitExceeded { .. } => {
                PrepareImportErrorCode::TotalDocumentBytesLimitExceeded
            }
            Self::BlockLimitExceeded { .. } => PrepareImportErrorCode::BlockLimitExceeded,
            Self::DocumentBlockLimitExceeded { .. } => {
                PrepareImportErrorCode::DocumentBlockLimitExceeded
            }
            Self::NestingDepthExceeded { .. } => PrepareImportErrorCode::NestingDepthExceeded,
            Self::DiagnosticLimitExceeded { .. } => PrepareImportErrorCode::DiagnosticLimitExceeded,
            Self::PlanSerialization(_) => PrepareImportErrorCode::PlanSerialization,
        }
    }
}

#[derive(Debug)]
struct PageWork<'document> {
    document: &'document ParsedLogseqDocument,
    manifest_sha256: Sha256Digest,
    uuid: Uuid,
    source_identity: String,
    destination_title_key: String,
    kind: ImportPageKind,
    aliases: Vec<String>,
    blocks: Vec<BlockWork>,
}

#[derive(Debug)]
struct BlockWork {
    derived_uuid: Uuid,
    target_uuid: Uuid,
    parent_index: Option<usize>,
    source: ImportBlockSource,
    sibling_ordinal: u64,
    structural_path: Vec<u64>,
    source_range: SourceRange,
    source_content_sha256: Sha256Digest,
    markdown: String,
    source_lines: Vec<LogseqMarkdownSourceLine>,
    explicit_identity: ExplicitIdentity,
    task: Option<ImportTaskMapping>,
}

#[derive(Debug, Clone)]
enum ExplicitIdentity {
    None,
    Candidate { uuid: Uuid, range: SourceRange },
    Rejected,
}

impl ExplicitIdentity {
    fn candidate_uuid(&self) -> Option<Uuid> {
        match self {
            Self::Candidate { uuid, .. } => Some(*uuid),
            Self::None | Self::Rejected => None,
        }
    }
}

/// Build a deterministic, persistence-independent import plan.
///
/// This function performs no I/O. The caller must scan and parse source files
/// first, then pass the exact immutable manifest associated with those bytes.
pub fn prepare_import(
    parsed_documents: &[ParsedLogseqDocument],
    identity: IdentityContext,
    manifest: &GraphManifest,
) -> Result<PreparedImport, PrepareImportError> {
    prepare_import_with_limits(
        parsed_documents,
        identity,
        manifest,
        &PrepareLimits::default(),
    )
}

/// Same pure planner with caller-supplied resource limits. Hosts should use
/// limits consistent with the preceding scanner boundary.
pub fn prepare_import_with_limits(
    parsed_documents: &[ParsedLogseqDocument],
    identity: IdentityContext,
    manifest: &GraphManifest,
    limits: &PrepareLimits,
) -> Result<PreparedImport, PrepareImportError> {
    validate_identity_context(identity)?;
    let documents = validate_manifest_documents(parsed_documents, manifest)?;
    validate_prepare_limits(&documents, limits)?;
    let manifest_entries = supported_manifest_entries(manifest)?;

    let mut diagnostics = documents
        .iter()
        .flat_map(|document| document.diagnostics.iter().cloned())
        .collect::<Vec<_>>();
    validate_diagnostic_limit(&diagnostics, limits)?;
    let mut pages = Vec::with_capacity(documents.len());

    for document in documents {
        validate_source_ast(document)?;
        let entry = manifest_entries
            .get(&document.relative_path)
            .expect("validated manifest document exists");
        let page = build_page_work(document, entry.sha256, identity, &mut diagnostics)?;
        pages.push(page);
        validate_diagnostic_limit(&diagnostics, limits)?;
    }

    diagnose_page_collisions(&pages, &mut diagnostics);
    validate_diagnostic_limit(&diagnostics, limits)?;
    resolve_block_identities(&mut pages, &mut diagnostics);
    validate_diagnostic_limit(&diagnostics, limits)?;
    let identity_maps = build_identity_maps(&pages, &mut diagnostics);
    validate_diagnostic_limit(&diagnostics, limits)?;
    let prepared_pages = pages.iter().map(finalize_page).collect::<Vec<_>>();
    let media_inputs = build_media_inputs(&pages, &prepared_pages);
    let media = collect_media(
        &media_inputs,
        manifest,
        MediaLimits {
            max_references: limits.max_media_references,
            max_references_per_document: limits.max_media_references_per_document,
            max_reference_bytes: limits.max_media_reference_bytes,
            max_inline_decoded_bytes: limits.max_inline_media_bytes,
            max_inline_image_width: limits.max_inline_image_width,
            max_inline_image_height: limits.max_inline_image_height,
            max_inline_image_pixels: limits.max_inline_image_pixels,
            max_diagnostics: limits.max_diagnostics,
        },
        &mut diagnostics,
    )
    .map_err(map_media_error)?;
    validate_diagnostic_limit(&diagnostics, limits)?;
    let references = collect_references(
        &pages,
        &identity_maps,
        &media.references,
        limits,
        &mut diagnostics,
    )?;
    validate_diagnostic_limit(&diagnostics, limits)?;

    diagnostics.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(
                left.range
                    .map(|range| range.start.byte_offset)
                    .cmp(&right.range.map(|range| range.start.byte_offset)),
            )
            .then(left.code.cmp(&right.code))
    });

    let report = build_report(
        &prepared_pages,
        &references,
        &media.references,
        media.unreferenced_asset_count,
        media.unreferenced_drawing_count,
        &diagnostics,
    );
    let plan_sha256 = hash_prepared_plan(
        identity,
        manifest,
        &prepared_pages,
        &references,
        &media.references,
        &identity_maps,
        &report,
    )?;
    let provenance = build_provenance(identity, manifest, &prepared_pages, plan_sha256);

    Ok(PreparedImport {
        identity,
        pages: prepared_pages,
        references,
        media_references: media.references,
        identity_maps,
        provenance,
        report,
    })
}

fn build_media_inputs<'input, 'document: 'input>(
    pages: &'input [PageWork<'document>],
    prepared_pages: &'input [ImportPage],
) -> Vec<MediaOwnerInput<'input>> {
    let mut inputs = Vec::new();
    for (page, prepared_page) in pages.iter().zip(prepared_pages) {
        debug_assert_eq!(page.blocks.len(), prepared_page.blocks.len());
        for (block, prepared_block) in page.blocks.iter().zip(&prepared_page.blocks) {
            let owner = match &block.source {
                ImportBlockSource::Preamble => ImportMediaOwner::Page {
                    page_uuid: page.uuid,
                },
                ImportBlockSource::Structural { .. } => ImportMediaOwner::Block {
                    block_uuid: block.target_uuid,
                },
            };
            let removed_source_range = match block.explicit_identity {
                ExplicitIdentity::Candidate { range, .. } => Some(range),
                ExplicitIdentity::None | ExplicitIdentity::Rejected => None,
            };
            inputs.push(MediaOwnerInput {
                relative_path: &page.document.relative_path,
                document_source: &page.document.raw_markdown,
                owner,
                original_markdown: &block.markdown,
                final_markdown: &prepared_block.markdown,
                source_lines: &block.source_lines,
                removed_source_range,
            });
        }
    }
    inputs
}

fn map_media_error(error: MediaCollectionError) -> PrepareImportError {
    match error {
        MediaCollectionError::InvalidSourceMapping { relative_path } => {
            PrepareImportError::InvalidSourceAst { relative_path }
        }
        MediaCollectionError::ReferenceLimitExceeded {
            relative_path,
            limit,
        } => PrepareImportError::MediaReferenceLimitExceeded {
            relative_path,
            limit,
        },
        MediaCollectionError::TotalReferenceLimitExceeded { limit } => {
            PrepareImportError::TotalMediaReferenceLimitExceeded { limit }
        }
        MediaCollectionError::ReferenceTooLong {
            relative_path,
            limit_bytes,
        } => PrepareImportError::MediaReferenceTooLong {
            relative_path,
            limit_bytes,
        },
        MediaCollectionError::DiagnosticLimitExceeded { limit } => {
            PrepareImportError::DiagnosticLimitExceeded { limit }
        }
    }
}

fn validate_prepare_limits(
    documents: &[&ParsedLogseqDocument],
    limits: &PrepareLimits,
) -> Result<(), PrepareImportError> {
    let mut total_bytes = 0_u64;
    let mut total_blocks = 0_u64;
    for document in documents {
        let bytes = document.raw_markdown.len() as u64;
        total_bytes = total_bytes.checked_add(bytes).ok_or(
            PrepareImportError::TotalDocumentBytesLimitExceeded {
                limit_bytes: limits.max_total_document_bytes,
            },
        )?;
        if total_bytes > limits.max_total_document_bytes {
            return Err(PrepareImportError::TotalDocumentBytesLimitExceeded {
                limit_bytes: limits.max_total_document_bytes,
            });
        }
        let target_blocks = document.blocks.len() as u64 + u64::from(document.preamble.is_some());
        if target_blocks > limits.max_blocks_per_document {
            return Err(PrepareImportError::DocumentBlockLimitExceeded {
                relative_path: document.relative_path.clone(),
                limit: limits.max_blocks_per_document,
            });
        }
        total_blocks = total_blocks.checked_add(target_blocks).ok_or(
            PrepareImportError::BlockLimitExceeded {
                limit: limits.max_blocks,
            },
        )?;
        if total_blocks > limits.max_blocks {
            return Err(PrepareImportError::BlockLimitExceeded {
                limit: limits.max_blocks,
            });
        }
        if document
            .blocks
            .iter()
            .any(|block| block.depth > limits.max_nesting_depth)
        {
            return Err(PrepareImportError::NestingDepthExceeded {
                relative_path: document.relative_path.clone(),
                limit: limits.max_nesting_depth,
            });
        }
    }
    Ok(())
}

fn validate_diagnostic_limit(
    diagnostics: &[ImportDiagnostic],
    limits: &PrepareLimits,
) -> Result<(), PrepareImportError> {
    if diagnostics.len() as u64 > limits.max_diagnostics {
        return Err(PrepareImportError::DiagnosticLimitExceeded {
            limit: limits.max_diagnostics,
        });
    }
    Ok(())
}

fn validate_identity_context(identity: IdentityContext) -> Result<(), PrepareImportError> {
    if identity.workspace_uuid.is_nil() {
        return Err(PrepareImportError::NilWorkspaceUuid);
    }
    if identity.import_namespace_uuid.is_nil() {
        return Err(PrepareImportError::NilImportNamespaceUuid);
    }
    Ok(())
}

fn supported_manifest_entries(
    manifest: &GraphManifest,
) -> Result<BTreeMap<String, &crate::ManifestEntry>, PrepareImportError> {
    let mut entries = BTreeMap::new();
    for entry in manifest.entries().iter().filter(|entry| {
        matches!(entry.kind, SourceKind::Page | SourceKind::Journal)
            && entry.document_format == Some(DocumentFormat::Markdown)
    }) {
        if entries.insert(entry.relative_path.clone(), entry).is_some() {
            return Err(PrepareImportError::DuplicateManifestDocument {
                relative_path: entry.relative_path.clone(),
            });
        }
    }
    Ok(entries)
}

fn validate_manifest_documents<'document>(
    parsed_documents: &'document [ParsedLogseqDocument],
    manifest: &GraphManifest,
) -> Result<Vec<&'document ParsedLogseqDocument>, PrepareImportError> {
    let entries = supported_manifest_entries(manifest)?;
    let mut documents = BTreeMap::new();
    for document in parsed_documents {
        if !entries.contains_key(&document.relative_path) {
            return Err(PrepareImportError::UnexpectedParsedDocument {
                relative_path: document.relative_path.clone(),
            });
        }
        if documents
            .insert(document.relative_path.clone(), document)
            .is_some()
        {
            return Err(PrepareImportError::DuplicateParsedDocument {
                relative_path: document.relative_path.clone(),
            });
        }
    }

    for (path, entry) in &entries {
        let document =
            documents
                .get(path)
                .ok_or_else(|| PrepareImportError::MissingParsedDocument {
                    relative_path: path.clone(),
                })?;
        let bytes = document.raw_markdown.as_bytes();
        if bytes.len() as u64 != entry.size_bytes
            || Sha256Digest::from_bytes(Sha256::digest(bytes).into()) != entry.sha256
        {
            return Err(PrepareImportError::SourceManifestMismatch {
                relative_path: path.clone(),
            });
        }
        let source_matches = matches!(
            (entry.kind, &document.source),
            (SourceKind::Page, LogseqDocumentSource::Page { .. })
                | (SourceKind::Journal, LogseqDocumentSource::Journal { .. })
        );
        if !source_matches {
            return Err(PrepareImportError::SourceKindMismatch {
                relative_path: path.clone(),
            });
        }
    }

    Ok(documents.into_values().collect())
}

fn validate_source_ast(document: &ParsedLogseqDocument) -> Result<(), PrepareImportError> {
    let invalid = || PrepareImportError::InvalidSourceAst {
        relative_path: document.relative_path.clone(),
    };
    let mut index_to_position = BTreeMap::new();
    let mut sibling_ordinals = BTreeSet::new();
    let mut sibling_sequences = BTreeMap::<Option<u64>, Vec<u64>>::new();

    if let Some(preamble) = &document.preamble
        && (!range_is_valid(preamble.source_range, &document.raw_markdown)
            || !validate_markdown_source_lines(
                &preamble.markdown,
                &preamble.source_lines,
                preamble.source_range,
                &document.raw_markdown,
            ))
    {
        return Err(invalid());
    }

    for (position, block) in document.blocks.iter().enumerate() {
        if index_to_position.insert(block.index, position).is_some()
            || !range_is_valid(block.source_range, &document.raw_markdown)
            || !range_is_valid(block.content_range, &document.raw_markdown)
            || block.content_range.start.byte_offset < block.source_range.start.byte_offset
            || block.content_range.end.byte_offset > block.source_range.end.byte_offset
            || !validate_markdown_source_lines(
                &block.markdown,
                &block.source_lines,
                block.source_range,
                &document.raw_markdown,
            )
            || !sibling_ordinals.insert((block.parent_index, block.sibling_index))
        {
            return Err(invalid());
        }
        sibling_sequences
            .entry(block.parent_index)
            .or_default()
            .push(block.sibling_index);
        match block.parent_index {
            Some(parent_index) => {
                let parent_position = *index_to_position.get(&parent_index).ok_or_else(invalid)?;
                if parent_position >= position
                    || document.blocks[parent_position].depth.checked_add(1) != Some(block.depth)
                {
                    return Err(invalid());
                }
            }
            None if block.depth == 0 => {}
            None => return Err(invalid()),
        }
    }

    for mut ordinals in sibling_sequences.into_values() {
        ordinals.sort_unstable();
        if ordinals
            .iter()
            .enumerate()
            .any(|(expected, actual)| u64::try_from(expected).ok() != Some(*actual))
        {
            return Err(invalid());
        }
    }

    for construct in &document.constructs {
        if !range_is_valid(construct.source_range, &document.raw_markdown) {
            return Err(invalid());
        }
        if let LogseqConstructOwner::Block { index } = construct.owner {
            let Some(position) = index_to_position.get(&index) else {
                return Err(invalid());
            };
            let owner_range = document.blocks[*position].source_range;
            if construct.source_range.start.byte_offset < owner_range.start.byte_offset
                || construct.source_range.end.byte_offset > owner_range.end.byte_offset
            {
                return Err(invalid());
            }
        }
        if let LogseqConstructKind::Property { value_range, .. } = &construct.kind
            && (!range_is_valid(*value_range, &document.raw_markdown)
                || value_range.start.byte_offset < construct.source_range.start.byte_offset
                || value_range.end.byte_offset > construct.source_range.end.byte_offset)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn validate_markdown_source_lines(
    markdown: &str,
    lines: &[LogseqMarkdownSourceLine],
    owner_range: SourceRange,
    source: &str,
) -> bool {
    if lines.is_empty() {
        return false;
    }
    let mut expected_start = 0_u64;
    for line in lines {
        let Ok(markdown_start) = usize::try_from(line.markdown_start_byte) else {
            return false;
        };
        let Ok(markdown_end) = usize::try_from(line.markdown_end_byte) else {
            return false;
        };
        let Ok(source_start) = usize::try_from(line.source_range.start.byte_offset) else {
            return false;
        };
        let Ok(source_end) = usize::try_from(line.source_range.end.byte_offset) else {
            return false;
        };
        if line.markdown_start_byte != expected_start
            || markdown_start > markdown_end
            || markdown_end > markdown.len()
            || !markdown.is_char_boundary(markdown_start)
            || !markdown.is_char_boundary(markdown_end)
            || !range_is_valid(line.source_range, source)
            || line.source_range.start.byte_offset < owner_range.start.byte_offset
            || line.source_range.end.byte_offset > owner_range.end.byte_offset
            || source_end.checked_sub(source_start) != Some(markdown_end - markdown_start)
            || source[source_start..source_end] != markdown[markdown_start..markdown_end]
        {
            return false;
        }
        expected_start = match line.markdown_end_byte.checked_add(1) {
            Some(start) => start,
            None => return false,
        };
    }
    lines.last().is_some_and(|line| {
        usize::try_from(line.markdown_end_byte).ok() == Some(markdown.len())
            && markdown.split('\n').count() == lines.len()
    })
}

fn range_is_valid(range: SourceRange, source: &str) -> bool {
    let Ok(start) = usize::try_from(range.start.byte_offset) else {
        return false;
    };
    let Ok(end) = usize::try_from(range.end.byte_offset) else {
        return false;
    };
    start <= end
        && end <= source.len()
        && source.is_char_boundary(start)
        && source.is_char_boundary(end)
}

fn build_page_work<'document>(
    document: &'document ParsedLogseqDocument,
    manifest_sha256: Sha256Digest,
    identity: IdentityContext,
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> Result<PageWork<'document>, PrepareImportError> {
    let title_properties = page_title_properties(document);
    let title_override = if title_properties.len() == 1 {
        let (value, value_range, _) = title_properties[0];
        if value.is_empty() {
            diagnostics.push(ImportDiagnostic::error_at(
                DiagnosticCode::EmptyPageTitle,
                &document.relative_path,
                value_range,
                "page title property is empty",
                Some("Provide one non-empty title property or remove it".to_owned()),
            ));
            None
        } else {
            Some(value)
        }
    } else if title_properties.len() > 1 {
        for (_, _, range) in &title_properties {
            diagnostics.push(ImportDiagnostic::error_at(
                DiagnosticCode::MultiplePageTitles,
                &document.relative_path,
                *range,
                "page contains multiple title properties",
                Some("Keep exactly one page title property".to_owned()),
            ));
        }
        None
    } else {
        None
    };

    let (uuid, source_identity, kind, mut aliases) = match &document.source {
        LogseqDocumentSource::Page { file_title } => {
            let canonical_identity = normalize_logseq_identity(file_title);
            let uuid = Uuid::new_v5(
                &identity.import_namespace_uuid,
                canonical_identity.as_bytes(),
            );
            let title = title_override.unwrap_or(file_title).to_owned();
            let mut aliases = vec![
                canonical_identity.clone(),
                normalize_logseq_identity(&title),
            ];
            aliases.sort();
            aliases.dedup();
            (
                uuid,
                canonical_identity,
                ImportPageKind::Note { title },
                aliases,
            )
        }
        LogseqDocumentSource::Journal { date } => {
            let date_identity = date.as_str().to_owned();
            let uuid = Uuid::new_v5(&identity.workspace_uuid, date_identity.as_bytes());
            let title = title_override.unwrap_or(date.as_str()).to_owned();
            let mut aliases = journal_aliases(date.as_str());
            aliases.push(normalize_logseq_identity(&title));
            aliases.sort();
            aliases.dedup();
            (
                uuid,
                date_identity,
                ImportPageKind::Journal { date: date.clone() },
                aliases,
            )
        }
    };
    aliases.retain(|alias| !alias.is_empty());
    if source_identity.is_empty() {
        diagnostics.push(ImportDiagnostic::error(
            DiagnosticCode::EmptyPageTitle,
            Some(document.relative_path.clone()),
            "decoded page identity is empty after canonical normalization",
            Some("Rename the source page before importing".to_owned()),
        ));
    }
    let destination_title_key = match &kind {
        ImportPageKind::Note { title } => normalize_notes_title(title),
        ImportPageKind::Journal { date } => normalize_notes_title(date.as_str()),
    };

    let mut blocks = Vec::with_capacity(document.blocks.len());
    let mut source_index_to_position = BTreeMap::new();
    let has_preamble = if let Some(preamble) = &document.preamble {
        let content_hash =
            Sha256Digest::from_bytes(Sha256::digest(preamble.markdown.as_bytes()).into());
        let derived_uuid = derived_block_uuid(uuid, &[], content_hash);
        blocks.push(BlockWork {
            derived_uuid,
            target_uuid: derived_uuid,
            parent_index: None,
            source: ImportBlockSource::Preamble,
            sibling_ordinal: 0,
            structural_path: Vec::new(),
            source_range: preamble.source_range,
            source_content_sha256: content_hash,
            markdown: preamble.markdown.clone(),
            source_lines: preamble.source_lines.clone(),
            explicit_identity: ExplicitIdentity::None,
            task: None,
        });
        true
    } else {
        false
    };
    for block in &document.blocks {
        let position = blocks.len();
        source_index_to_position.insert(block.index, position);
        let structural_path = structural_path(block, &blocks, &source_index_to_position)?;
        let content_hash =
            Sha256Digest::from_bytes(Sha256::digest(block.markdown.as_bytes()).into());
        let derived_uuid = derived_block_uuid(uuid, &structural_path, content_hash);
        let explicit_identity = block_identity_candidate(document, block, diagnostics)?;
        let task = block_task_mapping(document, block)?;
        blocks.push(BlockWork {
            derived_uuid,
            target_uuid: derived_uuid,
            parent_index: block.parent_index.map(|parent| {
                *source_index_to_position
                    .get(&parent)
                    .expect("validated parent precedes child")
            }),
            source: ImportBlockSource::Structural {
                source_block_index: block.index,
                structural_path: structural_path.clone(),
            },
            sibling_ordinal: if has_preamble && block.parent_index.is_none() {
                block.sibling_index.checked_add(1).ok_or_else(|| {
                    PrepareImportError::InvalidSourceAst {
                        relative_path: document.relative_path.clone(),
                    }
                })?
            } else {
                block.sibling_index
            },
            structural_path,
            source_range: block.source_range,
            source_content_sha256: content_hash,
            markdown: block.markdown.clone(),
            source_lines: block.source_lines.clone(),
            explicit_identity,
            task,
        });
    }

    Ok(PageWork {
        document,
        manifest_sha256,
        uuid,
        source_identity,
        destination_title_key,
        kind,
        aliases,
        blocks,
    })
}

fn page_title_properties(document: &ParsedLogseqDocument) -> Vec<(&str, SourceRange, SourceRange)> {
    document
        .constructs
        .iter()
        .filter_map(|construct| match (&construct.owner, &construct.kind) {
            (
                LogseqConstructOwner::Preamble,
                LogseqConstructKind::Property {
                    name,
                    value,
                    value_range,
                },
            ) if name.eq_ignore_ascii_case("title") => {
                Some((value.as_str(), *value_range, construct.source_range))
            }
            _ => None,
        })
        .collect()
}

fn structural_path(
    block: &LogseqSourceBlock,
    prior_blocks: &[BlockWork],
    source_index_to_position: &BTreeMap<u64, usize>,
) -> Result<Vec<u64>, PrepareImportError> {
    let mut path = block
        .parent_index
        .map(|parent_index| {
            let position = *source_index_to_position
                .get(&parent_index)
                .expect("validated parent exists");
            prior_blocks[position].structural_path.clone()
        })
        .unwrap_or_default();
    path.push(block.sibling_index);
    Ok(path)
}

fn derived_block_uuid(
    page_uuid: Uuid,
    structural_path: &[u64],
    content_hash: Sha256Digest,
) -> Uuid {
    let mut name = Vec::with_capacity(
        BLOCK_ID_DOMAIN.len() + 8 + structural_path.len() * 8 + content_hash.as_bytes().len(),
    );
    name.extend_from_slice(BLOCK_ID_DOMAIN);
    name.extend_from_slice(&(structural_path.len() as u64).to_be_bytes());
    for ordinal in structural_path {
        name.extend_from_slice(&ordinal.to_be_bytes());
    }
    name.extend_from_slice(content_hash.as_bytes());
    Uuid::new_v5(&page_uuid, &name)
}

fn block_identity_candidate(
    document: &ParsedLogseqDocument,
    block: &LogseqSourceBlock,
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> Result<ExplicitIdentity, PrepareImportError> {
    let properties = document
        .constructs
        .iter()
        .filter_map(|construct| match (&construct.owner, &construct.kind) {
            (
                LogseqConstructOwner::Block { index },
                LogseqConstructKind::Property {
                    name,
                    value,
                    value_range,
                },
            ) if *index == block.index && name.eq_ignore_ascii_case("id") => {
                Some((value.as_str(), *value_range, construct.source_range))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    if properties.is_empty() {
        return Ok(ExplicitIdentity::None);
    }
    if properties.len() > 1 {
        for (_, _, range) in properties {
            diagnostics.push(ImportDiagnostic::error_at(
                DiagnosticCode::MultipleBlockIdentityProperties,
                &document.relative_path,
                range,
                "block contains multiple identity properties",
                Some("Keep one valid unique id property on the block".to_owned()),
            ));
        }
        return Ok(ExplicitIdentity::Rejected);
    }

    let (value, value_range, property_range) = properties[0];
    let parsed = parse_canonical_uuid(value);
    match parsed {
        Some(uuid) => {
            validate_property_logical_line(document, block, property_range, value)?;
            Ok(ExplicitIdentity::Candidate {
                uuid,
                range: property_range,
            })
        }
        None => {
            diagnostics.push(ImportDiagnostic::error_at(
                DiagnosticCode::InvalidBlockUuid,
                &document.relative_path,
                value_range,
                "block identity property is not a non-nil UUID",
                Some("Replace it with one valid unique UUID".to_owned()),
            ));
            Ok(ExplicitIdentity::Rejected)
        }
    }
}

fn validate_property_logical_line(
    document: &ParsedLogseqDocument,
    block: &LogseqSourceBlock,
    property_range: SourceRange,
    expected_value: &str,
) -> Result<(), PrepareImportError> {
    let line_index = property_range
        .start
        .line
        .checked_sub(block.content_range.start.line)
        .ok_or_else(|| PrepareImportError::InvalidSourceAst {
            relative_path: document.relative_path.clone(),
        })? as usize;
    let line = block.markdown.split('\n').nth(line_index).ok_or_else(|| {
        PrepareImportError::InvalidSourceAst {
            relative_path: document.relative_path.clone(),
        }
    })?;
    let (name, value) =
        line.trim()
            .split_once("::")
            .ok_or_else(|| PrepareImportError::InvalidSourceAst {
                relative_path: document.relative_path.clone(),
            })?;
    if !name.trim().eq_ignore_ascii_case("id") || value.trim() != expected_value {
        return Err(PrepareImportError::InvalidSourceAst {
            relative_path: document.relative_path.clone(),
        });
    }
    Ok(())
}

fn block_task_mapping(
    document: &ParsedLogseqDocument,
    block: &LogseqSourceBlock,
) -> Result<Option<ImportTaskMapping>, PrepareImportError> {
    let mapping = document.constructs.iter().find_map(|construct| {
        let LogseqConstructOwner::Block { index } = construct.owner else {
            return None;
        };
        let LogseqConstructKind::TaskMarker { marker } = &construct.kind else {
            return None;
        };
        (index == block.index).then(|| {
            parse_task_marker(marker).map(|source_marker| ImportTaskMapping {
                source_marker,
                target_state: map_task_state(source_marker),
                source_range: construct.source_range,
            })
        })
    });
    match mapping {
        Some(Some(mapping)) => Ok(Some(mapping)),
        Some(None) => Err(PrepareImportError::InvalidSourceAst {
            relative_path: document.relative_path.clone(),
        }),
        None => Ok(None),
    }
}

fn parse_task_marker(marker: &str) -> Option<LogseqTaskMarker> {
    match marker {
        "TODO" => Some(LogseqTaskMarker::Todo),
        "DOING" => Some(LogseqTaskMarker::Doing),
        "NOW" => Some(LogseqTaskMarker::Now),
        "LATER" => Some(LogseqTaskMarker::Later),
        "DONE" => Some(LogseqTaskMarker::Done),
        "WAIT" => Some(LogseqTaskMarker::Wait),
        "WAITING" => Some(LogseqTaskMarker::Waiting),
        "CANCELLED" => Some(LogseqTaskMarker::Cancelled),
        "CANCELED" => Some(LogseqTaskMarker::Canceled),
        "IN-PROGRESS" => Some(LogseqTaskMarker::InProgress),
        _ => None,
    }
}

const fn map_task_state(marker: LogseqTaskMarker) -> ImportTaskState {
    match marker {
        LogseqTaskMarker::Todo => ImportTaskState::Todo,
        LogseqTaskMarker::Doing | LogseqTaskMarker::InProgress => ImportTaskState::Doing,
        LogseqTaskMarker::Now => ImportTaskState::Now,
        LogseqTaskMarker::Later => ImportTaskState::Later,
        LogseqTaskMarker::Done => ImportTaskState::Done,
        LogseqTaskMarker::Wait | LogseqTaskMarker::Waiting => ImportTaskState::Waiting,
        LogseqTaskMarker::Cancelled | LogseqTaskMarker::Canceled => ImportTaskState::Cancelled,
    }
}

fn diagnose_page_collisions(pages: &[PageWork<'_>], diagnostics: &mut Vec<ImportDiagnostic>) {
    let mut page_uuids: BTreeMap<Uuid, Vec<&PageWork<'_>>> = BTreeMap::new();
    let mut journal_dates: BTreeMap<&str, Vec<&PageWork<'_>>> = BTreeMap::new();
    let mut normal_identities: BTreeMap<&str, Vec<&PageWork<'_>>> = BTreeMap::new();
    let mut destination_titles: BTreeMap<&str, Vec<&PageWork<'_>>> = BTreeMap::new();
    for page in pages {
        page_uuids.entry(page.uuid).or_default().push(page);
        destination_titles
            .entry(&page.destination_title_key)
            .or_default()
            .push(page);
        match &page.kind {
            ImportPageKind::Note { .. } => normal_identities
                .entry(&page.source_identity)
                .or_default()
                .push(page),
            ImportPageKind::Journal { date } => {
                journal_dates.entry(date.as_str()).or_default().push(page);
            }
        }
    }
    for duplicates in normal_identities.values().filter(|pages| pages.len() > 1) {
        for page in duplicates {
            diagnostics.push(ImportDiagnostic::error(
                DiagnosticCode::DuplicatePageIdentity,
                Some(page.document.relative_path.clone()),
                "multiple page sources have the same canonical decoded identity",
                Some("Rename one source page before importing".to_owned()),
            ));
        }
    }
    for duplicates in journal_dates.values().filter(|pages| pages.len() > 1) {
        for page in duplicates {
            diagnostics.push(ImportDiagnostic::error(
                DiagnosticCode::DuplicateJournalDate,
                Some(page.document.relative_path.clone()),
                "multiple journal sources represent the same date",
                Some("Keep exactly one source journal for this date".to_owned()),
            ));
        }
    }
    for duplicates in page_uuids.values().filter(|pages| pages.len() > 1) {
        for page in duplicates {
            diagnostics.push(ImportDiagnostic::error(
                DiagnosticCode::DuplicateTargetUuid,
                Some(page.document.relative_path.clone()),
                "multiple source pages map to the same target UUID",
                Some("Resolve page identity collisions before importing".to_owned()),
            ));
        }
    }
    for duplicates in destination_titles.values().filter(|pages| pages.len() > 1) {
        for page in duplicates {
            diagnostics.push(ImportDiagnostic::error(
                DiagnosticCode::DuplicatePageTitle,
                Some(page.document.relative_path.clone()),
                "multiple source pages collide under destination title normalization",
                Some(
                    "Choose destination titles that remain distinct after Unicode normalization"
                        .to_owned(),
                ),
            ));
        }
    }
}

fn resolve_block_identities(pages: &mut [PageWork<'_>], diagnostics: &mut Vec<ImportDiagnostic>) {
    let page_uuids = pages.iter().map(|page| page.uuid).collect::<BTreeSet<_>>();
    let derived_owners = pages
        .iter()
        .enumerate()
        .flat_map(|(page_index, page)| {
            page.blocks
                .iter()
                .enumerate()
                .map(move |(block_index, block)| (block.derived_uuid, (page_index, block_index)))
        })
        .fold(
            BTreeMap::<Uuid, Vec<(usize, usize)>>::new(),
            |mut owners, (uuid, owner)| {
                owners.entry(uuid).or_default().push(owner);
                owners
            },
        );
    let explicit_owners = pages
        .iter()
        .enumerate()
        .flat_map(|(page_index, page)| {
            page.blocks
                .iter()
                .enumerate()
                .filter_map(move |(block_index, block)| {
                    block
                        .explicit_identity
                        .candidate_uuid()
                        .map(|uuid| (uuid, (page_index, block_index)))
                })
        })
        .fold(
            BTreeMap::<Uuid, Vec<(usize, usize)>>::new(),
            |mut owners, (uuid, owner)| {
                owners.entry(uuid).or_default().push(owner);
                owners
            },
        );

    for owners in derived_owners.values().filter(|owners| owners.len() > 1) {
        for &(page_index, block_index) in owners {
            let page = &pages[page_index];
            diagnostics.push(ImportDiagnostic::error_at(
                DiagnosticCode::DuplicateBlockUuid,
                &page.document.relative_path,
                page.blocks[block_index].source_range,
                "derived block identity collision was detected",
                Some("Do not apply this plan; change the import namespace".to_owned()),
            ));
        }
    }
    for (uuid, owners) in &derived_owners {
        if !page_uuids.contains(uuid) {
            continue;
        }
        for &(page_index, block_index) in owners {
            let page = &pages[page_index];
            diagnostics.push(ImportDiagnostic::error_at(
                DiagnosticCode::DuplicateTargetUuid,
                &page.document.relative_path,
                page.blocks[block_index].source_range,
                "derived block identity collides with a page identity",
                Some("Do not apply this plan; change the import namespace".to_owned()),
            ));
        }
    }

    for (page_index, page) in pages.iter_mut().enumerate() {
        for (block_index, block) in page.blocks.iter_mut().enumerate() {
            let candidate = block.explicit_identity.candidate_uuid();
            let Some(candidate) = candidate else {
                continue;
            };
            let duplicate = explicit_owners
                .get(&candidate)
                .is_some_and(|owners| owners.len() > 1);
            let cross_kind = page_uuids.contains(&candidate);
            let derived_collision = derived_owners.get(&candidate).is_some_and(|owners| {
                owners
                    .iter()
                    .any(|owner| *owner != (page_index, block_index))
            });
            if duplicate || cross_kind || derived_collision {
                let range = match block.explicit_identity {
                    ExplicitIdentity::Candidate { range, .. } => range,
                    ExplicitIdentity::None | ExplicitIdentity::Rejected => unreachable!(),
                };
                diagnostics.push(ImportDiagnostic::error_at(
                    if duplicate || derived_collision {
                        DiagnosticCode::DuplicateBlockUuid
                    } else {
                        DiagnosticCode::DuplicateTargetUuid
                    },
                    &page.document.relative_path,
                    range,
                    "block identity property collides with another target identity",
                    Some("Assign a unique block UUID before importing".to_owned()),
                ));
                block.explicit_identity = ExplicitIdentity::Rejected;
                continue;
            }
            block.target_uuid = candidate;
        }
    }
}

fn build_identity_maps(
    pages: &[PageWork<'_>],
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> ImportIdentityMaps {
    let mut aliases = BTreeMap::<String, Vec<(Uuid, &str)>>::new();
    let mut journal_date_candidates = BTreeMap::<String, Vec<Uuid>>::new();
    let mut block_source_uuids = BTreeMap::new();
    for page in pages {
        for alias in &page.aliases {
            aliases
                .entry(alias.clone())
                .or_default()
                .push((page.uuid, &page.document.relative_path));
        }
        if let ImportPageKind::Journal { date } = &page.kind {
            journal_date_candidates
                .entry(date.as_str().to_owned())
                .or_default()
                .push(page.uuid);
        }
        for block in &page.blocks {
            if let ExplicitIdentity::Candidate { uuid, .. } = block.explicit_identity {
                block_source_uuids.insert(uuid, block.target_uuid);
            }
        }
    }

    let mut page_titles = BTreeMap::new();
    let mut ambiguous_page_titles = Vec::new();
    for (alias, mut owners) in aliases {
        owners.sort();
        owners.dedup();
        if owners.len() == 1 {
            page_titles.insert(alias, owners[0].0);
        } else {
            ambiguous_page_titles.push(alias);
            for (_, path) in owners {
                diagnostics.push(ImportDiagnostic::error(
                    DiagnosticCode::DuplicatePageTitle,
                    Some(path.to_owned()),
                    "page title alias resolves to multiple source pages",
                    Some(
                        "Rename one source page or remove the conflicting title property"
                            .to_owned(),
                    ),
                ));
            }
        }
    }

    let journal_dates = journal_date_candidates
        .into_iter()
        .filter_map(|(date, uuids)| (uuids.len() == 1).then(|| (date, uuids[0])))
        .collect();

    ImportIdentityMaps {
        page_titles,
        ambiguous_page_titles,
        journal_dates,
        block_source_uuids,
    }
}

fn collect_references(
    pages: &[PageWork<'_>],
    identities: &ImportIdentityMaps,
    media_references: &[ImportMediaReference],
    limits: &PrepareLimits,
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> Result<Vec<ImportReference>, PrepareImportError> {
    let mut references = Vec::new();
    for page in pages {
        let mut source_index =
            SourceCursor::new(&page.document.raw_markdown, &page.document.relative_path);
        let mut skipped_ranges = fenced_ranges(page.document);
        skipped_ranges.extend(
            media_references
                .iter()
                .filter(|reference| reference.relative_path == page.document.relative_path)
                .map(|reference| {
                    (
                        reference.source_range.start.byte_offset,
                        reference.source_range.end.byte_offset,
                    )
                }),
        );
        skipped_ranges.sort_unstable();
        let mut document_reference_count = 0_u64;
        for block in &page.blocks {
            scan_reference_fragment(
                &page.document.raw_markdown,
                block.source_range,
                ImportReferenceOwner {
                    page_uuid: page.uuid,
                    block_uuid: block.target_uuid,
                },
                &mut source_index,
                &skipped_ranges,
                identities,
                &page.document.relative_path,
                &mut document_reference_count,
                limits,
                diagnostics,
                &mut references,
            )?;
        }
    }
    references.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(
                left.source_range
                    .start
                    .byte_offset
                    .cmp(&right.source_range.start.byte_offset),
            )
            .then(left.kind.cmp(&right.kind))
    });
    Ok(references)
}

#[allow(clippy::too_many_arguments)]
fn scan_reference_fragment(
    source: &str,
    fragment_range: SourceRange,
    owner: ImportReferenceOwner,
    source_index: &mut SourceCursor<'_>,
    skipped_ranges: &[(u64, u64)],
    identities: &ImportIdentityMaps,
    relative_path: &str,
    document_reference_count: &mut u64,
    limits: &PrepareLimits,
    diagnostics: &mut Vec<ImportDiagnostic>,
    output: &mut Vec<ImportReference>,
) -> Result<(), PrepareImportError> {
    let start = usize::try_from(fragment_range.start.byte_offset).map_err(|_| {
        PrepareImportError::InvalidSourceAst {
            relative_path: relative_path.to_owned(),
        }
    })?;
    let end = usize::try_from(fragment_range.end.byte_offset).map_err(|_| {
        PrepareImportError::InvalidSourceAst {
            relative_path: relative_path.to_owned(),
        }
    })?;
    let bytes = source.as_bytes();
    let mut position = start;
    let mut skipped_index =
        skipped_ranges.partition_point(|(_, skip_end)| *skip_end <= start as u64);
    let mut unclosed_wikilink = false;
    let mut unclosed_block_reference = false;
    while position + 1 < end {
        while let Some((_, skip_end)) = skipped_ranges.get(skipped_index)
            && *skip_end <= position as u64
        {
            skipped_index += 1;
        }
        if let Some((skip_start, skip_end)) = skipped_ranges.get(skipped_index)
            && *skip_start <= position as u64
            && (position as u64) < *skip_end
        {
            position = usize::try_from(*skip_end).unwrap_or(end).min(end);
            skipped_index += 1;
            continue;
        }
        if bytes[position] == b'`' {
            position = skip_inline_code(bytes, position, end);
            continue;
        }
        if matches!(bytes[position], b'[' | b'(') && is_escaped(bytes, position) {
            position += 1;
            continue;
        }
        let kind = match bytes[position..end].get(..2) {
            Some(b"[[") => ImportReferenceKind::WikiLink,
            Some(b"((") => ImportReferenceKind::BlockReference,
            _ => {
                position += source[position..end]
                    .chars()
                    .next()
                    .expect("valid UTF-8 boundary")
                    .len_utf8();
                continue;
            }
        };
        let target_start = position + 2;
        if (kind == ImportReferenceKind::WikiLink && unclosed_wikilink)
            || (kind == ImportReferenceKind::BlockReference && unclosed_block_reference)
        {
            position += 2;
            continue;
        }
        let Some((close, reference_end, nested)) = reference_bounds(bytes, position, end, kind)
        else {
            match kind {
                ImportReferenceKind::WikiLink => unclosed_wikilink = true,
                ImportReferenceKind::BlockReference => unclosed_block_reference = true,
            }
            position += 2;
            continue;
        };
        if (reference_end - position) as u64 > limits.max_reference_bytes {
            return Err(PrepareImportError::ReferenceTooLong {
                relative_path: relative_path.to_owned(),
                limit_bytes: limits.max_reference_bytes,
            });
        }
        if *document_reference_count >= limits.max_references_per_document {
            return Err(PrepareImportError::ReferenceLimitExceeded {
                relative_path: relative_path.to_owned(),
                limit: limits.max_references_per_document,
            });
        }
        if output.len() as u64 >= limits.max_references {
            return Err(PrepareImportError::TotalReferenceLimitExceeded {
                limit: limits.max_references,
            });
        }
        let inner = &source[target_start..close];
        let trimmed_start = inner.len() - inner.trim_start().len();
        let trimmed_end = inner.trim_end().len();
        let semantic_start = target_start + trimmed_start;
        let semantic_end = target_start + trimmed_end;
        let target_text = source[semantic_start..semantic_end].to_owned();
        let resolution = if nested {
            ImportReferenceResolution::Unresolved {
                reason: ImportReferenceUnresolvedReason::UnsupportedNested,
            }
        } else {
            resolve_reference(kind, &target_text, identities)
        };
        let source_start_position = source_index.position(position)?;
        let target_start_position = source_index.position(semantic_start)?;
        let target_end_position = source_index.position(semantic_end)?;
        let source_end_position = source_index.position(reference_end)?;
        let source_range = SourceRange {
            start: source_start_position,
            end: source_end_position,
        };
        let target_range = SourceRange {
            start: target_start_position,
            end: target_end_position,
        };
        if let ImportReferenceResolution::Unresolved { reason } = resolution {
            let (code, message) = match (kind, reason) {
                (
                    ImportReferenceKind::WikiLink,
                    ImportReferenceUnresolvedReason::UnsupportedNested,
                ) => (
                    DiagnosticCode::UnsupportedNestedWikilink,
                    "nested page reference syntax is unsupported and was left unchanged",
                ),
                (ImportReferenceKind::WikiLink, ImportReferenceUnresolvedReason::Ambiguous) => (
                    DiagnosticCode::AmbiguousPageReference,
                    "page reference is ambiguous and was left unchanged",
                ),
                (ImportReferenceKind::WikiLink, _) => (
                    DiagnosticCode::UnresolvedPageReference,
                    "page reference could not be resolved and was left unchanged",
                ),
                (ImportReferenceKind::BlockReference, ImportReferenceUnresolvedReason::Invalid) => {
                    (
                        DiagnosticCode::InvalidBlockReference,
                        "block reference is not a valid non-nil UUID and was left unchanged",
                    )
                }
                (ImportReferenceKind::BlockReference, _) => (
                    DiagnosticCode::UnresolvedBlockReference,
                    "block reference could not be resolved and was left unchanged",
                ),
            };
            diagnostics.push(ImportDiagnostic::warning_at(
                code,
                relative_path,
                target_range,
                message,
                None,
            ));
        }
        output.push(ImportReference {
            relative_path: relative_path.to_owned(),
            owner,
            kind,
            raw_spelling: source[position..reference_end].to_owned(),
            target_text,
            source_range,
            target_range,
            resolution,
        });
        *document_reference_count += 1;
        position = reference_end;
    }
    Ok(())
}

fn reference_bounds(
    bytes: &[u8],
    start: usize,
    end: usize,
    kind: ImportReferenceKind,
) -> Option<(usize, usize, bool)> {
    match kind {
        ImportReferenceKind::BlockReference => {
            let target_start = start + 2;
            let close = bytes[target_start..end]
                .windows(2)
                .position(|window| window == b"))")?
                + target_start;
            Some((close, close + 2, false))
        }
        ImportReferenceKind::WikiLink => {
            let mut depth = 1_u64;
            let mut cursor = start + 2;
            let mut nested = false;
            while cursor + 1 < end {
                match &bytes[cursor..cursor + 2] {
                    b"[[" => {
                        depth = depth.checked_add(1)?;
                        nested = true;
                        cursor += 2;
                    }
                    b"]]" => {
                        depth -= 1;
                        if depth == 0 {
                            return Some((cursor, cursor + 2, nested));
                        }
                        cursor += 2;
                    }
                    _ => cursor += 1,
                }
            }
            None
        }
    }
}

fn resolve_reference(
    kind: ImportReferenceKind,
    target: &str,
    identities: &ImportIdentityMaps,
) -> ImportReferenceResolution {
    match kind {
        ImportReferenceKind::WikiLink => {
            let normalized = normalize_logseq_identity(target);
            if identities
                .ambiguous_page_titles
                .binary_search(&normalized)
                .is_ok()
            {
                return ImportReferenceResolution::Unresolved {
                    reason: ImportReferenceUnresolvedReason::Ambiguous,
                };
            }
            identities.page_titles.get(&normalized).map_or(
                ImportReferenceResolution::Unresolved {
                    reason: ImportReferenceUnresolvedReason::Unknown,
                },
                |uuid| ImportReferenceResolution::Resolved {
                    target_uuid: *uuid,
                    target_kind: ImportReferenceTargetKind::Page,
                },
            )
        }
        ImportReferenceKind::BlockReference => {
            let Some(source_uuid) = parse_canonical_uuid(target) else {
                return ImportReferenceResolution::Unresolved {
                    reason: ImportReferenceUnresolvedReason::Invalid,
                };
            };
            identities.block_source_uuids.get(&source_uuid).map_or(
                ImportReferenceResolution::Unresolved {
                    reason: ImportReferenceUnresolvedReason::Unknown,
                },
                |uuid| ImportReferenceResolution::Resolved {
                    target_uuid: *uuid,
                    target_kind: ImportReferenceTargetKind::Block,
                },
            )
        }
    }
}

fn fenced_ranges(document: &ParsedLogseqDocument) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    let mut opening = None;
    for construct in &document.constructs {
        match construct.kind {
            LogseqConstructKind::FenceStart { .. } => {
                opening = Some(construct.source_range.start.byte_offset);
            }
            LogseqConstructKind::FenceEnd { .. } => {
                if let Some(start) = opening.take() {
                    ranges.push((start, construct.source_range.end.byte_offset));
                }
            }
            _ => {}
        }
    }
    if let Some(start) = opening {
        ranges.push((start, document.raw_markdown.len() as u64));
    }
    ranges
}

fn skip_inline_code(bytes: &[u8], start: usize, end: usize) -> usize {
    let run = bytes[start..end]
        .iter()
        .take_while(|byte| **byte == b'`')
        .count();
    let mut position = start + run;
    while position + run <= end {
        if bytes[position..position + run]
            .iter()
            .all(|byte| *byte == b'`')
        {
            return position + run;
        }
        position += 1;
    }
    start + run
}

fn is_escaped(bytes: &[u8], position: usize) -> bool {
    let mut preceding = 0;
    let mut cursor = position;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        preceding += 1;
        cursor -= 1;
    }
    preceding % 2 == 1
}

struct SourceCursor<'source> {
    source: &'source str,
    relative_path: &'source str,
    byte_offset: usize,
    line: u64,
    column: u64,
}

impl<'source> SourceCursor<'source> {
    fn new(source: &'source str, relative_path: &'source str) -> Self {
        Self {
            source,
            relative_path,
            byte_offset: 0,
            line: 1,
            column: 1,
        }
    }

    fn position(&mut self, byte_offset: usize) -> Result<SourcePosition, PrepareImportError> {
        if byte_offset < self.byte_offset || !self.source.is_char_boundary(byte_offset) {
            return Err(PrepareImportError::InvalidSourceAst {
                relative_path: self.relative_path.to_owned(),
            });
        }
        for character in self.source[self.byte_offset..byte_offset].chars() {
            if character == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        self.byte_offset = byte_offset;
        Ok(SourcePosition {
            line: self.line,
            column: self.column,
            byte_offset: byte_offset as u64,
        })
    }
}

fn finalize_page(page: &PageWork<'_>) -> ImportPage {
    let blocks = page
        .blocks
        .iter()
        .map(|block| {
            let parent_block_uuid = block
                .parent_index
                .map(|position| page.blocks[position].target_uuid);
            let (identity, id_property_range) = match block.explicit_identity {
                ExplicitIdentity::Candidate { uuid, range } => (
                    ImportBlockIdentity::Preserved {
                        source_uuid: uuid,
                        property_range: range,
                    },
                    Some(range),
                ),
                ExplicitIdentity::None => (ImportBlockIdentity::Derived, None),
                ExplicitIdentity::Rejected => (ImportBlockIdentity::RejectedSourceProperty, None),
            };
            let mut markdown = id_property_range.map_or_else(
                || block.markdown.clone(),
                |range| remove_identity_property_line(&block.markdown, block.source_range, range),
            );
            if let Some(task) = &block.task {
                markdown = strip_task_marker(&markdown, task.source_marker);
            }
            ImportBlock {
                uuid: block.target_uuid,
                page_uuid: page.uuid,
                parent_block_uuid,
                sibling_ordinal: block.sibling_ordinal,
                markdown,
                task: block.task.clone(),
                provenance: ImportBlockProvenance {
                    relative_path: page.document.relative_path.clone(),
                    source: block.source.clone(),
                    source_range: block.source_range,
                    source_content_sha256: block.source_content_sha256,
                    identity,
                },
            }
        })
        .collect();
    ImportPage {
        uuid: page.uuid,
        kind: page.kind.clone(),
        source: ImportPageSource {
            relative_path: page.document.relative_path.clone(),
            manifest_sha256: page.manifest_sha256,
        },
        source_identity: page.source_identity.clone(),
        blocks,
    }
}

fn remove_identity_property_line(
    markdown: &str,
    block_source_range: SourceRange,
    property_range: SourceRange,
) -> String {
    let line_index = property_range
        .start
        .line
        .saturating_sub(block_source_range.start.line) as usize;
    markdown
        .split('\n')
        .enumerate()
        .filter_map(|(index, line)| (index != line_index).then_some(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_task_marker(markdown: &str, marker: LogseqTaskMarker) -> String {
    let marker = match marker {
        LogseqTaskMarker::Todo => "TODO",
        LogseqTaskMarker::Doing => "DOING",
        LogseqTaskMarker::Now => "NOW",
        LogseqTaskMarker::Later => "LATER",
        LogseqTaskMarker::Done => "DONE",
        LogseqTaskMarker::Wait => "WAIT",
        LogseqTaskMarker::Waiting => "WAITING",
        LogseqTaskMarker::Cancelled => "CANCELLED",
        LogseqTaskMarker::Canceled => "CANCELED",
        LogseqTaskMarker::InProgress => "IN-PROGRESS",
    };
    let (first, rest) = markdown
        .find('\n')
        .map_or((markdown, ""), |end| markdown.split_at(end));
    let leading_bytes = first.len() - first.trim_start_matches([' ', '\t']).len();
    let (leading, content) = first.split_at(leading_bytes);
    let Some(suffix) = content.strip_prefix(marker) else {
        return markdown.to_owned();
    };
    format!("{leading}{}{rest}", suffix.trim_start_matches([' ', '\t']))
}

fn build_provenance(
    identity: IdentityContext,
    manifest: &GraphManifest,
    pages: &[ImportPage],
    plan_sha256: Sha256Digest,
) -> ImportProvenance {
    ImportProvenance {
        planner_version: IMPORT_PLANNER_VERSION,
        plan_sha256,
        source_manifest: manifest.clone(),
        identity_context: identity,
        page_mappings: pages
            .iter()
            .map(|page| ImportPageProvenance {
                relative_path: page.source.relative_path.clone(),
                source_identity: page.source_identity.clone(),
                target_uuid: page.uuid,
            })
            .collect(),
        block_mappings: pages
            .iter()
            .flat_map(|page| {
                page.blocks.iter().map(|block| ImportBlockMapping {
                    relative_path: block.provenance.relative_path.clone(),
                    source: block.provenance.source.clone(),
                    target_uuid: block.uuid,
                    preserved_source_uuid: match block.provenance.identity {
                        ImportBlockIdentity::Preserved { source_uuid, .. } => Some(source_uuid),
                        ImportBlockIdentity::Derived
                        | ImportBlockIdentity::RejectedSourceProperty => None,
                    },
                })
            })
            .collect(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedPlanHashInput<'input> {
    planner_version: u32,
    identity: IdentityContext,
    source_manifest: &'input GraphManifest,
    pages: &'input [ImportPage],
    references: &'input [ImportReference],
    media_references: &'input [ImportMediaReference],
    identity_maps: &'input ImportIdentityMaps,
    report: &'input ImportReport,
}

fn hash_prepared_plan(
    identity: IdentityContext,
    manifest: &GraphManifest,
    pages: &[ImportPage],
    references: &[ImportReference],
    media_references: &[ImportMediaReference],
    identity_maps: &ImportIdentityMaps,
    report: &ImportReport,
) -> Result<Sha256Digest, PrepareImportError> {
    const DOMAIN: &[u8] = b"notes-rs/import/logseq/prepared-plan/v2\0";
    let canonical = serde_json::to_vec(&PreparedPlanHashInput {
        planner_version: IMPORT_PLANNER_VERSION,
        identity,
        source_manifest: manifest,
        pages,
        references,
        media_references,
        identity_maps,
        report,
    })
    .map_err(PrepareImportError::PlanSerialization)?;
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update((canonical.len() as u64).to_be_bytes());
    hasher.update(canonical);
    Ok(Sha256Digest::from_bytes(hasher.finalize().into()))
}

fn build_report(
    pages: &[ImportPage],
    references: &[ImportReference],
    media_references: &[ImportMediaReference],
    unreferenced_asset_count: u64,
    unreferenced_drawing_count: u64,
    diagnostics: &[ImportDiagnostic],
) -> ImportReport {
    let blocks = pages
        .iter()
        .flat_map(|page| page.blocks.iter())
        .collect::<Vec<_>>();
    let resolved_reference_count = references
        .iter()
        .filter(|reference| {
            matches!(
                reference.resolution,
                ImportReferenceResolution::Resolved { .. }
            )
        })
        .count() as u64;
    ImportReport {
        page_count: pages.len() as u64,
        journal_count: pages
            .iter()
            .filter(|page| matches!(page.kind, ImportPageKind::Journal { .. }))
            .count() as u64,
        block_count: blocks.len() as u64,
        synthetic_preamble_block_count: blocks
            .iter()
            .filter(|block| matches!(block.provenance.source, ImportBlockSource::Preamble))
            .count() as u64,
        task_count: blocks.iter().filter(|block| block.task.is_some()).count() as u64,
        reference_count: references.len() as u64,
        resolved_reference_count,
        unresolved_reference_count: references.len() as u64 - resolved_reference_count,
        media_reference_count: media_references.len() as u64,
        markdown_image_count: media_references
            .iter()
            .filter(|reference| reference.kind == ImportMediaKind::MarkdownImage)
            .count() as u64,
        legacy_excalidraw_count: media_references
            .iter()
            .filter(|reference| reference.kind == ImportMediaKind::LegacyExcalidraw)
            .count() as u64,
        local_media_reference_count: media_references
            .iter()
            .filter(|reference| {
                matches!(
                    reference.resolution,
                    ImportMediaResolution::LocalManifest { .. }
                )
            })
            .count() as u64,
        inline_media_reference_count: media_references
            .iter()
            .filter(|reference| {
                matches!(
                    reference.resolution,
                    ImportMediaResolution::InlineData { .. }
                )
            })
            .count() as u64,
        blocked_remote_media_reference_count: media_references
            .iter()
            .filter(|reference| {
                matches!(
                    reference.resolution,
                    ImportMediaResolution::RemoteBlocked { .. }
                )
            })
            .count() as u64,
        missing_media_reference_count: media_references
            .iter()
            .filter(|reference| {
                matches!(reference.resolution, ImportMediaResolution::Missing { .. })
            })
            .count() as u64,
        blocked_unsafe_media_reference_count: media_references
            .iter()
            .filter(|reference| {
                matches!(reference.resolution, ImportMediaResolution::Blocked { .. })
            })
            .count() as u64,
        unsupported_media_reference_count: media_references
            .iter()
            .filter(|reference| {
                matches!(
                    reference.resolution,
                    ImportMediaResolution::Unsupported { .. }
                )
            })
            .count() as u64,
        unreferenced_asset_count,
        unreferenced_drawing_count,
        preserved_block_uuid_count: blocks
            .iter()
            .filter(|block| {
                matches!(
                    block.provenance.identity,
                    ImportBlockIdentity::Preserved { .. }
                )
            })
            .count() as u64,
        derived_block_uuid_count: blocks
            .iter()
            .filter(|block| {
                !matches!(
                    block.provenance.identity,
                    ImportBlockIdentity::Preserved { .. }
                )
            })
            .count() as u64,
        blocking_diagnostic_count: diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .count() as u64,
        diagnostics: diagnostics.to_vec(),
    }
}

fn normalize_notes_title(title: &str) -> String {
    title.trim().nfkc().flat_map(char::to_lowercase).collect()
}

fn parse_canonical_uuid(value: &str) -> Option<Uuid> {
    let uuid = Uuid::parse_str(value).ok()?;
    (!uuid.is_nil()
        && value.len() == 36
        && uuid.hyphenated().to_string().eq_ignore_ascii_case(value))
    .then_some(uuid)
}

fn normalize_logseq_identity(title: &str) -> String {
    title.trim().nfc().flat_map(char::to_lowercase).collect()
}

fn journal_aliases(date: &str) -> Vec<String> {
    let parts = date.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return vec![normalize_logseq_identity(date)];
    }
    let year = parts[0];
    let month = parts[1].parse::<usize>().ok();
    let day = parts[2].parse::<u8>().ok();
    let mut aliases = vec![normalize_logseq_identity(date), date.replace('-', "_")];
    if let (Some(month), Some(day)) = (month, day)
        && let Some(month_name) = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ]
        .get(month.saturating_sub(1))
    {
        let suffix = match day % 100 {
            11..=13 => "th",
            _ => match day % 10 {
                1 => "st",
                2 => "nd",
                3 => "rd",
                _ => "th",
            },
        };
        aliases.push(normalize_logseq_identity(&format!(
            "{month_name} {day}{suffix}, {year}"
        )));
    }
    aliases
}

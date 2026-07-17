//! Pure import discovery and planning primitives.
//!
//! This crate deliberately has no persistence, AI, host, or UI dependencies.
//! Scanning a graph only reads source files and produces a deterministic
//! manifest; applying that manifest belongs to a separate boundary.

mod config;
mod diagnostic;
mod manifest;
mod materialize;
mod media;
mod parser;
mod prepare;
mod prepared;
mod scanner;
mod source;

pub use config::{
    ConfigError, FileNameFormat, LogseqConfig, ParsedLogseqConfig, RelativeDirectory,
    parse_logseq_config,
};
pub use diagnostic::{
    DiagnosticCode, DiagnosticSeverity, ImportDiagnostic, SourcePosition, SourceRange,
};
pub use manifest::{DocumentFormat, GraphManifest, ManifestEntry, Sha256Digest, SourceKind};
pub use materialize::{
    MaterializeMediaError, MaterializeMediaErrorCode, MaterializedMediaAttachment,
    MaterializedMediaBlob, MediaMaterializationDiagnostic, MediaMaterializationIssue,
    MediaMaterializationLimits, MediaMaterializationPlan, MediaRewrite, PreservedMediaReason,
    PreservedMediaReference, materialize_source_media, materialize_source_media_with_limits,
    materialized_attachment_uuid,
};
pub use parser::{
    LogseqParseError, LogseqParseErrorCode, decode_logseq_page_title, parse_logseq_markdown,
};
pub use prepare::{
    PrepareImportError, PrepareImportErrorCode, PrepareLimits, prepare_import,
    prepare_import_with_limits,
};
pub use prepared::{
    IMPORT_PLANNER_VERSION, IdentityContext, ImportBlock, ImportBlockIdentity, ImportBlockMapping,
    ImportBlockProvenance, ImportBlockSource, ImportIdentityMaps, ImportInlineImageMime,
    ImportMarkdownRange, ImportMediaBlockedReason, ImportMediaKind, ImportMediaOwner,
    ImportMediaReference, ImportMediaResolution, ImportMediaUnsupportedReason, ImportPage,
    ImportPageKind, ImportPageProvenance, ImportPageSource, ImportProvenance, ImportReference,
    ImportReferenceKind, ImportReferenceOwner, ImportReferenceResolution,
    ImportReferenceTargetKind, ImportReferenceUnresolvedReason, ImportRemoteMediaScheme,
    ImportReport, ImportRerunDecision, ImportTaskMapping, ImportTaskState, LogseqTaskMarker,
    PreparedImport, compare_import_provenance,
};
pub use scanner::{
    GraphScanReport, ManifestVerification, ScanError, ScanErrorCode, ScanLimits, scan_logseq_graph,
    scan_logseq_graph_with_limits, verify_logseq_manifest, verify_logseq_manifest_with_limits,
};
pub use source::{
    LogseqConstruct, LogseqConstructKind, LogseqConstructOwner, LogseqDocumentSource,
    LogseqJournalDate, LogseqMarkdownSourceLine, LogseqPreamble, LogseqSourceBlock,
    ParsedLogseqDocument,
};

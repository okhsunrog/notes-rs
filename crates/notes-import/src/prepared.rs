use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{GraphManifest, ImportDiagnostic, LogseqJournalDate, Sha256Digest, SourceRange};

pub const IMPORT_PLANNER_VERSION: u32 = 1;

/// Durable identity inputs owned by the caller, never inferred from a path or
/// from the current source manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityContext {
    pub workspace_uuid: Uuid,
    pub import_namespace_uuid: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedImport {
    pub identity: IdentityContext,
    pub pages: Vec<ImportPage>,
    pub references: Vec<ImportReference>,
    pub identity_maps: ImportIdentityMaps,
    pub provenance: ImportProvenance,
    pub report: ImportReport,
}

impl PreparedImport {
    #[must_use]
    pub fn is_committable(&self) -> bool {
        self.report.blocking_diagnostic_count == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPage {
    pub uuid: Uuid,
    pub kind: ImportPageKind,
    pub source: ImportPageSource,
    /// Canonical decoded source identity used to derive a normal-page UUID.
    pub source_identity: String,
    pub blocks: Vec<ImportBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportPageKind {
    Note { title: String },
    Journal { date: LogseqJournalDate },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPageSource {
    pub relative_path: String,
    pub manifest_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBlock {
    pub uuid: Uuid,
    pub page_uuid: Uuid,
    /// `None` means a page root; a page UUID cannot be passed accidentally.
    pub parent_block_uuid: Option<Uuid>,
    /// Zero-based deterministic ordinal among siblings. Persistence maps this
    /// to its concrete OrderKey in a later boundary.
    pub sibling_ordinal: u64,
    pub markdown: String,
    pub task: Option<ImportTaskMapping>,
    pub provenance: ImportBlockProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportTaskState {
    Todo,
    Doing,
    Now,
    Later,
    Done,
    Waiting,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING-KEBAB-CASE")]
pub enum LogseqTaskMarker {
    Todo,
    Doing,
    Now,
    Later,
    Done,
    Wait,
    Waiting,
    Cancelled,
    Canceled,
    InProgress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTaskMapping {
    pub source_marker: LogseqTaskMarker,
    pub target_state: ImportTaskState,
    pub source_range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBlockProvenance {
    pub relative_path: String,
    pub source: ImportBlockSource,
    pub source_range: SourceRange,
    pub source_content_sha256: Sha256Digest,
    pub identity: ImportBlockIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportBlockSource {
    /// Page-level source before the first structural Logseq block. It is
    /// imported as a synthetic page-root block at ordinal zero.
    Preamble,
    Structural {
        source_block_index: u64,
        structural_path: Vec<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ImportBlockIdentity {
    Preserved {
        source_uuid: Uuid,
        property_range: SourceRange,
    },
    Derived,
    /// A deterministic derived UUID is present in the dry-run plan, but the
    /// blocking diagnostic must be resolved before the plan can be applied.
    RejectedSourceProperty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportIdentityMaps {
    /// Only unambiguous normalized aliases are exposed.
    pub page_titles: BTreeMap<String, Uuid>,
    /// Normalized aliases intentionally omitted from `page_titles` because
    /// they refer to more than one source page.
    pub ambiguous_page_titles: Vec<String>,
    pub journal_dates: BTreeMap<String, Uuid>,
    /// Only valid, globally unique Logseq `id::` values are exposed.
    pub block_source_uuids: BTreeMap<Uuid, Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReferenceOwner {
    pub page_uuid: Uuid,
    pub block_uuid: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportReferenceKind {
    WikiLink,
    BlockReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReference {
    pub relative_path: String,
    pub owner: ImportReferenceOwner,
    pub kind: ImportReferenceKind,
    /// Exact source spelling, retained without rewriting the source Markdown.
    pub raw_spelling: String,
    pub target_text: String,
    pub source_range: SourceRange,
    pub target_range: SourceRange,
    pub resolution: ImportReferenceResolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ImportReferenceResolution {
    Resolved {
        target_uuid: Uuid,
        target_kind: ImportReferenceTargetKind,
    },
    Unresolved {
        reason: ImportReferenceUnresolvedReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportReferenceTargetKind {
    Page,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportReferenceUnresolvedReason {
    Unknown,
    Ambiguous,
    Invalid,
    UnsupportedNested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProvenance {
    pub planner_version: u32,
    /// Full exact immutable source snapshot, not merely its digest.
    pub source_manifest: GraphManifest,
    pub identity_context: IdentityContext,
    pub page_mappings: Vec<ImportPageProvenance>,
    pub block_mappings: Vec<ImportBlockMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPageProvenance {
    pub relative_path: String,
    pub source_identity: String,
    pub target_uuid: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBlockMapping {
    pub relative_path: String,
    pub source: ImportBlockSource,
    pub target_uuid: Uuid,
    pub preserved_source_uuid: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub page_count: u64,
    pub journal_count: u64,
    pub block_count: u64,
    pub synthetic_preamble_block_count: u64,
    pub task_count: u64,
    pub reference_count: u64,
    pub resolved_reference_count: u64,
    pub unresolved_reference_count: u64,
    pub preserved_block_uuid_count: u64,
    pub derived_block_uuid_count: u64,
    pub blocking_diagnostic_count: u64,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ImportRerunDecision {
    ExactNoOp,
    RefuseChangedManifest {
        previous_sha256: Sha256Digest,
        current_sha256: Sha256Digest,
    },
    RefuseIdentityContext,
    RefusePlannerVersion {
        previous: u32,
        current: u32,
    },
}

/// Deliberately refuses arbitrary merge semantics. Only an exact manifest,
/// identity context, and planner version can be treated as an idempotent no-op.
///
/// `previous` must have been stored atomically with a successful apply. A
/// dry-run plan is not committed provenance and must never be supplied here;
/// destination emptiness and atomic provenance storage belong to the later
/// persistence boundary.
#[must_use]
pub fn compare_import_provenance(
    previous: &ImportProvenance,
    identity: IdentityContext,
    manifest: &GraphManifest,
) -> ImportRerunDecision {
    if previous.planner_version != IMPORT_PLANNER_VERSION {
        return ImportRerunDecision::RefusePlannerVersion {
            previous: previous.planner_version,
            current: IMPORT_PLANNER_VERSION,
        };
    }
    if previous.identity_context != identity {
        return ImportRerunDecision::RefuseIdentityContext;
    }
    if previous.source_manifest != *manifest {
        return ImportRerunDecision::RefuseChangedManifest {
            previous_sha256: previous.source_manifest.sha256(),
            current_sha256: manifest.sha256(),
        };
    }
    ImportRerunDecision::ExactNoOp
}

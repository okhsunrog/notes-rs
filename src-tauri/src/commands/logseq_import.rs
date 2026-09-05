use super::*;
#[cfg(not(target_os = "android"))]
use anyhow::Context;
#[cfg(not(target_os = "android"))]
use notes_blob::{BlobHash, BlobStore, InstallOutcome};
#[cfg(not(target_os = "android"))]
use notes_core::{
    BlockStyle, ExternalBlockId, ExternalImportAttachment, ExternalImportAttachmentOwner,
    ExternalImportBatch, ExternalImportDigest, ExternalImportFormat, ExternalImportIdentityContext,
    ExternalImportOutcome, ExternalImportPage, ExternalImportPageKind, ExternalImportProvenance,
    ExternalPageId, JournalDate, OrderKey, PageAlias, PageLayout, TaskState,
};
#[cfg(not(target_os = "android"))]
use notes_import::{
    DiagnosticSeverity, DocumentFormat, DrawingConversionPublication, DrawingConversionStatus,
    IdentityContext, ImportBlockPresentation, ImportDiagnostic, ImportMediaKind, ImportMediaOwner,
    ImportMediaResolution, ImportPageKind, ImportTaskState, LoadDrawingConversionErrorCode,
    MaterializedMediaBlob, MediaMaterializationPlan, PreparedImport, SourceKind,
};
#[cfg(not(target_os = "android"))]
use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_os = "android"))]
use std::fs::File;
#[cfg(not(target_os = "android"))]
use std::io::Read;
#[cfg(not(target_os = "android"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "android"))]
use std::sync::Mutex;

// Android exposes the same unavailable-command IPC contract without linking the desktop-only
// importer. Keep these wire-only diagnostics in sync with `notes_import` until the importer
// protocol is promoted to a format-neutral shared crate (for example when Obsidian lands).
#[cfg(target_os = "android")]
mod mobile_diagnostics {
    #![allow(dead_code)]

    use serde::Serialize;

    #[derive(Debug, Clone, Copy, Serialize, specta::Type)]
    #[serde(rename_all = "snake_case")]
    pub enum DiagnosticSeverity {
        Info,
        Warning,
        Error,
    }

    #[derive(Debug, Clone, Copy, Serialize, specta::Type)]
    #[serde(rename_all = "snake_case")]
    pub enum DiagnosticCode {
        ConfigNotFound,
        SourceDirectoryNotFound,
        UnsupportedDocumentFormat,
        MixedIndentation,
        NonCanonicalIndentation,
        NonCanonicalContinuationIndentation,
        UnclosedFence,
        PreservedMacro,
        EmptyPageTitle,
        MultiplePageTitles,
        DuplicatePageIdentity,
        DuplicatePageTitle,
        DuplicateJournalDate,
        DuplicateTargetUuid,
        InvalidBlockUuid,
        MultipleBlockIdentityProperties,
        DuplicateBlockUuid,
        UnresolvedPageReference,
        AmbiguousPageReference,
        InvalidBlockReference,
        UnresolvedBlockReference,
        UnsupportedNestedWikilink,
        MissingMediaSource,
        RemoteMediaBlocked,
        UnsafeMediaSource,
        UnsupportedInlineMedia,
    }

    #[derive(Debug, Clone, Copy, Serialize, specta::Type)]
    #[serde(rename_all = "camelCase")]
    pub struct SourcePosition {
        pub line: u64,
        pub column: u64,
        pub byte_offset: u64,
    }

    #[derive(Debug, Clone, Copy, Serialize, specta::Type)]
    #[serde(rename_all = "camelCase")]
    pub struct SourceRange {
        pub start: SourcePosition,
        pub end: SourcePosition,
    }

    #[derive(Debug, Clone, Serialize, specta::Type)]
    #[serde(rename_all = "camelCase")]
    pub struct ImportDiagnostic {
        pub severity: DiagnosticSeverity,
        pub code: DiagnosticCode,
        pub relative_path: Option<String>,
        pub range: Option<SourceRange>,
        pub message: String,
        pub remediation: Option<String>,
    }
}

#[cfg(target_os = "android")]
use mobile_diagnostics::ImportDiagnostic;

#[cfg(not(target_os = "android"))]
const MAX_PREVIEW_DIAGNOSTICS: usize = 200;
#[cfg(not(target_os = "android"))]
const DOCUMENT_READ_BUFFER_BYTES: usize = 64 * 1024;

#[cfg(not(target_os = "android"))]
#[derive(Debug)]
struct LogseqImportSession {
    session_uuid: uuid::Uuid,
    source_root: PathBuf,
    prepared: PreparedImport,
    scan_diagnostics: Vec<ImportDiagnostic>,
    drawing_conversions: Option<DrawingConversionPublication>,
    static_commit_allowed: bool,
}

#[cfg(not(target_os = "android"))]
struct PreparedGraph {
    prepared: PreparedImport,
    scan_diagnostics: Vec<ImportDiagnostic>,
}

#[cfg(not(target_os = "android"))]
#[derive(Debug, thiserror::Error)]
enum PrepareGraphError {
    #[error(transparent)]
    Scan(#[from] notes_import::ScanError),
    #[error("could not read imported document {relative_path}: {source:#}")]
    DocumentRead {
        relative_path: String,
        #[source]
        source: anyhow::Error,
    },
    #[error(transparent)]
    Parse(#[from] notes_import::LogseqParseError),
    #[error("the selected Logseq graph changed while it was being prepared")]
    SourceChanged,
    #[error(transparent)]
    Prepare(#[from] notes_import::PrepareImportError),
}

#[cfg(not(target_os = "android"))]
#[derive(Debug, thiserror::Error)]
enum CommitPlanError {
    #[error(transparent)]
    Scan(#[from] notes_import::ScanError),
    #[error("the selected Logseq graph changed after the dry run")]
    SourceChanged,
    #[error(transparent)]
    Materialize(#[from] notes_import::MaterializeMediaError),
    #[error("the prepared Logseq graph could not be converted: {0:#}")]
    Adapter(anyhow::Error),
}

#[cfg(not(target_os = "android"))]
#[derive(Default)]
struct LogseqImportSessionState {
    preparing: bool,
    active: Option<LogseqImportSession>,
    committing: Option<uuid::Uuid>,
}

#[derive(Default)]
pub struct LogseqImportSessions {
    #[cfg(not(target_os = "android"))]
    state: Mutex<LogseqImportSessionState>,
}

#[cfg(not(target_os = "android"))]
struct PreparingGuard<'sessions> {
    sessions: &'sessions LogseqImportSessions,
}

#[cfg(not(target_os = "android"))]
impl Drop for PreparingGuard<'_> {
    fn drop(&mut self) {
        self.sessions
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .preparing = false;
    }
}

#[cfg(not(target_os = "android"))]
struct CommittingGuard<'sessions> {
    sessions: &'sessions LogseqImportSessions,
}

#[cfg(not(target_os = "android"))]
impl Drop for CommittingGuard<'_> {
    fn drop(&mut self) {
        self.sessions
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .committing = None;
    }
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum LogseqImportStage {
    Scanning,
    Parsing,
    Preparing,
    Verifying,
    Materializing,
    Installing,
    Committing,
    Complete,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "progress", rename_all = "snake_case")]
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum LogseqImportProgress {
    Indeterminate {
        stage: LogseqImportStage,
    },
    Items {
        stage: LogseqImportStage,
        completed: u64,
        total: u64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum LogseqImportDestination {
    Empty,
    ExistingReceipt,
    NotEmpty,
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum LogseqImportBlocker {
    EmptyImport,
    NonEmptyWorkspace,
    BlockingDiagnostics,
    AliasCollision,
    ExistingImportChanged,
    BackgroundAiEnabled,
    ServerAiStatusUnavailable,
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LogseqImportAvailability {
    #[cfg_attr(target_os = "android", allow(dead_code))]
    Available,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Unavailable {
        reason: LogseqImportUnavailableReason,
    },
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub enum LogseqImportUnavailableReason {
    MobilePlatform,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LogseqImportDiagnosticPage {
    pub offset: u64,
    pub total: u64,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LogseqImportReportSummary {
    pub page_count: u64,
    pub journal_count: u64,
    pub block_count: u64,
    pub synthetic_preamble_block_count: u64,
    pub task_count: u64,
    pub reference_count: u64,
    pub resolved_reference_count: u64,
    pub unresolved_reference_count: u64,
    pub media_reference_count: u64,
    pub markdown_image_count: u64,
    pub drawing_conversion_state: LogseqDrawingConversionState,
    pub prepared_excalidraw_count: u64,
    pub preserved_excalidraw_count: u64,
    pub local_media_reference_count: u64,
    pub inline_media_reference_count: u64,
    pub blocked_remote_media_reference_count: u64,
    pub missing_media_reference_count: u64,
    pub blocked_unsafe_media_reference_count: u64,
    pub unsupported_media_reference_count: u64,
    pub unreferenced_asset_count: u64,
    pub unreferenced_drawing_count: u64,
    pub preserved_block_uuid_count: u64,
    pub derived_block_uuid_count: u64,
    pub warning_count: u64,
    pub blocking_diagnostic_count: u64,
    pub diagnostic_count: u64,
    pub diagnostics_truncated: bool,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum LogseqDrawingConversionState {
    Absent,
    Prepared,
    Invalid,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LogseqImportPreview {
    pub session_uuid: uuid::Uuid,
    pub source_name: String,
    pub manifest_sha256: String,
    pub destination: LogseqImportDestination,
    pub blockers: Vec<LogseqImportBlocker>,
    pub can_commit: bool,
    pub report: LogseqImportReportSummary,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum LogseqImportCommitResult {
    Applied {
        receipt_uuid: uuid::Uuid,
        page_count: u64,
        block_count: u64,
        alias_count: u64,
        attachment_count: u64,
        operation_count: u64,
        open_page_uuid: Option<uuid::Uuid>,
    },
    ExactNoOp {
        receipt_uuid: uuid::Uuid,
        open_page_uuid: Option<uuid::Uuid>,
    },
}

#[tauri::command]
#[specta::specta]
pub const fn logseq_import_capability() -> LogseqImportAvailability {
    #[cfg(target_os = "android")]
    {
        LogseqImportAvailability::Unavailable {
            reason: LogseqImportUnavailableReason::MobilePlatform,
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        LogseqImportAvailability::Available
    }
}

#[tauri::command]
#[specta::specta]
pub fn logseq_import_diagnostics(
    sessions: State<'_, LogseqImportSessions>,
    session_uuid: uuid::Uuid,
    offset: u64,
    limit: u32,
) -> CommandResult<LogseqImportDiagnosticPage> {
    #[cfg(target_os = "android")]
    {
        let _ = (sessions, session_uuid, offset, limit);
        Err(CommandError::new(
            CommandErrorCode::Unavailable,
            "direct Logseq folder import is available on desktop only",
        ))
    }

    #[cfg(not(target_os = "android"))]
    {
        if !(1..=200).contains(&limit) {
            return Err(CommandError::invalid(
                "diagnostic page limit must be between 1 and 200",
            ));
        }
        let state = sessions
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let session = state.active.as_ref().ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::NotFound,
                "the Logseq import session is not active",
            )
        })?;
        if session.session_uuid != session_uuid {
            return Err(CommandError::conflict(
                "a newer Logseq dry run replaced this import session",
            ));
        }
        let diagnostics = session
            .scan_diagnostics
            .iter()
            .chain(&session.prepared.report.diagnostics)
            .collect::<Vec<_>>();
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(diagnostics.len());
        let end = start.saturating_add(limit as usize).min(diagnostics.len());
        Ok(LogseqImportDiagnosticPage {
            offset: start as u64,
            total: diagnostics.len() as u64,
            diagnostics: diagnostics[start..end]
                .iter()
                .map(|diagnostic| (*diagnostic).clone())
                .collect(),
        })
    }
}

#[tauri::command]
#[specta::specta]
pub async fn prepare_logseq_import(
    app: AppHandle,
    state: State<'_, AppState>,
    sessions: State<'_, LogseqImportSessions>,
    on_progress: Channel<LogseqImportProgress>,
) -> CommandResult<Option<LogseqImportPreview>> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, state, sessions, on_progress);
        Err(CommandError::new(
            CommandErrorCode::Unavailable,
            "direct Logseq folder import is available on desktop only",
        ))
    }

    #[cfg(not(target_os = "android"))]
    {
        let _preparing = begin_preparing(&sessions)?;
        let Some(source_root) = app.dialog().file().blocking_pick_folder() else {
            return Ok(None);
        };
        let source_root = source_root.into_path().map_err(err)?;
        send_indeterminate_import_progress(&on_progress, LogseqImportStage::Scanning);

        let existing_receipt =
            notes_core::external_import_receipt(&state.conn, ExternalImportFormat::Logseq)
                .await
                .map_err(err)?;
        let destination_empty = notes_core::external_import_destination_is_empty(&state.conn)
            .await
            .map_err(err)?;
        let identity = if let Some(receipt) = existing_receipt.as_ref() {
            IdentityContext {
                workspace_uuid: receipt.identity.workspace_uuid,
                import_namespace_uuid: receipt.identity.import_namespace_uuid,
            }
        } else {
            IdentityContext {
                workspace_uuid: db::workspace_uuid(&state.conn).await.map_err(err)?,
                import_namespace_uuid: uuid::Uuid::now_v7(),
            }
        };

        let worker_root = source_root.clone();
        let worker_progress = on_progress.clone();
        let prepared_graph = tauri::async_runtime::spawn_blocking(move || {
            prepare_selected_graph(&worker_root, identity, &worker_progress)
        })
        .await
        .map_err(err)?
        .map_err(map_prepare_graph_error)?;

        let prepared = prepared_graph.prepared;
        let scan_diagnostics = prepared_graph.scan_diagnostics;
        let drawing_conversions = select_drawing_conversions(&state, &source_root, &prepared);
        let destination = if existing_receipt.is_some() {
            LogseqImportDestination::ExistingReceipt
        } else if destination_empty {
            LogseqImportDestination::Empty
        } else {
            LogseqImportDestination::NotEmpty
        };
        let mut blockers = Vec::new();
        if prepared.pages.is_empty() {
            blockers.push(LogseqImportBlocker::EmptyImport);
        }
        if !prepared.is_committable()
            || scan_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        {
            blockers.push(LogseqImportBlocker::BlockingDiagnostics);
        }
        if matches!(destination, LogseqImportDestination::NotEmpty) {
            blockers.push(LogseqImportBlocker::NonEmptyWorkspace);
        }
        if existing_receipt.as_ref().is_some_and(|receipt| {
            receipt.manifest_digest.as_bytes()
                != prepared.provenance.source_manifest.sha256().as_bytes()
        }) {
            blockers.push(LogseqImportBlocker::ExistingImportChanged);
        }
        if validate_destination_aliases(&prepared).is_err() {
            blockers.push(LogseqImportBlocker::AliasCollision);
        }
        let static_commit_allowed = blockers.is_empty();
        match background_ai_safety(&state).await {
            BackgroundAiSafety::Safe => {}
            BackgroundAiSafety::Enabled => blockers.push(LogseqImportBlocker::BackgroundAiEnabled),
            BackgroundAiSafety::Unavailable => {
                blockers.push(LogseqImportBlocker::ServerAiStatusUnavailable)
            }
        }
        let can_commit = blockers.is_empty();
        let session_uuid = uuid::Uuid::now_v7();
        let preview = LogseqImportPreview {
            session_uuid,
            source_name: source_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Logseq graph")
                .to_owned(),
            manifest_sha256: prepared.provenance.source_manifest.sha256().to_string(),
            destination,
            blockers,
            can_commit,
            report: summarize_report(&prepared, &scan_diagnostics, &drawing_conversions),
        };
        sessions
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active = Some(LogseqImportSession {
            session_uuid,
            source_root,
            prepared,
            scan_diagnostics,
            drawing_conversions: drawing_conversions.publication,
            static_commit_allowed,
        });
        send_import_progress(&on_progress, LogseqImportStage::Complete, 1, 1);
        Ok(Some(preview))
    }
}

#[tauri::command]
#[specta::specta]
pub async fn commit_logseq_import(
    app: AppHandle,
    state: State<'_, AppState>,
    sessions: State<'_, LogseqImportSessions>,
    session_uuid: uuid::Uuid,
    on_progress: Channel<LogseqImportProgress>,
) -> CommandResult<LogseqImportCommitResult> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, state, sessions, session_uuid, on_progress);
        Err(CommandError::new(
            CommandErrorCode::Unavailable,
            "direct Logseq folder import is available on desktop only",
        ))
    }

    #[cfg(not(target_os = "android"))]
    {
        ensure_background_ai_disabled(&state).await?;
        let (session, _committing) = begin_commit(&sessions, session_uuid)?;
        if !session.static_commit_allowed || !session.prepared.is_committable() {
            return Err(CommandError::conflict(
                "the Logseq dry run contains blockers and cannot be committed",
            ));
        }
        let current_receipt =
            notes_core::external_import_receipt(&state.conn, ExternalImportFormat::Logseq)
                .await
                .map_err(err)?;
        if let Some(receipt) = &current_receipt {
            let provenance = &session.prepared.provenance;
            if receipt.manifest_digest.as_bytes() != provenance.source_manifest.sha256().as_bytes()
                || receipt.planner_version != provenance.planner_version
                || receipt.identity.workspace_uuid != session.prepared.identity.workspace_uuid
                || receipt.identity.import_namespace_uuid
                    != session.prepared.identity.import_namespace_uuid
            {
                return Err(CommandError::conflict(
                    "the committed Logseq import receipt no longer matches this dry run",
                ));
            }
        } else if !notes_core::external_import_destination_is_empty(&state.conn)
            .await
            .map_err(err)?
        {
            return Err(CommandError::conflict(
                "Logseq import requires an empty workspace; local data changed after the dry run",
            ));
        }
        let first_import = current_receipt.is_none();
        let open_page_uuid = preferred_open_page(&session.prepared);
        let source_root = session.source_root;
        let prepared = session.prepared;
        let scan_diagnostics = session.scan_diagnostics;
        let drawing_conversions = session.drawing_conversions;
        let worker_progress = on_progress.clone();
        send_import_progress(&on_progress, LogseqImportStage::Verifying, 0, 1);
        let (batch, blobs) = tauri::async_runtime::spawn_blocking(move || {
            match notes_import::verify_logseq_manifest(
                &source_root,
                &prepared.provenance.source_manifest,
            )? {
                notes_import::ManifestVerification::Exact { .. } => {}
                notes_import::ManifestVerification::Changed { .. } => {
                    return Err(CommitPlanError::SourceChanged);
                }
            }
            send_import_progress(&worker_progress, LogseqImportStage::Verifying, 1, 1);
            send_import_progress(&worker_progress, LogseqImportStage::Materializing, 0, 1);
            let materialized = if let Some(publication) = drawing_conversions.as_ref() {
                notes_import::materialize_source_media_with_drawing_conversions(
                    &prepared,
                    &source_root,
                    publication,
                )?
            } else {
                notes_import::materialize_source_media(&prepared, &source_root)?
            };
            match notes_import::verify_logseq_manifest(
                &source_root,
                &prepared.provenance.source_manifest,
            )? {
                notes_import::ManifestVerification::Exact { .. } => {}
                notes_import::ManifestVerification::Changed { .. } => {
                    return Err(CommitPlanError::SourceChanged);
                }
            }
            let result = build_external_import_batch(prepared, scan_diagnostics, materialized)
                .map_err(CommitPlanError::Adapter)?;
            send_import_progress(&worker_progress, LogseqImportStage::Materializing, 1, 1);
            Ok::<_, CommitPlanError>(result)
        })
        .await
        .map_err(err)?
        .map_err(map_commit_plan_error)?;

        if first_import {
            super::data::write_backup(&app, &state.conn, &state.blob_store, "before-logseq-import")
                .await
                .map_err(err)?;
        }

        let published = std::sync::Arc::new(Mutex::new(Vec::<(BlobHash, u64)>::new()));
        let install_tracker = published.clone();
        let install_store = state.blob_store.clone();
        send_import_progress(
            &on_progress,
            LogseqImportStage::Installing,
            0,
            blobs.len() as u64,
        );
        let install_progress = on_progress.clone();
        let install_result = tauri::async_runtime::spawn_blocking(move || {
            install_import_blobs(&install_store, blobs, &install_tracker, &install_progress)
        })
        .await
        .map_err(err)?;
        if let Err(install_error) = install_result {
            return Err(cleanup_failed_import(
                &state.conn,
                &state.blob_store,
                &published,
                install_error,
            )
            .await);
        }

        send_import_progress(&on_progress, LogseqImportStage::Committing, 0, 1);
        let outcome = match notes_core::apply_external_import(&state.conn, batch).await {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(cleanup_failed_import(
                    &state.conn,
                    &state.blob_store,
                    &published,
                    error,
                )
                .await);
            }
        };
        send_import_progress(&on_progress, LogseqImportStage::Committing, 1, 1);
        let result = match outcome {
            ExternalImportOutcome::Applied {
                receipt_uuid,
                page_count,
                block_count,
                alias_count,
                attachment_count,
                operation_count,
                ..
            } => {
                emit_domain(&app, DomainEvent::WorkspaceChanged);
                LogseqImportCommitResult::Applied {
                    receipt_uuid,
                    page_count,
                    block_count,
                    alias_count,
                    attachment_count,
                    operation_count,
                    open_page_uuid,
                }
            }
            ExternalImportOutcome::ExactNoOp { receipt_uuid } => {
                LogseqImportCommitResult::ExactNoOp {
                    receipt_uuid,
                    open_page_uuid,
                }
            }
        };
        send_import_progress(&on_progress, LogseqImportStage::Complete, 1, 1);
        Ok(result)
    }
}

#[cfg(not(target_os = "android"))]
enum BackgroundAiSafety {
    Safe,
    Enabled,
    Unavailable,
}

#[cfg(not(target_os = "android"))]
async fn background_ai_safety(state: &AppState) -> BackgroundAiSafety {
    let Some(remote) = state.remote_ai.as_ref() else {
        return BackgroundAiSafety::Safe;
    };
    let Ok(status) = remote.ai_status().await else {
        return BackgroundAiSafety::Unavailable;
    };
    if status.settings.automatic_embeddings || status.settings.entity_extraction {
        BackgroundAiSafety::Enabled
    } else {
        BackgroundAiSafety::Safe
    }
}

#[cfg(not(target_os = "android"))]
async fn ensure_background_ai_disabled(state: &AppState) -> CommandResult<()> {
    match background_ai_safety(state).await {
        BackgroundAiSafety::Safe => Ok(()),
        BackgroundAiSafety::Enabled => Err(CommandError::conflict(
            "disable automatic embeddings and entity extraction on the notes server before importing a Logseq graph",
        )),
        BackgroundAiSafety::Unavailable => Err(CommandError::new(
            CommandErrorCode::Unavailable,
            "the notes server AI status is unavailable; reconnect before importing to guarantee that paid background indexing is disabled",
        )),
    }
}

#[tauri::command]
#[specta::specta]
pub fn discard_logseq_import(
    sessions: State<'_, LogseqImportSessions>,
    session_uuid: uuid::Uuid,
) -> bool {
    #[cfg(target_os = "android")]
    {
        let _ = (sessions, session_uuid);
        false
    }

    #[cfg(not(target_os = "android"))]
    {
        let mut state = sessions
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state
            .active
            .as_ref()
            .is_some_and(|session| session.session_uuid == session_uuid)
        {
            state.active.take();
            true
        } else {
            false
        }
    }
}

#[cfg(not(target_os = "android"))]
fn begin_preparing(sessions: &LogseqImportSessions) -> CommandResult<PreparingGuard<'_>> {
    let mut state = sessions
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state.preparing || state.committing.is_some() {
        return Err(CommandError::conflict(
            "another Logseq import operation is already running",
        ));
    }
    state.preparing = true;
    drop(state);
    Ok(PreparingGuard { sessions })
}

#[cfg(not(target_os = "android"))]
fn begin_commit(
    sessions: &LogseqImportSessions,
    session_uuid: uuid::Uuid,
) -> CommandResult<(LogseqImportSession, CommittingGuard<'_>)> {
    let mut state = sessions
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state.preparing || state.committing.is_some() {
        return Err(CommandError::conflict(
            "another Logseq import operation is already running",
        ));
    }
    let session = match state.active.as_ref() {
        Some(session) if session.session_uuid == session_uuid => state
            .active
            .take()
            .expect("the matching import session was present"),
        Some(_) => Err(CommandError::conflict(
            "a newer Logseq dry run replaced this import session",
        ))?,
        None => Err(CommandError::new(
            CommandErrorCode::NotFound,
            "the Logseq import session is not active",
        ))?,
    };
    state.committing = Some(session_uuid);
    drop(state);
    Ok((session, CommittingGuard { sessions }))
}

#[cfg(not(target_os = "android"))]
fn preferred_open_page(prepared: &PreparedImport) -> Option<uuid::Uuid> {
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    prepared
        .pages
        .iter()
        .find(
            |page| matches!(&page.kind, ImportPageKind::Journal { date } if date.as_str() == today),
        )
        .or_else(|| prepared.pages.first())
        .map(|page| page.uuid)
}

#[cfg(not(target_os = "android"))]
fn build_external_import_batch(
    mut prepared: PreparedImport,
    scan_diagnostics: Vec<ImportDiagnostic>,
    materialized: MediaMaterializationPlan,
) -> anyhow::Result<(ExternalImportBatch, Vec<MaterializedMediaBlob>)> {
    apply_media_rewrites(&mut prepared, &materialized)?;
    let audit_payload = serde_json::json!({
        "preparedProvenance": &prepared.provenance,
        "report": &prepared.report,
        "scanDiagnostics": scan_diagnostics,
        "mediaDiagnostics": &materialized.diagnostics,
        "preservedMediaReferences": &materialized.preserved_references,
    });
    let mut aliases = destination_aliases(&prepared)?;

    let pages = prepared
        .pages
        .iter()
        .map(|page| -> anyhow::Result<ExternalImportPage> {
            let id = ExternalPageId::new(page.uuid)?;
            let kind = match &page.kind {
                ImportPageKind::Note { title } => ExternalImportPageKind::Note {
                    title: title.clone(),
                },
                ImportPageKind::Journal { date } => ExternalImportPageKind::Journal {
                    date: date.as_str().parse::<JournalDate>()?,
                },
            };
            let blocks = page
                .blocks
                .iter()
                .map(|block| -> anyhow::Result<_> {
                    let ordinal = usize::try_from(block.sibling_ordinal)
                        .context("Logseq sibling ordinal does not fit this platform")?
                        .checked_add(1)
                        .context("Logseq sibling ordinal overflowed")?;
                    Ok(notes_core::ExternalImportBlock {
                        id: ExternalBlockId::new(block.uuid)?,
                        parent_id: block
                            .parent_block_uuid
                            .map(ExternalBlockId::new)
                            .transpose()?,
                        order_key: OrderKey::from_ordinal(ordinal),
                        style: imported_block_style(block),
                        markdown: block.markdown.clone(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(ExternalImportPage {
                id,
                kind,
                layout: PageLayout::Outline,
                aliases: aliases.remove(&page.uuid).unwrap_or_default(),
                blocks,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    anyhow::ensure!(
        aliases.is_empty(),
        "import aliases refer to an unknown page"
    );

    let attachments = materialized
        .attachments
        .into_iter()
        .map(|attachment| -> anyhow::Result<_> {
            let owner = match attachment.owner {
                ImportMediaOwner::Page { page_uuid } => {
                    ExternalImportAttachmentOwner::Page(ExternalPageId::new(page_uuid)?)
                }
                ImportMediaOwner::Block { block_uuid } => {
                    ExternalImportAttachmentOwner::Block(ExternalBlockId::new(block_uuid)?)
                }
            };
            Ok(ExternalImportAttachment {
                attachment_uuid: attachment.attachment_uuid,
                owner,
                blob_hash: BlobHash::from_bytes(*attachment.sha256.as_bytes()),
                filename: attachment.filename,
                mime: attachment.mime,
                size: attachment.size_bytes,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let provenance = ExternalImportProvenance {
        format: ExternalImportFormat::Logseq,
        manifest_digest: ExternalImportDigest::from_bytes(
            *prepared.provenance.source_manifest.sha256().as_bytes(),
        ),
        planner_version: prepared.provenance.planner_version,
        identity: ExternalImportIdentityContext {
            workspace_uuid: prepared.identity.workspace_uuid,
            import_namespace_uuid: prepared.identity.import_namespace_uuid,
        },
        payload: audit_payload,
    };
    Ok((
        ExternalImportBatch {
            provenance,
            pages,
            attachments,
        },
        materialized.blobs,
    ))
}

#[cfg(not(target_os = "android"))]
fn validate_destination_aliases(prepared: &PreparedImport) -> anyhow::Result<()> {
    destination_aliases(prepared).map(|_| ())
}

#[cfg(not(target_os = "android"))]
fn destination_aliases(
    prepared: &PreparedImport,
) -> anyhow::Result<BTreeMap<uuid::Uuid, Vec<PageAlias>>> {
    let page_kinds = prepared
        .pages
        .iter()
        .map(|page| (page.uuid, &page.kind))
        .collect::<BTreeMap<_, _>>();
    let mut owners = BTreeMap::<String, uuid::Uuid>::new();
    for page in &prepared.pages {
        if let ImportPageKind::Note { title } = &page.kind {
            let canonical = PageAlias::new(title)?;
            owners.insert(canonical.as_str().to_owned(), page.uuid);
        }
    }
    let mut result = BTreeMap::<uuid::Uuid, Vec<PageAlias>>::new();
    for (source_alias, page_uuid) in &prepared.identity_maps.page_titles {
        let kind = page_kinds
            .get(page_uuid)
            .context("prepared alias refers to an unknown page")?;
        let alias = PageAlias::new(source_alias)?;
        let is_redundant = match **kind {
            ImportPageKind::Note { ref title } => PageAlias::new(title)?.as_str() == alias.as_str(),
            ImportPageKind::Journal { .. } => false,
        };
        if is_redundant {
            continue;
        }
        if let Some(previous) = owners.insert(alias.as_str().to_owned(), *page_uuid)
            && previous != *page_uuid
        {
            anyhow::bail!(
                "Logseq aliases collide after destination normalization: {}",
                alias.as_str()
            );
        }
        result.entry(*page_uuid).or_default().push(alias);
    }
    for aliases in result.values_mut() {
        aliases.sort();
        aliases.dedup();
    }
    Ok(result)
}

#[cfg(not(target_os = "android"))]
fn imported_block_style(block: &notes_import::ImportBlock) -> BlockStyle {
    if let Some(task) = &block.task {
        return BlockStyle::task(match task.target_state {
            ImportTaskState::Todo => TaskState::Todo,
            ImportTaskState::Doing => TaskState::Doing,
            ImportTaskState::Now => TaskState::Now,
            ImportTaskState::Later => TaskState::Later,
            ImportTaskState::Done => TaskState::Done,
            ImportTaskState::Waiting => TaskState::Waiting,
            ImportTaskState::Cancelled => TaskState::Cancelled,
        });
    }
    match block.presentation {
        ImportBlockPresentation::Paragraph => BlockStyle::Paragraph,
        ImportBlockPresentation::Bullet => BlockStyle::Bullet,
        ImportBlockPresentation::Numbered => BlockStyle::Numbered,
    }
}

#[cfg(not(target_os = "android"))]
fn apply_media_rewrites(
    prepared: &mut PreparedImport,
    materialized: &MediaMaterializationPlan,
) -> anyhow::Result<()> {
    let mut blocks = HashMap::new();
    for (page_index, page) in prepared.pages.iter().enumerate() {
        for (block_index, block) in page.blocks.iter().enumerate() {
            anyhow::ensure!(
                blocks
                    .insert(block.uuid, (page_index, block_index))
                    .is_none(),
                "prepared import contains a duplicate block UUID"
            );
        }
    }
    for rewrite in &materialized.rewrites {
        let reference_index = usize::try_from(rewrite.reference_index)
            .context("media reference index does not fit this platform")?;
        let reference = prepared
            .media_references
            .get(reference_index)
            .context("media rewrite refers to a missing prepared reference")?;
        let &(page_index, block_index) = blocks
            .get(&rewrite.markdown_block_uuid)
            .context("media rewrite refers to a missing Markdown block")?;
        let markdown = &mut prepared.pages[page_index].blocks[block_index].markdown;
        let start = usize::try_from(rewrite.markdown_range.start_byte)
            .context("media rewrite start does not fit this platform")?;
        let end = usize::try_from(rewrite.markdown_range.end_byte)
            .context("media rewrite end does not fit this platform")?;
        anyhow::ensure!(
            start <= end
                && markdown.get(start..end) == Some(reference.owner_markdown_spelling.as_str()),
            "prepared Markdown changed before a media rewrite"
        );
        markdown.replace_range(start..end, &rewrite.replacement_markdown);
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn install_import_blobs(
    store: &BlobStore,
    blobs: Vec<MaterializedMediaBlob>,
    published: &Mutex<Vec<(BlobHash, u64)>>,
    progress: &Channel<LogseqImportProgress>,
) -> anyhow::Result<()> {
    let total = blobs.len() as u64;
    for (index, blob) in blobs.into_iter().enumerate() {
        let hash = BlobHash::from_bytes(*blob.sha256.as_bytes());
        let installed = store.install_reader(blob.bytes.as_ref(), hash, blob.size_bytes)?;
        anyhow::ensure!(
            installed.blob.size == blob.size_bytes,
            "installed Logseq media blob has the wrong size"
        );
        if installed.outcome == InstallOutcome::Installed {
            published
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((hash, blob.size_bytes));
        }
        send_import_progress(
            progress,
            LogseqImportStage::Installing,
            index as u64 + 1,
            total,
        );
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
async fn cleanup_failed_import(
    connection: &Connection,
    store: &BlobStore,
    published: &Mutex<Vec<(BlobHash, u64)>>,
    error: impl Into<anyhow::Error>,
) -> CommandError {
    let error = error.into();
    let installed = published
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let sizes = installed.iter().copied().collect::<BTreeMap<_, _>>();
    let candidates = installed.into_iter().map(|(hash, _)| hash).collect();
    let cleanup_store = store.clone();
    let cleanup = db::cleanup_unreferenced_attachment_blobs(connection, candidates, move |hash| {
        let size = sizes
            .get(&hash)
            .copied()
            .context("missing published Logseq blob size")?;
        cleanup_store.remove_verified(hash, size)?;
        Ok(())
    })
    .await;
    match cleanup {
        Ok(_) => err(error),
        Err(cleanup_error) => err(error.context(format!(
            "Logseq import failed, and orphan blob cleanup also failed: {cleanup_error:#}"
        ))),
    }
}

#[cfg(not(target_os = "android"))]
fn prepare_selected_graph(
    source_root: &Path,
    identity: IdentityContext,
    progress: &Channel<LogseqImportProgress>,
) -> Result<PreparedGraph, PrepareGraphError> {
    let scan = notes_import::scan_logseq_graph(source_root)?;
    let document_entries = scan
        .manifest
        .entries()
        .iter()
        .filter(|entry| {
            matches!(entry.kind, SourceKind::Page | SourceKind::Journal)
                && entry.document_format == Some(DocumentFormat::Markdown)
        })
        .collect::<Vec<_>>();
    let total = document_entries.len() as u64;
    send_import_progress(progress, LogseqImportStage::Parsing, 0, total);
    let mut documents = Vec::with_capacity(document_entries.len());
    for (index, entry) in document_entries.into_iter().enumerate() {
        let bytes = read_manifest_document(source_root, entry).map_err(|source| {
            PrepareGraphError::DocumentRead {
                relative_path: entry.relative_path.clone(),
                source,
            }
        })?;
        documents.push(notes_import::parse_logseq_markdown(
            entry,
            &bytes,
            scan.manifest.config(),
        )?);
        let completed = index as u64 + 1;
        if completed == total || completed.is_multiple_of(32) {
            send_import_progress(progress, LogseqImportStage::Parsing, completed, total);
        }
    }
    match notes_import::verify_logseq_manifest(source_root, &scan.manifest)? {
        notes_import::ManifestVerification::Exact { .. } => {}
        notes_import::ManifestVerification::Changed { .. } => {
            return Err(PrepareGraphError::SourceChanged);
        }
    }
    send_import_progress(progress, LogseqImportStage::Preparing, 0, 1);
    let prepared = notes_import::prepare_import(&documents, identity, &scan.manifest)?;
    send_import_progress(progress, LogseqImportStage::Preparing, 1, 1);
    Ok(PreparedGraph {
        prepared,
        scan_diagnostics: scan.diagnostics,
    })
}

#[cfg(not(target_os = "android"))]
fn map_prepare_graph_error(error: PrepareGraphError) -> CommandError {
    let code = match &error {
        PrepareGraphError::SourceChanged => CommandErrorCode::Conflict,
        PrepareGraphError::Scan(scan)
            if matches!(scan.code(), notes_import::ScanErrorCode::SourceChanged) =>
        {
            CommandErrorCode::Conflict
        }
        PrepareGraphError::Parse(parse)
            if matches!(
                parse.code(),
                notes_import::LogseqParseErrorCode::SourceSizeMismatch
                    | notes_import::LogseqParseErrorCode::SourceHashMismatch
            ) =>
        {
            CommandErrorCode::Conflict
        }
        PrepareGraphError::DocumentRead { source, .. }
            if source
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            CommandErrorCode::Conflict
        }
        PrepareGraphError::Scan(_)
        | PrepareGraphError::DocumentRead { .. }
        | PrepareGraphError::Parse(_)
        | PrepareGraphError::Prepare(_) => CommandErrorCode::InvalidInput,
    };
    CommandError::new(code, error.to_string())
}

#[cfg(not(target_os = "android"))]
fn map_commit_plan_error(error: CommitPlanError) -> CommandError {
    let code = match &error {
        CommitPlanError::Scan(_) | CommitPlanError::SourceChanged => CommandErrorCode::Conflict,
        CommitPlanError::Materialize(materialize)
            if matches!(
                materialize.code(),
                notes_import::MaterializeMediaErrorCode::InvalidLimits
                    | notes_import::MaterializeMediaErrorCode::InvalidPreparedPlan
            ) =>
        {
            CommandErrorCode::Internal
        }
        CommitPlanError::Materialize(_) => CommandErrorCode::Conflict,
        CommitPlanError::Adapter(_) => CommandErrorCode::Internal,
    };
    CommandError::new(code, error.to_string())
}

#[cfg(not(target_os = "android"))]
fn read_manifest_document(
    source_root: &Path,
    entry: &notes_import::ManifestEntry,
) -> anyhow::Result<Vec<u8>> {
    let path = source_root.join(&entry.relative_path);
    let mut file = File::open(&path)
        .with_context(|| format!("opening imported document {}", entry.relative_path))?;
    let maximum = entry
        .size_bytes
        .checked_add(1)
        .context("source document size is too large")?;
    let mut bytes = Vec::new();
    let mut limited = (&mut file).take(maximum);
    let mut buffer = [0_u8; DOCUMENT_READ_BUFFER_BYTES];
    loop {
        let read = limited
            .read(&mut buffer)
            .with_context(|| format!("reading imported document {}", entry.relative_path))?;
        if read == 0 {
            break;
        }
        bytes
            .try_reserve(read)
            .context("allocating imported document buffer")?;
        bytes.extend_from_slice(&buffer[..read]);
    }
    anyhow::ensure!(
        bytes.len() as u64 == entry.size_bytes,
        "imported document {} changed after scanning",
        entry.relative_path
    );
    Ok(bytes)
}

#[cfg(not(target_os = "android"))]
struct DrawingConversionSelection {
    publication: Option<DrawingConversionPublication>,
    state: LogseqDrawingConversionState,
    prepared_count: u64,
    preserved_count: u64,
}

#[cfg(not(target_os = "android"))]
fn select_drawing_conversions(
    state: &AppState,
    source_root: &Path,
    prepared: &PreparedImport,
) -> DrawingConversionSelection {
    let total = prepared.report.legacy_excalidraw_count;
    let publication_root = state
        .blob_store
        .root()
        .join("logseq-drawing-conversions")
        .join(prepared.provenance.source_manifest.sha256().to_string());
    let publication = match notes_import::load_drawing_conversion_publication(&publication_root) {
        Ok(publication) => publication,
        Err(error)
            if matches!(
                error.code(),
                LoadDrawingConversionErrorCode::RootNotFound
                    | LoadDrawingConversionErrorCode::BundleNotFound
            ) =>
        {
            return DrawingConversionSelection {
                publication: None,
                state: LogseqDrawingConversionState::Absent,
                prepared_count: 0,
                preserved_count: total,
            };
        }
        Err(error) => {
            tracing::warn!(
                code = ?error.code(),
                "ignoring an invalid Logseq drawing conversion publication"
            );
            return DrawingConversionSelection {
                publication: None,
                state: LogseqDrawingConversionState::Invalid,
                prepared_count: 0,
                preserved_count: total,
            };
        }
    };
    let source_root = match std::fs::canonicalize(source_root) {
        Ok(root) => root,
        Err(error) => {
            tracing::warn!(%error, "could not canonicalize the Logseq graph for drawing conversion");
            return DrawingConversionSelection {
                publication: None,
                state: LogseqDrawingConversionState::Invalid,
                prepared_count: 0,
                preserved_count: total,
            };
        }
    };
    if publication.bundle().source_root.manifest_sha256
        != prepared.provenance.source_manifest.sha256()
        || publication.root().starts_with(&source_root)
        || source_root.starts_with(publication.root())
    {
        tracing::warn!("ignoring a Logseq drawing conversion publication for another source");
        return DrawingConversionSelection {
            publication: None,
            state: LogseqDrawingConversionState::Invalid,
            prepared_count: 0,
            preserved_count: total,
        };
    }
    let entries = publication
        .bundle()
        .drawings
        .iter()
        .map(|entry| (entry.source.relative_path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let prepared_count = prepared
        .media_references
        .iter()
        .filter(|reference| reference.kind == ImportMediaKind::LegacyExcalidraw)
        .filter(|reference| {
            let ImportMediaResolution::LocalManifest {
                relative_path,
                size_bytes,
                sha256,
            } = &reference.resolution
            else {
                return false;
            };
            entries.get(relative_path.as_str()).is_some_and(|entry| {
                entry.source.size_bytes == *size_bytes
                    && entry.source.sha256 == *sha256
                    && entry.status == DrawingConversionStatus::Converted
            })
        })
        .count() as u64;
    DrawingConversionSelection {
        publication: Some(publication),
        state: LogseqDrawingConversionState::Prepared,
        prepared_count,
        preserved_count: total.saturating_sub(prepared_count),
    }
}

#[cfg(not(target_os = "android"))]
fn summarize_report(
    prepared: &PreparedImport,
    scan_diagnostics: &[ImportDiagnostic],
    drawing_conversions: &DrawingConversionSelection,
) -> LogseqImportReportSummary {
    let report = &prepared.report;
    let diagnostic_count = report.diagnostics.len() as u64 + scan_diagnostics.len() as u64;
    let warning_count = report
        .diagnostics
        .iter()
        .chain(scan_diagnostics)
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
        .count() as u64;
    let diagnostics = scan_diagnostics
        .iter()
        .chain(&report.diagnostics)
        .take(MAX_PREVIEW_DIAGNOSTICS)
        .cloned()
        .collect();
    LogseqImportReportSummary {
        page_count: report.page_count,
        journal_count: report.journal_count,
        block_count: report.block_count,
        synthetic_preamble_block_count: report.synthetic_preamble_block_count,
        task_count: report.task_count,
        reference_count: report.reference_count,
        resolved_reference_count: report.resolved_reference_count,
        unresolved_reference_count: report.unresolved_reference_count,
        media_reference_count: report.media_reference_count,
        markdown_image_count: report.markdown_image_count,
        drawing_conversion_state: drawing_conversions.state,
        prepared_excalidraw_count: drawing_conversions.prepared_count,
        preserved_excalidraw_count: drawing_conversions.preserved_count,
        local_media_reference_count: report.local_media_reference_count,
        inline_media_reference_count: report.inline_media_reference_count,
        blocked_remote_media_reference_count: report.blocked_remote_media_reference_count,
        missing_media_reference_count: report.missing_media_reference_count,
        blocked_unsafe_media_reference_count: report.blocked_unsafe_media_reference_count,
        unsupported_media_reference_count: report.unsupported_media_reference_count,
        unreferenced_asset_count: report.unreferenced_asset_count,
        unreferenced_drawing_count: report.unreferenced_drawing_count,
        preserved_block_uuid_count: report.preserved_block_uuid_count,
        derived_block_uuid_count: report.derived_block_uuid_count,
        warning_count,
        blocking_diagnostic_count: report.blocking_diagnostic_count,
        diagnostic_count,
        diagnostics_truncated: diagnostic_count > MAX_PREVIEW_DIAGNOSTICS as u64,
        diagnostics,
    }
}

#[cfg(not(target_os = "android"))]
fn send_import_progress(
    channel: &Channel<LogseqImportProgress>,
    stage: LogseqImportStage,
    completed: u64,
    total: u64,
) {
    debug_assert!(completed <= total);
    let _ = channel.send(LogseqImportProgress::Items {
        stage,
        completed,
        total,
    });
}

#[cfg(not(target_os = "android"))]
fn send_indeterminate_import_progress(
    channel: &Channel<LogseqImportProgress>,
    stage: LogseqImportStage,
) {
    let _ = channel.send(LogseqImportProgress::Indeterminate { stage });
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;
    use notes_core::AttachmentOwner;

    fn ignored_progress() -> Channel<LogseqImportProgress> {
        Channel::new(|_| Ok(()))
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/notes-import/tests/fixtures")
            .join(name)
    }

    fn prepared_fixture(name: &str) -> PreparedGraph {
        prepare_selected_graph(
            &fixture(name),
            IdentityContext {
                workspace_uuid: uuid::Uuid::now_v7(),
                import_namespace_uuid: uuid::Uuid::now_v7(),
            },
            &ignored_progress(),
        )
        .expect("prepare fixture")
    }

    fn source_metadata(
        root: &Path,
        manifest: &notes_import::GraphManifest,
    ) -> BTreeMap<String, (u64, std::time::SystemTime)> {
        manifest
            .entries()
            .iter()
            .map(|entry| {
                let metadata = std::fs::symlink_metadata(root.join(&entry.relative_path))
                    .unwrap_or_else(|error| {
                        panic!(
                            "could not inspect source metadata for {}: {error}",
                            entry.relative_path
                        )
                    });
                let modified = metadata.modified().unwrap_or_else(|error| {
                    panic!(
                        "could not read source mtime for {}: {error}",
                        entry.relative_path
                    )
                });
                (entry.relative_path.clone(), (metadata.len(), modified))
            })
            .collect()
    }

    fn deepest_import_page(prepared: &PreparedImport) -> (uuid::Uuid, usize, usize) {
        prepared
            .pages
            .iter()
            .map(|page| {
                let parents = page
                    .blocks
                    .iter()
                    .map(|block| (block.uuid, block.parent_block_uuid))
                    .collect::<HashMap<_, _>>();
                let maximum_depth = page
                    .blocks
                    .iter()
                    .map(|block| {
                        let mut depth = 1_usize;
                        let mut parent = block.parent_block_uuid;
                        while let Some(parent_uuid) = parent {
                            depth += 1;
                            assert!(
                                depth <= page.blocks.len(),
                                "prepared real-corpus page contains a parent cycle"
                            );
                            parent = *parents
                                .get(&parent_uuid)
                                .expect("prepared parent belongs to its page");
                        }
                        depth
                    })
                    .max()
                    .unwrap_or_default();
                (page.uuid, maximum_depth, page.blocks.len())
            })
            .max_by_key(|(_, depth, _)| *depth)
            .expect("prepared corpus contains pages")
    }

    #[test]
    fn adapter_preserves_journals_tasks_tree_styles_and_order() {
        let graph = prepared_fixture("parser");
        let materialized =
            notes_import::materialize_source_media(&graph.prepared, fixture("parser"))
                .expect("materialize fixture");
        let (batch, blobs) =
            build_external_import_batch(graph.prepared, graph.scan_diagnostics, materialized)
                .expect("build core batch");

        assert!(blobs.is_empty());
        assert_eq!(batch.pages.len(), 2);
        assert!(
            batch
                .pages
                .iter()
                .all(|page| page.layout == PageLayout::Outline)
        );
        assert!(
            batch
                .pages
                .iter()
                .any(|page| matches!(page.kind, ExternalImportPageKind::Journal { .. }))
        );
        let blocks = batch
            .pages
            .iter()
            .flat_map(|page| &page.blocks)
            .collect::<Vec<_>>();
        assert!(blocks.iter().any(|block| {
            block.style == BlockStyle::task(TaskState::Todo) && !block.markdown.starts_with("TODO ")
        }));
        assert!(
            blocks
                .iter()
                .any(|block| block.style == BlockStyle::Paragraph)
        );
        assert!(blocks.iter().any(|block| block.style == BlockStyle::Bullet));
        assert!(
            blocks
                .iter()
                .any(|block| block.style == BlockStyle::Numbered)
        );
        assert!(blocks.iter().all(|block| block.order_key.value() > 0));
        assert!(batch.provenance.payload.get("scanDiagnostics").is_some());
        assert!(batch.provenance.payload.get("report").is_some());
    }

    #[test]
    fn destination_alias_validation_catches_nfkc_collisions() {
        let mut graph = prepared_fixture("parser");
        let first = graph.prepared.pages[0].uuid;
        let second = graph.prepared.pages[1].uuid;
        graph
            .prepared
            .identity_maps
            .page_titles
            .insert("1".into(), first);
        graph
            .prepared
            .identity_maps
            .page_titles
            .insert("①".into(), second);

        assert!(destination_aliases(&graph.prepared).is_err());
    }

    #[test]
    fn import_and_core_share_attachment_uuid_contract() {
        let digest = notes_import::Sha256Digest::from_bytes([0x5a; 32]);
        let hash = BlobHash::from_bytes([0x5a; 32]);
        let page_uuid = uuid::Uuid::now_v7();
        let block_uuid = uuid::Uuid::now_v7();

        assert_eq!(
            notes_import::materialized_attachment_uuid(
                ImportMediaOwner::Page { page_uuid },
                digest
            ),
            notes_core::attachment_uuid(AttachmentOwner::Page(page_uuid), &hash)
        );
        assert_eq!(
            notes_import::materialized_attachment_uuid(
                ImportMediaOwner::Block { block_uuid },
                digest
            ),
            notes_core::attachment_uuid(AttachmentOwner::Block(block_uuid), &hash)
        );
    }

    #[test]
    fn session_state_rejects_parallel_prepare_and_consumes_commit_once() {
        let sessions = LogseqImportSessions::default();
        let preparing = begin_preparing(&sessions).expect("begin prepare");
        assert!(begin_preparing(&sessions).is_err());
        drop(preparing);

        let graph = prepared_fixture("basic");
        let session_uuid = uuid::Uuid::now_v7();
        sessions.state.lock().unwrap().active = Some(LogseqImportSession {
            session_uuid,
            source_root: fixture("basic"),
            prepared: graph.prepared,
            scan_diagnostics: graph.scan_diagnostics,
            drawing_conversions: None,
            static_commit_allowed: true,
        });
        let (_session, committing) = begin_commit(&sessions, session_uuid).expect("begin commit");
        assert!(begin_commit(&sessions, session_uuid).is_err());
        drop(committing);
        assert!(begin_commit(&sessions, session_uuid).is_err());
    }

    #[tokio::test]
    #[ignore = "reads ~/Documents/notes and imports it into disposable storage"]
    async fn real_corpus_dry_run_and_disposable_import_are_read_only_and_idempotent() {
        let source_root = PathBuf::from(std::env::var_os("HOME").expect("HOME for local smoke"))
            .join("Documents/notes");
        assert!(source_root.is_dir(), "real Logseq graph is unavailable");
        let database_file = tempfile::NamedTempFile::new().expect("temporary database");
        let connection = db::open(database_file.path()).await.expect("open database");
        let workspace_uuid = db::workspace_uuid(&connection)
            .await
            .expect("workspace UUID");
        let identity = IdentityContext {
            workspace_uuid,
            import_namespace_uuid: uuid::Uuid::now_v7(),
        };
        let before = notes_import::scan_logseq_graph(&source_root).expect("scan source before");
        let source_metadata_before = source_metadata(&source_root, &before.manifest);
        let graph = prepare_selected_graph(&source_root, identity, &ignored_progress())
            .expect("prepare real corpus");
        assert!(graph.prepared.is_committable());
        assert!(!graph.prepared.pages.is_empty());
        let expected_report = graph.prepared.report.clone();
        let expected_journal_count = graph
            .prepared
            .pages
            .iter()
            .filter(|page| matches!(page.kind, ImportPageKind::Journal { .. }))
            .count();
        let (deepest_page_uuid, expected_maximum_depth, deepest_page_block_count) =
            deepest_import_page(&graph.prepared);
        assert!(
            expected_maximum_depth >= 4,
            "the real-corpus smoke must exercise a meaningfully nested page"
        );
        let publication_root =
            PathBuf::from(std::env::var_os("HOME").expect("HOME for drawing publication"))
                .join(".local/share/dev.okhsunrog.tangleaf/logseq-drawing-conversions")
                .join(
                    graph
                        .prepared
                        .provenance
                        .source_manifest
                        .sha256()
                        .to_string(),
                );
        let drawing_publication =
            notes_import::load_drawing_conversion_publication(&publication_root)
                .expect("load the prepared real-corpus drawing publication");
        let converted_drawing_count = drawing_publication
            .bundle()
            .drawings
            .iter()
            .filter(|entry| entry.status == DrawingConversionStatus::Converted)
            .count();
        let converted_drawing_hashes = drawing_publication
            .bundle()
            .drawings
            .iter()
            .filter_map(|entry| entry.output.as_ref().map(|output| output.sha256))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(converted_drawing_count, 4);
        assert_eq!(
            drawing_publication
                .bundle()
                .drawings
                .iter()
                .filter(|entry| entry.status == DrawingConversionStatus::SkippedEmpty)
                .count(),
            1
        );
        let materialized = notes_import::materialize_source_media_with_drawing_conversions(
            &graph.prepared,
            &source_root,
            &drawing_publication,
        )
        .expect("materialize real media and converted drawings");
        assert_eq!(converted_drawing_hashes.len(), converted_drawing_count);
        let materialized_png_hashes = materialized
            .attachments
            .iter()
            .filter(|attachment| attachment.mime == "image/png")
            .map(|attachment| attachment.sha256)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            converted_drawing_hashes.is_subset(&materialized_png_hashes),
            "missing converted drawing hashes; diagnostics: {:?}",
            materialized.diagnostics
        );
        assert!(
            materialized
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.issue
                    != notes_import::MediaMaterializationIssue::DrawingConversionFailed),
            "the verified real-corpus publication must not silently preserve a failed drawing"
        );
        let expected_attachment_count = materialized.attachments.len();
        let expected_blobs = materialized
            .blobs
            .iter()
            .map(|blob| {
                (
                    BlobHash::from_bytes(*blob.sha256.as_bytes()),
                    blob.size_bytes,
                )
            })
            .collect::<Vec<_>>();
        let (batch, blobs) =
            build_external_import_batch(graph.prepared, graph.scan_diagnostics, materialized)
                .expect("build real core batch");
        let blob_directory = tempfile::tempdir().expect("temporary blobs");
        let blob_store = BlobStore::new(blob_directory.path());
        install_import_blobs(
            &blob_store,
            blobs,
            &Mutex::new(Vec::new()),
            &ignored_progress(),
        )
        .expect("install real media");
        let outcome = notes_core::apply_external_import(&connection, batch)
            .await
            .expect("apply real corpus");
        assert!(matches!(
            outcome,
            ExternalImportOutcome::Applied {
                page_count,
                block_count,
                attachment_count,
                ..
            } if page_count == expected_report.page_count
                && block_count == expected_report.block_count
                && attachment_count == expected_attachment_count as u64
        ));
        for (hash, size) in expected_blobs {
            blob_store
                .open_verified(hash, size)
                .expect("every imported source blob remains verified");
        }
        let snapshot = notes_core::export_sync_snapshot(&connection, 0)
            .await
            .expect("export imported workspace snapshot");
        assert_eq!(snapshot.pages.len(), expected_report.page_count as usize);
        assert_eq!(snapshot.blocks.len(), expected_report.block_count as usize);
        assert_eq!(
            snapshot
                .page_identities
                .iter()
                .filter(|identity| matches!(identity.kind, notes_core::PageKind::Journal { .. }))
                .count(),
            expected_journal_count
        );
        assert_eq!(
            snapshot
                .attachments
                .iter()
                .filter(|attachment| attachment.present)
                .count(),
            expected_attachment_count
        );
        assert_eq!(
            db::read_subtree(&connection, deepest_page_uuid, u32::MAX)
                .await
                .expect("read deepest imported page")
                .len(),
            deepest_page_block_count
        );

        let second = prepare_selected_graph(&source_root, identity, &ignored_progress())
            .expect("prepare exact rerun");
        let materialized = notes_import::materialize_source_media_with_drawing_conversions(
            &second.prepared,
            &source_root,
            &drawing_publication,
        )
        .expect("materialize exact rerun with the same drawing publication");
        let (batch, _) =
            build_external_import_batch(second.prepared, second.scan_diagnostics, materialized)
                .expect("build exact rerun");
        assert!(matches!(
            notes_core::apply_external_import(&connection, batch)
                .await
                .expect("exact rerun"),
            ExternalImportOutcome::ExactNoOp { .. }
        ));

        let after = notes_import::scan_logseq_graph(&source_root).expect("scan source after");
        assert_eq!(before.manifest, after.manifest);
        assert_eq!(
            source_metadata_before,
            source_metadata(&source_root, &after.manifest),
            "the import smoke must not change source file sizes or mtimes"
        );
    }
}

use anyhow::Context;
use base64::Engine;
use futures::StreamExt;
use notes_ai::agent::{self, ChatEvent, ChatTurn};
use notes_ai::embed::{EmbedderBackend, RerankBackend};
use notes_core::Connection;
use notes_core::db::{self, Node, SearchHit};
use notes_core::{NodeKind, ReorderDirection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;

pub type CommandResult<T> = Result<T, CommandError>;

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
}

impl CommandError {
    fn from_message(message: String) -> Self {
        let normalized = message.to_ascii_lowercase();
        let code = if normalized.contains("not found")
            || normalized.contains("disappeared")
            || normalized.contains("no containing")
        {
            CommandErrorCode::NotFound
        } else if normalized.contains("must ")
            || normalized.contains("requires ")
            || normalized.contains("required")
            || normalized.contains("invalid")
            || normalized.contains("empty")
            || normalized.contains("limited to")
        {
            CommandErrorCode::InvalidInput
        } else if normalized.contains("cannot")
            || normalized.contains("already")
            || normalized.contains("conflict")
            || normalized.contains("descendant")
        {
            CommandErrorCode::Conflict
        } else if normalized.contains("timeout")
            || normalized.contains("network")
            || normalized.contains("provider")
            || normalized.contains("unavailable")
        {
            CommandErrorCode::Unavailable
        } else {
            CommandErrorCode::Internal
        };
        Self { code, message }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::from_message(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::from_message(message.to_owned())
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

pub struct AppState {
    pub conn: Connection,
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<dyn RerankBackend>,
    pub background_paused: Arc<AtomicBool>,
    pub chat_cancellations: Arc<std::sync::Mutex<HashMap<uuid::Uuid, Arc<AtomicBool>>>>,
}

/// The single frontend invalidation stream for persisted Rust state.
/// Payloads carry affected IDs when a command can identify them; whole-workspace
/// replacements (import/sync) deliberately request a full cache refresh.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainEvent {
    NodeChanged {
        node_uuids: Vec<uuid::Uuid>,
        parent_uuids: Vec<uuid::Uuid>,
    },
    NodeDeleted {
        node_uuids: Vec<uuid::Uuid>,
        parent_uuids: Vec<uuid::Uuid>,
    },
    GraphChanged {
        node_uuids: Vec<uuid::Uuid>,
    },
    HistoryChanged,
    BackgroundStatusChanged,
    SettingsChanged,
    WorkspaceChanged,
}

#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "domain:event")]
pub struct DomainEventMessage(pub DomainEvent);

pub(crate) fn emit_domain(app: &AppHandle, event: DomainEvent) {
    if let Err(error) = DomainEventMessage(event).emit(app) {
        tracing::warn!(%error, "failed to emit domain event");
    }
}

async fn node_uuids_for_ids(
    connection: &Connection,
    ids: impl IntoIterator<Item = i64>,
) -> Vec<uuid::Uuid> {
    match db::node_uuids_for_ids(connection, ids).await {
        Ok(uuids) => uuids,
        Err(error) => {
            tracing::warn!(%error, "failed to resolve node UUIDs for domain event");
            Vec::new()
        }
    }
}

async fn emit_nodes_changed(
    app: &AppHandle,
    connection: &Connection,
    nodes: &[Node],
    additional_parent_ids: impl IntoIterator<Item = i64>,
) {
    let parent_ids = nodes
        .iter()
        .filter_map(|node| node.parent_id)
        .chain(additional_parent_ids);
    emit_domain(
        app,
        DomainEvent::NodeChanged {
            node_uuids: nodes.iter().map(|node| node.uuid).collect(),
            parent_uuids: node_uuids_for_ids(connection, parent_ids).await,
        },
    );
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundStatus {
    pub paused: bool,
    pub embeddings_pending: i64,
    pub embeddings_failed: i64,
    pub extractions_pending: i64,
    pub extractions_failed: i64,
    pub failures: Vec<db::BackgroundFailure>,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProbeRequest {
    protocol: crate::settings::CompletionProtocol,
    base_url: String,
    model: String,
    api_key: Option<String>,
    key_scope: Option<crate::settings::ProviderKeyScope>,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProbeResult {
    capabilities: Vec<CapabilityProbeResult>,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityProbeResult {
    name: ProbeCapability,
    ok: bool,
    latency_ms: u64,
    detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProbeCapability {
    Completion,
    Streaming,
    RequiredTool,
    StructuredOutput,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct StructuredProbe {
    status: String,
}

fn probe_result(
    name: ProbeCapability,
    started: std::time::Instant,
    result: Result<String, impl std::fmt::Display>,
) -> CapabilityProbeResult {
    match result {
        Ok(detail) => CapabilityProbeResult {
            name,
            ok: true,
            latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            detail: detail.chars().take(240).collect(),
        },
        Err(error) => CapabilityProbeResult {
            name,
            ok: false,
            latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            detail: error.to_string().chars().take(500).collect(),
        },
    }
}

/// Registered immediately in setup so the frontend can ask whether the heavy
/// initialization (embedder download, db open) has finished. Without this,
/// commands invoked during the ~minutes-long first-run model fetch fail with
/// an opaque "state not managed" error.
pub struct Startup {
    pub status: Arc<RwLock<StartupStatus>>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StartupStatus {
    Starting { message: String },
    Ready,
    Error { message: String },
}

fn err<E: std::fmt::Display>(error: E) -> CommandError {
    CommandError::from_message(error.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn is_ready(state: State<'_, Startup>) -> bool {
    matches!(
        *state.status.read().unwrap_or_else(|e| e.into_inner()),
        StartupStatus::Ready
    )
}

#[tauri::command]
#[specta::specta]
pub fn startup_status(state: State<'_, Startup>) -> StartupStatus {
    state
        .status
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[tauri::command]
#[specta::specta]
pub fn load_settings(app: AppHandle) -> CommandResult<crate::settings::SettingsSnapshot> {
    crate::settings::load(&app).map_err(err)
}

#[tauri::command]
#[specta::specta]
pub fn save_settings(
    app: AppHandle,
    update: crate::settings::SettingsUpdate,
) -> CommandResult<crate::settings::SettingsSnapshot> {
    let settings = crate::settings::save(&app, update).map_err(err)?;
    emit_domain(&app, DomainEvent::SettingsChanged);
    Ok(settings)
}

#[tauri::command]
#[specta::specta]
pub async fn test_completion_provider(
    request: ProviderProbeRequest,
) -> CommandResult<ProviderProbeResult> {
    let mut config = crate::settings::completion_config_for_probe(
        request.protocol,
        request.base_url,
        request.model,
        request.api_key,
        request.key_scope,
    )
    .map_err(err)?
    .timeout(std::time::Duration::from_secs(20))
    .max_tokens(64);
    config.retry_policy.max_retries = 0;
    let client = llm_relay::LlmClient::new(config).map_err(err)?;
    let mut capabilities = Vec::new();

    let started = std::time::Instant::now();
    let completion = client
        .complete("Reply with exactly: OK", llm_relay::ChatOptions::default())
        .await
        .and_then(|response| {
            let text = response.text();
            if text.trim().is_empty() {
                Err(llm_relay::LlmError::EmptyResponse)
            } else {
                Ok(text)
            }
        });
    capabilities.push(probe_result(
        ProbeCapability::Completion,
        started,
        completion,
    ));

    let started = std::time::Instant::now();
    let streaming = async {
        let mut stream = client
            .chat_stream(
                &[llm_relay::Message::user_text("Reply with exactly: OK")],
                llm_relay::ChatOptions::default(),
            )
            .await?;
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let llm_relay::StreamEvent::TextDelta { text: delta } = event? {
                text.push_str(&delta);
            }
        }
        if text.trim().is_empty() {
            Err(llm_relay::LlmError::EmptyResponse)
        } else {
            Ok(text)
        }
    }
    .await;
    capabilities.push(probe_result(ProbeCapability::Streaming, started, streaming));

    let started = std::time::Instant::now();
    let tools = [llm_relay::ToolDefinition::new(
        "provider_probe",
        "Return the requested provider probe status.",
        serde_json::json!({
            "type": "object",
            "properties": { "status": { "type": "string" } },
            "required": ["status"],
            "additionalProperties": false
        }),
    )];
    let tool_call = client
        .complete(
            "Call provider_probe with status OK.",
            llm_relay::ChatOptions {
                tools: Some(&tools),
                required_tool: Some("provider_probe"),
                ..llm_relay::ChatOptions::default()
            },
        )
        .await
        .and_then(|response| {
            response
                .tool_uses()
                .first()
                .map(|tool| format!("{tool:?}"))
                .ok_or_else(|| llm_relay::LlmError::InvalidStructuredOutput {
                    error: "provider did not return the required tool call".into(),
                    body: response.text(),
                })
        });
    capabilities.push(probe_result(
        ProbeCapability::RequiredTool,
        started,
        tool_call,
    ));

    let started = std::time::Instant::now();
    let structured = client
        .complete_structured::<StructuredProbe>(
            "Return a status field containing exactly OK.",
            "provider_probe",
            None,
        )
        .await
        .map(|response| response.data.status);
    capabilities.push(probe_result(
        ProbeCapability::StructuredOutput,
        started,
        structured,
    ));

    Ok(ProviderProbeResult { capabilities })
}

#[tauri::command]
#[specta::specta]
pub fn restart_app(app: AppHandle) {
    app.restart()
}

#[tauri::command]
#[specta::specta]
pub async fn background_status(state: State<'_, AppState>) -> CommandResult<BackgroundStatus> {
    let queues = db::queue_status(&state.conn).await.map_err(err)?;
    Ok(BackgroundStatus {
        paused: state.background_paused.load(Ordering::Acquire),
        embeddings_pending: queues.embeddings_pending,
        embeddings_failed: queues.embeddings_failed,
        extractions_pending: queues.extractions_pending,
        extractions_failed: queues.extractions_failed,
        failures: queues.failures,
    })
}

#[tauri::command]
#[specta::specta]
pub fn set_background_paused(app: AppHandle, state: State<'_, AppState>, paused: bool) {
    state.background_paused.store(paused, Ordering::Release);
    emit_domain(&app, DomainEvent::BackgroundStatusChanged);
}

#[tauri::command]
#[specta::specta]
pub async fn retry_background_jobs(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    db::retry_background_jobs(&state.conn).await.map_err(err)?;
    emit_domain(&app, DomainEvent::BackgroundStatusChanged);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn clear_background_jobs(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    db::clear_background_jobs(&state.conn).await.map_err(err)?;
    emit_domain(&app, DomainEvent::BackgroundStatusChanged);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn create_node(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: NodeKind,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> CommandResult<Node> {
    let node = db::create_node(&state.conn, kind, title, content, content_json)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn update_node(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> CommandResult<()> {
    db::update_node(&state.conn, id, title, content, content_json)
        .await
        .map_err(err)?;
    let node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .ok_or_else(|| "updated node disappeared".to_string())?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn rename_page(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    title: Option<String>,
) -> CommandResult<Node> {
    let node = db::rename_page(&state.conn, uuid, title)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn create_note(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<db::CreatedNote> {
    db::checkpoint_history(&state.conn, "create note")
        .await
        .map_err(err)?;
    let note = db::create_note(&state.conn).await.map_err(err)?;
    emit_nodes_changed(
        &app,
        &state.conn,
        &[note.page.clone(), note.initial_block.clone()],
        [],
    )
    .await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(note)
}

#[tauri::command]
#[specta::specta]
pub async fn set_block_content(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    block: db::BlockContent,
) -> CommandResult<(Node, u32)> {
    let result = db::set_block_content(&state.conn, uuid, block)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&result.0), []).await;
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub async fn split_block(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    parts: Vec<db::BlockContent>,
) -> CommandResult<Vec<Node>> {
    db::checkpoint_history(&state.conn, "split block")
        .await
        .map_err(err)?;
    let nodes = db::split_block(&state.conn, id, parts).await.map_err(err)?;
    emit_nodes_changed(&app, &state.conn, &nodes, []).await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(nodes)
}

#[tauri::command]
#[specta::specta]
pub async fn link_nodes(
    app: AppHandle,
    state: State<'_, AppState>,
    src: i64,
    dst: i64,
    kind: String,
    weight: Option<f64>,
) -> CommandResult<()> {
    db::link_nodes(&state.conn, src, dst, kind, weight.unwrap_or(1.0))
        .await
        .map_err(err)?;
    emit_domain(
        &app,
        DomainEvent::GraphChanged {
            node_uuids: node_uuids_for_ids(&state.conn, [src, dst]).await,
        },
    );
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_node(state: State<'_, AppState>, id: i64) -> CommandResult<Option<Node>> {
    db::get_node(&state.conn, id).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn neighbors(
    state: State<'_, AppState>,
    id: i64,
    depth: u32,
) -> CommandResult<Vec<Node>> {
    db::neighbors(&state.conn, id, depth).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn list_entities(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> CommandResult<Vec<Node>> {
    db::list_entities(&state.conn, limit.unwrap_or(50))
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn list_pages(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> CommandResult<Vec<Node>> {
    db::list_pages(&state.conn, limit.unwrap_or(200))
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_page(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<bool> {
    let deleted_uuids = db::read_subtree(&state.conn, id, u32::MAX)
        .await
        .map_err(err)?
        .into_iter()
        .map(|node| node.uuid)
        .collect::<Vec<_>>();
    db::checkpoint_history(&state.conn, "delete page")
        .await
        .map_err(err)?;
    write_backup(&app, &state.conn, "before-delete")
        .await
        .map_err(err)?;
    let attachments = db::delete_page(&state.conn, id).await.map_err(err)?;
    if attachments.is_some() {
        // Retain attachment payloads so structural Undo can restore their
        // database nodes. Explicit attachment deletion removes the file.
        emit_domain(
            &app,
            DomainEvent::NodeDeleted {
                node_uuids: deleted_uuids,
                parent_uuids: Vec::new(),
            },
        );
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(attachments.is_some())
}

#[tauri::command]
#[specta::specta]
pub async fn find_backlinks(
    state: State<'_, AppState>,
    id: i64,
    kind: Option<String>,
) -> CommandResult<Vec<Node>> {
    db::find_backlinks(&state.conn, id, kind).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn graph_snapshot(
    state: State<'_, AppState>,
    focus_id: Option<i64>,
) -> CommandResult<db::GraphSnapshot> {
    db::graph_snapshot(&state.conn, focus_id).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn export_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<std::path::PathBuf>> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("notes-rs archive", &["json"])
        .set_file_name(format!(
            "notes-rs-{}.json",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ))
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(err)?;
    let mut archive = db::export_archive(&state.conn).await.map_err(err)?;
    add_archive_files(&app, &mut archive).map_err(err)?;
    let json = serde_json::to_string_pretty(&archive).map_err(err)?;
    std::fs::write(&path, json).map_err(err)?;
    Ok(Some(path))
}

#[tauri::command]
#[specta::specta]
pub async fn import_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<std::path::PathBuf>> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("notes-rs archive", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(err)?;
    let json = std::fs::read_to_string(&path).map_err(err)?;
    let archive: db::DataArchive = serde_json::from_str(&json).map_err(err)?;
    write_backup(&app, &state.conn, "before-import")
        .await
        .map_err(err)?;
    restore_archive(&app, &state.conn, archive)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::WorkspaceChanged);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(Some(path))
}

#[tauri::command]
#[specta::specta]
pub async fn create_backup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    write_backup(&app, &state.conn, "manual").await.map_err(err)
}

async fn write_backup(
    app: &AppHandle,
    connection: &Connection,
    reason: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let directory = app.path().app_data_dir()?.join("backups");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!(
        "notes-rs-{reason}-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let mut archive = db::export_archive(connection).await?;
    add_archive_files(app, &mut archive)?;
    std::fs::write(&path, serde_json::to_string_pretty(&archive)?)?;
    Ok(path)
}

#[tauri::command]
#[specta::specta]
pub fn choose_sync_directory(app: AppHandle) -> CommandResult<Option<std::path::PathBuf>> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .map(|path| path.into_path())
        .transpose()
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn sync_push(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let directory = sync_directory(settings.sync_directory.as_deref()).map_err(err)?;
    std::fs::create_dir_all(&directory).map_err(err)?;
    let mut archive = db::export_archive(&state.conn).await.map_err(err)?;
    add_archive_files(&app, &mut archive).map_err(err)?;
    let path = directory.join("notes-rs-sync.json");
    let temporary = directory.join("notes-rs-sync.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_string_pretty(&archive).map_err(err)?,
    )
    .map_err(err)?;
    std::fs::rename(&temporary, &path).map_err(err)?;
    Ok(path)
}

#[tauri::command]
#[specta::specta]
pub async fn sync_pull(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let path = sync_directory(settings.sync_directory.as_deref())
        .map_err(err)?
        .join("notes-rs-sync.json");
    let archive =
        serde_json::from_str(&std::fs::read_to_string(&path).map_err(err)?).map_err(err)?;
    write_backup(&app, &state.conn, "before-sync-pull")
        .await
        .map_err(err)?;
    restore_archive(&app, &state.conn, archive)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::WorkspaceChanged);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(path)
}

fn sync_directory(value: Option<&std::path::Path>) -> anyhow::Result<std::path::PathBuf> {
    value
        .map(std::path::Path::to_path_buf)
        .context("choose a sync directory in Settings first")
}

#[tauri::command]
#[specta::specta]
pub async fn attach_file(
    app: AppHandle,
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Option<Node>> {
    let Some(source) = app.dialog().file().blocking_pick_file() else {
        return Ok(None);
    };
    let source = source.into_path().map_err(err)?;
    let metadata = source.metadata().map_err(err)?;
    if !metadata.is_file() {
        return Err("attachments must be regular files".into());
    }
    if metadata.len() > 100 * 1024 * 1024 {
        return Err("attachments are limited to 100 MiB".into());
    }
    let title = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "attachment filename is not valid UTF-8".to_string())?
        .to_string();
    let blob_hash = format!("{:x}", Sha256::digest(std::fs::read(&source).map_err(err)?));
    let relative = std::path::PathBuf::from("attachments")
        .join(&blob_hash)
        .join(&title);
    let destination = app.path().app_data_dir().map_err(err)?.join(&relative);
    std::fs::create_dir_all(destination.parent().expect("attachment has parent")).map_err(err)?;
    std::fs::copy(&source, &destination).map_err(err)?;
    match db::create_attachment(
        &state.conn,
        parent_id,
        blob_hash,
        title,
        "application/octet-stream".into(),
        metadata.len(),
    )
    .await
    {
        Ok(node) => Ok(Some(node)),
        Err(error) => {
            let _ = std::fs::remove_file(destination);
            Err(err(error))
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_attachments(
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Vec<Node>> {
    db::list_attachments(&state.conn, parent_id)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<()> {
    let node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .filter(|node| node.kind == NodeKind::Attachment)
        .ok_or_else(|| "attachment not found".to_string())?;
    let path = safe_app_data_path(&app, &node.content).map_err(err)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<bool> {
    write_backup(&app, &state.conn, "before-attachment-delete")
        .await
        .map_err(err)?;
    let Some(node) = db::delete_attachment(&state.conn, id).await.map_err(err)? else {
        return Ok(false);
    };
    let path = safe_app_data_path(&app, &node.content).map_err(err)?;
    let still_referenced = db::attachment_path_ref_count(&state.conn, node.content)
        .await
        .map_err(err)?
        > 0;
    if !still_referenced && path.exists() {
        std::fs::remove_file(&path).map_err(err)?;
    }
    Ok(true)
}

fn add_archive_files(app: &AppHandle, archive: &mut db::DataArchive) -> anyhow::Result<()> {
    for node in archive
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Attachment)
    {
        let path = safe_app_data_path(app, &node.content)?;
        let bytes = std::fs::read(&path)
            .with_context(|| format!("reading attachment {}", path.display()))?;
        archive.files.insert(
            node.content.clone(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
    }
    Ok(())
}

async fn restore_archive(
    app: &AppHandle,
    connection: &Connection,
    archive: db::DataArchive,
) -> anyhow::Result<()> {
    let data_dir = app.path().app_data_dir()?;
    let staging = data_dir.join(format!("attachments-import-{}", uuid::Uuid::new_v4()));
    for (relative, encoded) in &archive.files {
        let relative = safe_relative_path(relative)?;
        let destination = staging.join(relative.strip_prefix("attachments").unwrap_or(&relative));
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            destination,
            base64::engine::general_purpose::STANDARD.decode(encoded)?,
        )?;
    }
    if let Err(error) = db::import_archive(connection, archive).await {
        let _ = std::fs::remove_dir_all(staging);
        return Err(error);
    }
    let attachments = data_dir.join("attachments");
    if attachments.exists() {
        std::fs::remove_dir_all(&attachments)?;
    }
    if staging.exists() {
        std::fs::rename(staging, attachments)?;
    }
    Ok(())
}

fn safe_app_data_path(app: &AppHandle, relative: &str) -> anyhow::Result<std::path::PathBuf> {
    Ok(app
        .path()
        .app_data_dir()?
        .join(safe_relative_path(relative)?))
}

fn safe_relative_path(value: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = std::path::Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("archive contains an unsafe attachment path");
    }
    if !path.starts_with("attachments") {
        anyhow::bail!("attachment paths must be below the attachments directory");
    }
    Ok(path.to_path_buf())
}

#[tauri::command]
#[specta::specta]
pub async fn list_block_children(
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Vec<Node>> {
    db::list_block_children(&state.conn, parent_id)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn create_block(
    app: AppHandle,
    state: State<'_, AppState>,
    parent_id: Option<i64>,
    position: Option<f64>,
    content: String,
    content_json: Option<String>,
) -> CommandResult<Node> {
    db::checkpoint_history(&state.conn, "create block")
        .await
        .map_err(err)?;
    let node = db::create_block(&state.conn, parent_id, position, content, content_json)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn indent_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    let old_parent_id = db::get_node_by_uuid(&state.conn, uuid)
        .await
        .map_err(err)?
        .and_then(|node| node.parent_id);
    db::checkpoint_history(&state.conn, "indent block")
        .await
        .map_err(err)?;
    let node = db::indent_block(&state.conn, uuid).await.map_err(err)?;
    emit_nodes_changed(
        &app,
        &state.conn,
        std::slice::from_ref(&node),
        old_parent_id,
    )
    .await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn outdent_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    let old_parent_id = db::get_node_by_uuid(&state.conn, uuid)
        .await
        .map_err(err)?
        .and_then(|node| node.parent_id);
    db::checkpoint_history(&state.conn, "outdent block")
        .await
        .map_err(err)?;
    let node = db::outdent_block(&state.conn, uuid).await.map_err(err)?;
    emit_nodes_changed(
        &app,
        &state.conn,
        std::slice::from_ref(&node),
        old_parent_id,
    )
    .await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(node)
}

async fn move_block_in_direction(
    app: &AppHandle,
    state: &AppState,
    uuid: uuid::Uuid,
    direction: ReorderDirection,
) -> CommandResult<Node> {
    db::checkpoint_history(&state.conn, "reorder block")
        .await
        .map_err(err)?;
    let node = db::move_block_in_direction(&state.conn, uuid, direction)
        .await
        .map_err(err)?;
    emit_nodes_changed(app, &state.conn, std::slice::from_ref(&node), []).await;
    emit_domain(app, DomainEvent::HistoryChanged);
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn move_block_up(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    move_block_in_direction(&app, &state, uuid, ReorderDirection::Up).await
}

#[tauri::command]
#[specta::specta]
pub async fn move_block_down(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    move_block_in_direction(&app, &state, uuid, ReorderDirection::Down).await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_block(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<bool> {
    let deleted_node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .filter(|node| node.kind == NodeKind::Block);
    let parent_uuids = node_uuids_for_ids(
        &state.conn,
        deleted_node.iter().filter_map(|node| node.parent_id),
    )
    .await;
    db::checkpoint_history(&state.conn, "delete block")
        .await
        .map_err(err)?;
    let deleted = db::delete_block(&state.conn, id).await.map_err(err)?;
    if deleted {
        emit_domain(
            &app,
            DomainEvent::NodeDeleted {
                node_uuids: deleted_node.into_iter().map(|node| node.uuid).collect(),
                parent_uuids,
            },
        );
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(deleted)
}

#[tauri::command]
#[specta::specta]
pub async fn replace_block_refs(
    app: AppHandle,
    state: State<'_, AppState>,
    block_id: i64,
    wikilink_titles: Vec<String>,
    block_uuids: Vec<String>,
) -> CommandResult<u32> {
    let broken = db::replace_block_refs(&state.conn, block_id, wikilink_titles, block_uuids)
        .await
        .map_err(err)?;
    emit_domain(
        &app,
        DomainEvent::GraphChanged {
            node_uuids: node_uuids_for_ids(&state.conn, [block_id]).await,
        },
    );
    Ok(broken)
}

#[tauri::command]
#[specta::specta]
pub async fn get_page_by_title(
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<Option<Node>> {
    db::get_page_by_title(&state.conn, title).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_node_by_uuid(
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Option<Node>> {
    db::get_node_by_uuid(&state.conn, uuid).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_or_create_page_by_title(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<Node> {
    let page = db::get_or_create_page_by_title(&state.conn, title)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&page), []).await;
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn create_page(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<Node> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title is required".into());
    }
    db::checkpoint_history(&state.conn, "create page")
        .await
        .map_err(err)?;
    let page = db::create_node(
        &state.conn,
        NodeKind::Page,
        Some(title),
        String::new(),
        None,
    )
    .await
    .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&page), []).await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn history_status(state: State<'_, AppState>) -> CommandResult<(i64, i64)> {
    db::history_status(&state.conn).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn undo(app: AppHandle, state: State<'_, AppState>) -> CommandResult<bool> {
    let changed = db::undo_history(&state.conn).await.map_err(err)?;
    if changed {
        emit_domain(&app, DomainEvent::WorkspaceChanged);
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(changed)
}

#[tauri::command]
#[specta::specta]
pub async fn redo(app: AppHandle, state: State<'_, AppState>) -> CommandResult<bool> {
    let changed = db::redo_history(&state.conn).await.map_err(err)?;
    if changed {
        emit_domain(&app, DomainEvent::WorkspaceChanged);
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(changed)
}

#[tauri::command]
#[specta::specta]
pub async fn get_containing_page(
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<Option<Node>> {
    db::get_containing_page(&state.conn, id).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_pages_by_title(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<Node>> {
    db::search_pages_by_title(&state.conn, query, limit)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_blocks_fts(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<Node>> {
    db::search_blocks_fts(&state.conn, query, limit)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_fts(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    let limit = validate_search_request(&query, limit)?;
    db::search_fts(&state.conn, query, limit).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_vec(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    let limit = validate_search_request(&query, limit)?;
    let emb = state.embedder.embed_query(query).await.map_err(err)?;
    db::search_vec(&state.conn, emb, limit).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_hybrid(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    let limit = validate_search_request(&query, limit)?;
    let emb = state
        .embedder
        .embed_query(query.clone())
        .await
        .map_err(err)?;
    db::search_hybrid(&state.conn, query, emb, limit)
        .await
        .map_err(err)
}

/// Retrieve via hybrid RRF, then rerank with the configured provider.
#[tauri::command]
#[specta::specta]
pub async fn search_agentic(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    let limit = validate_search_request(&query, limit)?;
    let emb = state
        .embedder
        .embed_query(query.clone())
        .await
        .map_err(err)?;
    // See SearchAgentic for rationale; widening the rerank pool matters even
    // more here because the UI can ask for `limit = 3` and starve the
    // reranker otherwise.
    let pool = limit.saturating_mul(4).max(32);
    let candidates = db::search_hybrid(&state.conn, query.clone(), emb, pool)
        .await
        .map_err(err)?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let docs: Vec<String> = candidates
        .iter()
        .map(|h| {
            let title = h.node.title.as_deref().unwrap_or("");
            format!("{title}\n{}", h.node.content)
        })
        .collect();
    let scored = state.reranker.rerank(query, docs).await.map_err(err)?;
    let mut out: Vec<SearchHit> = scored
        .into_iter()
        .take(limit as usize)
        .filter_map(|(idx, score)| {
            candidates.get(idx).map(|candidate| SearchHit {
                node: candidate.node.clone(),
                score: score as f64,
            })
        })
        .collect();
    out.truncate(limit as usize);
    Ok(out)
}

#[tauri::command]
#[specta::specta]
pub async fn rerank(
    state: State<'_, AppState>,
    query: String,
    documents: Vec<String>,
) -> CommandResult<Vec<(usize, f32)>> {
    if query.trim().is_empty() || query.chars().count() > 4_096 {
        return Err("query must contain 1 to 4096 characters".into());
    }
    if documents.len() > 128 || documents.iter().any(|document| document.len() > 100_000) {
        return Err("reranking accepts at most 128 documents of at most 100000 bytes each".into());
    }
    state.reranker.rerank(query, documents).await.map_err(err)
}

fn validate_search_request(query: &str, limit: u32) -> CommandResult<u32> {
    if query.trim().is_empty() || query.chars().count() > 4_096 {
        return Err("query must contain 1 to 4096 characters".into());
    }
    if !(1..=100).contains(&limit) {
        return Err("limit must be between 1 and 100".into());
    }
    Ok(limit)
}

#[tauri::command]
#[specta::specta]
pub async fn chat(state: State<'_, AppState>, message: String) -> CommandResult<String> {
    agent::run_chat(
        state.conn.clone(),
        state.embedder.clone(),
        state.reranker.clone(),
        message,
    )
    .await
    .map_err(err)
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn chat_stream(
    app: AppHandle,
    state: State<'_, AppState>,
    history: Vec<ChatTurn>,
    message: String,
    allow_writes: bool,
    active_node_id: Option<i64>,
    request_id: uuid::Uuid,
    on_event: Channel<ChatEvent>,
) -> CommandResult<String> {
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .chat_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.insert(request_id, cancellation.clone()).is_some() {
            return Err("a chat request with this ID is already active".into());
        }
    }
    let on_event = Arc::new(on_event);
    let emit = move |ev: ChatEvent| {
        let _ = on_event.send(ev);
    };
    let result = agent::run_chat_stream(
        state.conn.clone(),
        state.embedder.clone(),
        state.reranker.clone(),
        history,
        message,
        allow_writes,
        active_node_id,
        cancellation,
        emit,
    )
    .await;
    state
        .chat_cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&request_id);
    if allow_writes && result.is_ok() {
        emit_domain(&app, DomainEvent::WorkspaceChanged);
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    result.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub fn cancel_chat(state: State<'_, AppState>, request_id: uuid::Uuid) -> bool {
    let active = state
        .chat_cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(cancellation) = active.get(&request_id) {
        cancellation.store(true, Ordering::Release);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_errors_expose_stable_categories() {
        assert!(matches!(
            CommandError::from("title is required").code,
            CommandErrorCode::InvalidInput
        ));
        assert!(matches!(
            CommandError::from("block was not found").code,
            CommandErrorCode::NotFound
        ));
        assert!(matches!(
            CommandError::from("request is already active").code,
            CommandErrorCode::Conflict
        ));
        assert!(matches!(
            CommandError::from("provider unavailable").code,
            CommandErrorCode::Unavailable
        ));
    }
}

use crate::state::{UserRegistry, UserState};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use futures::{SinkExt, StreamExt};
use notes_blob::{BlobStore, BlobStoreError, InstallOutcome, VerifiedBlob};
use notes_core::BlobHash;
use notes_core::db::SearchHit;
use notes_protocol::{
    AcceptedOps, AiIndexStatus, AiProviderProbeResult, AiProviderSettingsUpdate, AiRuntimeSettings,
    ApiErrorCode, ApiErrorDetail, ApiErrorResponse, BootstrapRequest, ChatEvent, ChatTurn,
    ClientMessage, OpsBatch, PushOps, SearchRequest, ServerErrorCode, ServerInfo, ServerMessage,
};
use serde::{Deserialize, Serialize};
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;

const JSON_BODY_LIMIT: usize = 2 * 1024 * 1024;
const MAX_OPS_PAGE: usize = 1_000;

#[derive(Clone)]
pub struct AppState {
    pub registry: UserRegistry,
    pub data_dir: PathBuf,
    pub max_blob_bytes: u64,
    pub ai: Option<Arc<crate::ai::AiRuntime>>,
    pub shutdown: CancellationToken,
}

#[derive(Clone)]
struct AuthenticatedUser(Arc<UserState>);

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: ApiErrorCode,
    message: String,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    sync_format_version: u32,
}

#[derive(Deserialize)]
struct OpsQuery {
    #[serde(default)]
    since: u64,
    #[serde(default = "default_ops_limit")]
    limit: usize,
}

#[derive(Deserialize)]
struct SyncQuery {
    #[serde(default)]
    since: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChatRequest {
    #[serde(default)]
    history: Vec<ChatTurn>,
    message: String,
    #[serde(default)]
    allow_writes: bool,
    active_content_uuid: Option<uuid::Uuid>,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: ApiErrorCode::InvalidRequest,
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: ApiErrorCode::Unauthorized,
            message: "a valid bearer token is required".into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: ApiErrorCode::NotFound,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: ApiErrorCode::Conflict,
            message: message.into(),
        }
    }

    fn too_large(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: ApiErrorCode::PayloadTooLarge,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: ApiErrorCode::Unavailable,
            message: message.into(),
        }
    }

    fn internal(error: anyhow::Error) -> Self {
        tracing::error!(error = ?error, "request failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: ApiErrorCode::Internal,
            message: "the server could not complete the request".into(),
        }
    }

    fn from_domain(error: anyhow::Error) -> Self {
        for cause in error.chain() {
            let Some(core) = cause.downcast_ref::<notes_core::CoreError>() else {
                continue;
            };
            return match core {
                notes_core::CoreError::InvalidInput(message) => Self::bad_request(message.clone()),
                notes_core::CoreError::NotFound(message) => Self::not_found(message.clone()),
                notes_core::CoreError::Conflict(message)
                | notes_core::CoreError::SyncConflict(message) => Self::conflict(message.clone()),
                notes_core::CoreError::Database(_) => Self::internal(error),
            };
        }
        Self::internal(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ApiErrorResponse {
                error: ApiErrorDetail {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"notes-rs\""),
            );
        }
        response
    }
}

pub fn router(state: AppState) -> Router {
    let json_routes = Router::new()
        .route("/v1/ops", get(get_ops).post(push_ops))
        .route("/v1/snapshot", get(get_snapshot))
        .route("/v1/bootstrap", post(bootstrap))
        .route("/v1/info", get(info))
        .route("/v1/ai/status", get(ai_status).put(update_ai_settings))
        .route("/v1/ai/provider", put(update_ai_provider))
        .route("/v1/ai/provider/probe", post(probe_ai_provider))
        .route("/v1/ai/reindex", post(reindex_ai))
        .route("/v1/search", post(search))
        .route("/v1/chat", post(chat))
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT));
    let stream_routes = Router::new()
        .route("/v1/sync", get(sync_socket))
        .route(
            "/v1/blobs/{hash}",
            put(put_blob).get(get_blob).head(head_blob),
        )
        .layer(DefaultBodyLimit::disable());
    let protected = Router::new()
        .merge(json_routes)
        .merge(stream_routes)
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .route("/v1/health", get(health))
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn info(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<ServerInfo>, ApiError> {
    let workspace_uuid = notes_core::db::workspace_uuid(&user.0.notes)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ServerInfo {
        workspace_uuid,
        ai_enabled: state.ai.is_some(),
    }))
}

async fn ai_status(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<AiIndexStatus>, ApiError> {
    let ai = state
        .ai
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    ai.status(&user.0.id)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn update_ai_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(settings): Json<AiRuntimeSettings>,
) -> Result<Json<AiIndexStatus>, ApiError> {
    let ai = state
        .ai
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    ai.update_settings(&user.0.id, settings)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn update_ai_provider(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(settings): Json<AiProviderSettingsUpdate>,
) -> Result<Json<AiIndexStatus>, ApiError> {
    let ai = state
        .ai
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    ai.validate_provider_update(&settings)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    ai.update_provider(&user.0.id, settings)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn probe_ai_provider(
    State(state): State<AppState>,
    Json(settings): Json<AiProviderSettingsUpdate>,
) -> Result<Json<AiProviderProbeResult>, ApiError> {
    let ai = state
        .ai
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    ai.validate_provider_update(&settings)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    ai.probe_provider(settings)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn reindex_ai(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<AiIndexStatus>, ApiError> {
    let ai = state
        .ai
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    ai.reindex(&user.0.id)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn search(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    if request.query.trim().is_empty() || request.query.chars().count() > 4_096 {
        return Err(ApiError::bad_request(
            "query must contain 1 to 4096 characters",
        ));
    }
    if !(1..=100).contains(&request.limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 100"));
    }
    let ai = state
        .ai
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    ai.search(&user.0.id, request.query, request.limit, request.rerank)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn chat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<ChatRequest>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    let ai = state
        .ai
        .clone()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<ChatEvent>();
    let cancelled = CancellationToken::new();
    let cancellation_for_emit = cancelled.clone();
    let error_emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let error_for_emit = error_emitted.clone();
    let user_id = user.0.id.clone();
    tokio::spawn(async move {
        let event_sender = sender.clone();
        let emit = move |event| {
            if matches!(&event, ChatEvent::Error { .. }) {
                error_for_emit.store(true, std::sync::atomic::Ordering::Release);
            }
            if event_sender.send(event).is_err() {
                cancellation_for_emit.cancel();
            }
        };
        if let Err(error) = ai
            .chat(
                &user_id,
                request.history,
                request.message,
                request.allow_writes,
                request.active_content_uuid,
                cancelled,
                emit,
            )
            .await
        {
            if error_emitted.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let _ = sender.send(ChatEvent::Error {
                message: error.to_string(),
            });
        }
    });
    let stream = futures::stream::unfold(receiver, |mut receiver| async move {
        let event = receiver.recv().await?;
        let serialized = serde_json::to_string(&event).unwrap_or_else(|error| {
            serde_json::json!({ "kind": "error", "message": error.to_string() }).to_string()
        });
        Some((
            Ok(Event::default().event("chat").data(serialized)),
            receiver,
        ))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        sync_format_version: notes_sync::FORMAT_VERSION,
    })
}

async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let value = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let user = state
        .registry
        .authenticate(value)
        .ok_or_else(ApiError::unauthorized)?;
    request.extensions_mut().insert(AuthenticatedUser(user));
    Ok(next.run(request).await)
}

async fn get_ops(
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<OpsQuery>,
) -> Result<Json<OpsBatch>, ApiError> {
    let limit = query.limit.clamp(1, MAX_OPS_PAGE);
    let ops = user
        .0
        .oplog
        .ops_since(query.since, limit)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(OpsBatch { ops }))
}

async fn push_ops(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<PushOps>,
) -> Result<Json<AcceptedOps>, ApiError> {
    if request.ops.len() > 256 {
        return Err(ApiError::bad_request(
            "a sync batch cannot contain more than 256 operations",
        ));
    }
    validate_operation_blobs(&state, &request.ops).await?;
    let ops = user
        .0
        .ingest(request.ops)
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(AcceptedOps { ops }))
}

async fn get_snapshot(
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<notes_sync::SyncSnapshot>, ApiError> {
    user.0
        .snapshot()
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn bootstrap(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<BootstrapRequest>,
) -> Result<Json<notes_sync::SyncSnapshot>, ApiError> {
    validate_snapshot_blobs(&state, &request.snapshot).await?;
    user.0
        .bootstrap(request.snapshot)
        .await
        .map(Json)
        .map_err(ApiError::from_domain)
}

async fn sync_socket(
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<SyncQuery>,
    socket: WebSocketUpgrade,
) -> Response {
    socket
        .max_message_size(JSON_BODY_LIMIT)
        .on_upgrade(move |socket| websocket_session(user.0, query.since, socket))
}

async fn websocket_session(user: Arc<UserState>, since: u64, socket: WebSocket) {
    let mut operations = user.subscribe();
    let (mut sender, mut receiver) = socket.split();
    match user.oplog.ops_since(since, MAX_OPS_PAGE).await {
        Ok(backlog) if !backlog.is_empty() => {
            if send_server_message(&mut sender, &ServerMessage::Ops { ops: backlog })
                .await
                .is_err()
            {
                return;
            }
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(user = %user.id, error = ?error, "websocket catch-up failed");
            let _ = send_server_error(
                &mut sender,
                ServerErrorCode::CatchUpFailed,
                "could not load operations",
            )
            .await;
            return;
        }
    }

    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(incoming) = incoming else { return; };
                let message = match incoming {
                    Ok(Message::Text(text)) => serde_json::from_str::<ClientMessage>(&text),
                    Ok(Message::Close(_)) => return,
                    Ok(Message::Ping(payload)) => {
                        if sender.send(Message::Pong(payload)).await.is_err() { return; }
                        continue;
                    }
                    Ok(_) => {
                        if send_server_error(&mut sender, ServerErrorCode::UnsupportedMessage, "send JSON text messages").await.is_err() { return; }
                        continue;
                    }
                    Err(error) => {
                        tracing::debug!(user = %user.id, error = ?error, "websocket receive failed");
                        return;
                    }
                };
                match message {
                    Ok(ClientMessage::Push { ops }) if ops.len() <= 256 => match user.ingest(ops).await {
                        Ok(ops) => {
                            if send_server_message(&mut sender, &ServerMessage::Ack { ops }).await.is_err() { return; }
                        }
                        Err(error) => {
                            tracing::warn!(user = %user.id, error = ?error, "websocket ingest failed");
                            let code = if error.chain().any(|cause| {
                                matches!(
                                    cause.downcast_ref::<notes_core::CoreError>(),
                                    Some(
                                        notes_core::CoreError::Conflict(_)
                                            | notes_core::CoreError::SyncConflict(_)
                                    )
                                )
                            }) {
                                ServerErrorCode::Conflict
                            } else {
                                ServerErrorCode::IngestFailed
                            };
                            if send_server_error(&mut sender, code, "operation batch was rejected").await.is_err() { return; }
                        }
                    },
                    Ok(ClientMessage::Push { .. }) => {
                        if send_server_error(&mut sender, ServerErrorCode::BatchTooLarge, "a batch cannot exceed 256 operations").await.is_err() { return; }
                    }
                    Ok(ClientMessage::Ping) => {
                        if send_server_message(&mut sender, &ServerMessage::Pong).await.is_err() { return; }
                    }
                    Err(_) => {
                        if send_server_error(&mut sender, ServerErrorCode::InvalidMessage, "message does not match the sync protocol").await.is_err() { return; }
                    }
                }
            }
            published = operations.recv() => match published {
                Ok(operation) => {
                    if send_server_message(&mut sender, &ServerMessage::Ops { ops: vec![operation] }).await.is_err() { return; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = send_server_error(&mut sender, ServerErrorCode::ResyncRequired, "the client fell behind; reconnect to catch up").await;
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

async fn send_server_message(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    message: &ServerMessage,
) -> Result<(), axum::Error> {
    let encoded = serde_json::to_string(message).expect("server messages serialize");
    sender.send(Message::Text(encoded.into())).await
}

async fn send_server_error(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    code: ServerErrorCode,
    message: impl Into<String>,
) -> Result<(), axum::Error> {
    send_server_message(
        sender,
        &ServerMessage::Error {
            code,
            message: message.into(),
        },
    )
    .await
}

async fn put_blob(
    State(state): State<AppState>,
    Path(raw_hash): Path<String>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let hash = parse_blob_hash(&raw_hash)?;
    let staging = stage_blob(&state.data_dir, body, state.max_blob_bytes).await?;
    let staging_path = staging.to_path_buf();
    let store = BlobStore::new(state.data_dir.clone());
    let maximum = state.max_blob_bytes;
    let installed =
        tokio::task::spawn_blocking(move || store.install_file(&staging_path, hash, maximum))
            .await
            .map_err(|error| ApiError::internal(error.into()))?
            .map_err(map_blob_install_error)?;
    Ok(match installed.outcome {
        InstallOutcome::Installed => StatusCode::CREATED,
        InstallOutcome::AlreadyPresent => StatusCode::NO_CONTENT,
    })
}

async fn stage_blob(
    data_dir: &FilePath,
    body: Body,
    maximum: u64,
) -> Result<tempfile::TempPath, ApiError> {
    let staging_dir = data_dir.join("blob-staging");
    let (file, staging) = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&staging_dir)?;
        let temporary = tempfile::Builder::new()
            .prefix(".upload-")
            .tempfile_in(staging_dir)?;
        Ok::<_, std::io::Error>(temporary.into_parts())
    })
    .await
    .map_err(|error| ApiError::internal(error.into()))?
    .map_err(|error| ApiError::internal(error.into()))?;
    let mut file = tokio::fs::File::from_std(file);
    let mut stream = body.into_data_stream();
    let mut size = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| ApiError::bad_request(error.to_string()))?;
        size = size
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| ApiError::too_large("blob size overflow"))?;
        if size > maximum {
            return Err(ApiError::too_large(format!(
                "blob exceeds the configured {maximum} byte limit"
            )));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| ApiError::internal(error.into()))?;
    }
    file.flush()
        .await
        .map_err(|error| ApiError::internal(error.into()))?;
    drop(file);
    Ok(staging)
}

async fn head_blob(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(raw_hash): Path<String>,
) -> Result<Response, ApiError> {
    let hash = parse_blob_hash(&raw_hash)?;
    let verified = open_authorized_blob(&state, &user.0, hash).await?;
    Ok(blob_response_headers(verified.blob.size))
}

async fn get_blob(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(raw_hash): Path<String>,
) -> Result<Response, ApiError> {
    let hash = parse_blob_hash(&raw_hash)?;
    let verified = open_authorized_blob(&state, &user.0, hash).await?;
    let mut response = blob_response_headers(verified.blob.size);
    let file = tokio::fs::File::from_std(verified.into_file());
    *response.body_mut() = Body::from_stream(ReaderStream::new(file));
    Ok(response)
}

async fn open_authorized_blob(
    state: &AppState,
    user: &UserState,
    hash: BlobHash,
) -> Result<VerifiedBlob, ApiError> {
    if !user_references_blob(user, &hash).await? {
        return Err(ApiError::not_found("blob not found"));
    }
    let store = BlobStore::new(state.data_dir.clone());
    let maximum = state.max_blob_bytes;
    tokio::task::spawn_blocking(move || store.open_verified(hash, maximum))
        .await
        .map_err(|error| ApiError::internal(error.into()))?
        .map_err(map_blob_open_error)
}

fn blob_response_headers(size: u64) -> Response {
    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).expect("valid content length"),
    );
    response
}

async fn user_references_blob(user: &UserState, hash: &BlobHash) -> Result<bool, ApiError> {
    let hash = *hash;
    user.notes
        .call(move |database| {
            database.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM attachment_lww
                    WHERE blob_hash = ?1
                 )",
                [hash.as_bytes().as_slice()],
                |row| row.get(0),
            )
        })
        .await
        .map_err(ApiError::internal)
}

fn parse_blob_hash(hash: &str) -> Result<BlobHash, ApiError> {
    hash.parse::<BlobHash>()
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

async fn validate_operation_blobs(
    state: &AppState,
    operations: &[notes_core::Op],
) -> Result<(), ApiError> {
    let mut declared = std::collections::BTreeMap::new();
    for operation in operations {
        if let notes_core::OpKind::AttachmentAdd(attachment) = &operation.kind {
            record_declared_blob(&mut declared, attachment.blob_hash, attachment.size)?;
        }
    }
    for (hash, size) in declared {
        validate_declared_blob(state, hash, size).await?;
    }
    Ok(())
}

async fn validate_snapshot_blobs(
    state: &AppState,
    snapshot: &notes_sync::SyncSnapshot,
) -> Result<(), ApiError> {
    let mut declared = std::collections::BTreeMap::new();
    for attachment in snapshot
        .attachments
        .iter()
        .filter(|attachment| attachment.present)
    {
        let size = attachment
            .size
            .ok_or_else(|| ApiError::bad_request("present attachment is missing its size"))?;
        record_declared_blob(&mut declared, attachment.blob_hash, size)?;
    }
    for (hash, size) in declared {
        validate_declared_blob(state, hash, size).await?;
    }
    Ok(())
}

fn record_declared_blob(
    declared: &mut std::collections::BTreeMap<BlobHash, u64>,
    hash: BlobHash,
    size: u64,
) -> Result<(), ApiError> {
    if let Some(previous) = declared.insert(hash, size)
        && previous != size
    {
        return Err(ApiError::conflict(format!(
            "attachment metadata declares inconsistent sizes for blob {hash}"
        )));
    }
    Ok(())
}

async fn validate_declared_blob(
    state: &AppState,
    hash: BlobHash,
    declared_size: u64,
) -> Result<(), ApiError> {
    if declared_size > state.max_blob_bytes {
        return Err(ApiError::bad_request(format!(
            "attachment blob {hash} declares a size above the configured limit"
        )));
    }
    let store = BlobStore::new(state.data_dir.clone());
    let maximum = state.max_blob_bytes;
    let verified = tokio::task::spawn_blocking(move || store.open_verified(hash, maximum))
        .await
        .map_err(|error| ApiError::internal(error.into()))?
        .map_err(|error| map_declared_blob_error(hash, error))?;
    if verified.blob.size != declared_size {
        return Err(ApiError::conflict(format!(
            "attachment blob {hash} has size {}, but metadata declares {declared_size}",
            verified.blob.size
        )));
    }
    Ok(())
}

fn map_declared_blob_error(hash: BlobHash, error: BlobStoreError) -> ApiError {
    match error {
        BlobStoreError::NotFound { .. }
        | BlobStoreError::CorruptBlob { .. }
        | BlobStoreError::ExistingSizeMismatch { .. }
        | BlobStoreError::UnsafeFilesystemEntry { .. }
        | BlobStoreError::TooLarge { .. }
        | BlobStoreError::StoredBlobTooLarge { .. }
        | BlobStoreError::HashMismatch { .. } => {
            ApiError::conflict(format!("attachment blob {hash} is not available"))
        }
        BlobStoreError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            ApiError::conflict(format!("attachment blob {hash} is not available"))
        }
        other => ApiError::internal(other.into()),
    }
}

fn map_blob_install_error(error: BlobStoreError) -> ApiError {
    match error {
        BlobStoreError::HashMismatch { .. } => {
            ApiError::bad_request("request body SHA-256 does not match the blob URL")
        }
        BlobStoreError::TooLarge { limit } => {
            ApiError::too_large(format!("blob exceeds the configured {limit} byte limit"))
        }
        BlobStoreError::CorruptBlob { .. }
        | BlobStoreError::ExistingSizeMismatch { .. }
        | BlobStoreError::UnsafeFilesystemEntry { .. } => {
            ApiError::conflict("an existing stored blob failed integrity verification")
        }
        other => ApiError::internal(other.into()),
    }
}

fn map_blob_open_error(error: BlobStoreError) -> ApiError {
    match error {
        BlobStoreError::NotFound { .. } => ApiError::not_found("blob not found"),
        BlobStoreError::TooLarge { .. } | BlobStoreError::StoredBlobTooLarge { .. } => {
            ApiError::conflict("stored blob exceeds the configured size limit")
        }
        BlobStoreError::CorruptBlob { .. }
        | BlobStoreError::ExistingSizeMismatch { .. }
        | BlobStoreError::HashMismatch { .. }
        | BlobStoreError::UnsafeFilesystemEntry { .. } => {
            ApiError::conflict("stored blob failed integrity verification")
        }
        BlobStoreError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            ApiError::not_found("blob not found")
        }
        other => ApiError::internal(other.into()),
    }
}

const fn default_ops_limit() -> usize {
    256
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerConfig, UserConfig};
    use axum::http::Request;
    use http_body_util::BodyExt;
    use notes_core::{AttachmentOwner, Hlc, Op, OpKind, PageLayout};
    use notes_sync::{AttachmentAdd, AttachmentRemove, PageCreate};
    use tower::ServiceExt;

    const TOKEN: &str = "test-token-with-at-least-thirty-two-characters";
    const OTHER_TOKEN: &str = "other-token-with-at-least-thirty-two-chars";
    const TEST_WORKSPACE_UUID: uuid::Uuid = uuid::Uuid::from_u128(0xC0DE);

    async fn test_app() -> (tempfile::TempDir, Router) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().expect("listen address"),
            log_filter: "info".into(),
            data_dir: directory.path().to_owned(),
            snapshot_every_ops: 2,
            max_blob_bytes: 1024,
            ai: None,
            users: vec![
                UserConfig {
                    id: "owner".into(),
                    tokens: vec![TOKEN.into()],
                },
                UserConfig {
                    id: "other".into(),
                    tokens: vec![OTHER_TOKEN.into()],
                },
            ],
        };
        let state = crate::build_state(&config).await.expect("server state");
        for user in state.registry.users() {
            user.notes
                .call(|database| {
                    database.execute(
                        "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                        [TEST_WORKSPACE_UUID],
                    )?;
                    Ok(())
                })
                .await
                .expect("set deterministic workspace");
        }
        (directory, router(state))
    }

    fn operation(index: u128, kind: OpKind) -> Op {
        let device_id = uuid::Uuid::from_u128(1);
        Op {
            op_id: uuid::Uuid::from_u128(index + 10),
            workspace_uuid: TEST_WORKSPACE_UUID,
            device_id,
            hlc: Hlc::new(index as u64, 0, device_id),
            format_version: notes_sync::FORMAT_VERSION,
            kind,
        }
    }

    fn page_operation(page_uuid: uuid::Uuid) -> Op {
        operation(
            1,
            OpKind::PageCreate(PageCreate {
                uuid: page_uuid,
                kind: notes_core::PageKind::Note,
                title: Some("Synced page".into()),
                layout: PageLayout::Outline,
                created_at: 1,
            }),
        )
    }

    fn authorized(request: axum::http::request::Builder) -> axum::http::request::Builder {
        authorized_as(request, TOKEN)
    }

    fn authorized_as(
        request: axum::http::request::Builder,
        token: &str,
    ) -> axum::http::request::Builder {
        request.header(AUTHORIZATION, format!("Bearer {token}"))
    }

    async fn reference_blob(app: &Router, page_uuid: uuid::Uuid, hash: BlobHash, size: u64) {
        let response = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/ops")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&PushOps {
                            ops: vec![
                                page_operation(page_uuid),
                                operation(
                                    2,
                                    OpKind::AttachmentAdd(AttachmentAdd {
                                        owner: AttachmentOwner::Page(page_uuid),
                                        blob_hash: hash,
                                        filename: "attachment.bin".into(),
                                        mime: "application/octet-stream".into(),
                                        size,
                                    }),
                                ),
                            ],
                        })
                        .expect("reference request JSON"),
                    ))
                    .expect("reference request"),
            )
            .await
            .expect("reference response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn upload_test_blob(app: &Router, contents: &'static [u8]) -> BlobHash {
        let hash = BlobHash::digest(contents);
        let response = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{hash}"))
                    .body(Body::from(contents))
                    .expect("blob upload request"),
            )
            .await
            .expect("blob upload response");
        assert!(matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ));
        hash
    }

    fn assert_no_blob_temporaries(directory: &tempfile::TempDir, hash: BlobHash) {
        let staging = directory.path().join("blob-staging");
        if staging.exists() {
            assert!(
                std::fs::read_dir(&staging)
                    .expect("read blob staging")
                    .next()
                    .is_none(),
                "server staging files must be removed"
            );
        }
        let shard = BlobStore::new(directory.path())
            .path_for(hash)
            .parent()
            .expect("blob shard")
            .to_path_buf();
        if shard.exists() {
            assert!(
                std::fs::read_dir(shard)
                    .expect("read blob shard")
                    .filter_map(Result::ok)
                    .all(|entry| !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".incoming-")),
                "blob-store incoming files must be removed"
            );
        }
    }

    async fn typed_error(response: Response) -> ApiErrorResponse {
        serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .expect("error body")
                .to_bytes(),
        )
        .expect("typed API error")
    }

    #[tokio::test]
    async fn requires_auth_and_deduplicates_http_ingest() {
        let (_directory, app) = test_app().await;
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/ops")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let operation = page_operation(uuid::Uuid::from_u128(100));
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    authorized(Request::builder())
                        .method("POST")
                        .uri("/v1/ops")
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&PushOps {
                                ops: vec![operation.clone()],
                            })
                            .expect("request JSON"),
                        ))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            let accepted: AcceptedOps = serde_json::from_slice(&body).expect("accepted ops");
            assert_eq!(accepted.ops.len(), 1);
            assert_eq!(accepted.ops[0].seq, 1);
        }

        let response = app
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/ops?since=0&limit=10")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let batch: OpsBatch = serde_json::from_slice(&body).expect("ops batch");
        assert_eq!(batch.ops.len(), 1);
        assert_eq!(batch.ops[0].seq, 1);
    }

    #[tokio::test]
    async fn emits_typed_http_errors_and_rejects_conflicting_operation_id_reuse() {
        let (_directory, app) = test_app().await;
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .header(AUTHORIZATION, "Bearer wrong-token")
                    .uri("/v1/info")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            unauthorized.headers().get(WWW_AUTHENTICATE),
            Some(&HeaderValue::from_static("Bearer realm=\"notes-rs\""))
        );
        let body = unauthorized
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let error: ApiErrorResponse = serde_json::from_slice(&body).expect("typed error");
        assert_eq!(error.error.code, ApiErrorCode::Unauthorized);

        let original = page_operation(uuid::Uuid::from_u128(100));
        let accepted = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/ops")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&PushOps {
                            ops: vec![original.clone()],
                        })
                        .expect("request JSON"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(accepted.status(), StatusCode::OK);

        let mut conflicting = original;
        let OpKind::PageCreate(page) = &mut conflicting.kind else {
            panic!("test operation must create a page");
        };
        page.title = Some("Conflicting title".into());
        let conflict = app
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/ops")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&PushOps {
                            ops: vec![conflicting],
                        })
                        .expect("request JSON"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let body = conflict
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let error: ApiErrorResponse = serde_json::from_slice(&body).expect("typed error");
        assert_eq!(error.error.code, ApiErrorCode::Conflict);
        assert!(
            error
                .error
                .message
                .contains("reused with a different payload")
        );
    }

    #[tokio::test]
    async fn reports_server_owned_embedding_contract() {
        let (_directory, app) = test_app().await;
        let response = app
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/info")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let info: ServerInfo = serde_json::from_slice(&body).expect("server info");
        assert_eq!(info.workspace_uuid, TEST_WORKSPACE_UUID);
        assert!(!info.ai_enabled);
    }

    #[tokio::test]
    async fn empty_client_cannot_replace_the_server_workspace_identity() {
        let (_directory, app) = test_app().await;
        let snapshot_response = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/snapshot")
                    .body(Body::empty())
                    .expect("snapshot request"),
            )
            .await
            .expect("snapshot response");
        assert_eq!(snapshot_response.status(), StatusCode::OK);
        let snapshot = serde_json::from_slice(
            &snapshot_response
                .into_body()
                .collect()
                .await
                .expect("snapshot body")
                .to_bytes(),
        )
        .expect("empty server snapshot");

        let response = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/bootstrap")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&BootstrapRequest { snapshot })
                            .expect("bootstrap request JSON"),
                    ))
                    .expect("bootstrap request"),
            )
            .await
            .expect("bootstrap response");
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let info_response = app
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/info")
                    .body(Body::empty())
                    .expect("info request"),
            )
            .await
            .expect("info response");
        let info: ServerInfo = serde_json::from_slice(
            &info_response
                .into_body()
                .collect()
                .await
                .expect("info body")
                .to_bytes(),
        )
        .expect("server info");
        assert_eq!(info.workspace_uuid, TEST_WORKSPACE_UUID);
    }

    #[tokio::test]
    async fn mixed_push_with_missing_blob_is_rejected_without_mutation() {
        let (_directory, app) = test_app().await;
        let page_uuid = uuid::Uuid::from_u128(0xA110);
        let valid_contents = b"installed push attachment";
        let valid_hash = upload_test_blob(&app, valid_contents).await;
        let missing_hash = BlobHash::digest(b"missing push attachment");
        let response = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/ops")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&PushOps {
                            ops: vec![
                                page_operation(page_uuid),
                                operation(
                                    2,
                                    OpKind::AttachmentAdd(AttachmentAdd {
                                        owner: AttachmentOwner::Page(page_uuid),
                                        blob_hash: valid_hash,
                                        filename: "valid.bin".into(),
                                        mime: "application/octet-stream".into(),
                                        size: valid_contents.len() as u64,
                                    }),
                                ),
                                operation(
                                    3,
                                    OpKind::AttachmentAdd(AttachmentAdd {
                                        owner: AttachmentOwner::Page(page_uuid),
                                        blob_hash: missing_hash,
                                        filename: "missing.bin".into(),
                                        mime: "application/octet-stream".into(),
                                        size: 23,
                                    }),
                                ),
                            ],
                        })
                        .expect("mixed push JSON"),
                    ))
                    .expect("mixed push request"),
            )
            .await
            .expect("mixed push response");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            typed_error(response).await.error.code,
            ApiErrorCode::Conflict
        );

        let ops = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/ops?since=0&limit=10")
                    .body(Body::empty())
                    .expect("ops request"),
            )
            .await
            .expect("ops response")
            .into_body()
            .collect()
            .await
            .expect("ops body")
            .to_bytes();
        let ops: OpsBatch = serde_json::from_slice(&ops).expect("ops JSON");
        assert!(
            ops.ops.is_empty(),
            "rejected batch must not append to oplog"
        );

        let snapshot = app
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/snapshot")
                    .body(Body::empty())
                    .expect("snapshot request"),
            )
            .await
            .expect("snapshot response")
            .into_body()
            .collect()
            .await
            .expect("snapshot body")
            .to_bytes();
        let snapshot: notes_sync::SyncSnapshot =
            serde_json::from_slice(&snapshot).expect("snapshot JSON");
        assert!(snapshot.pages.is_empty());
        assert!(snapshot.attachments.is_empty());
    }

    #[tokio::test]
    async fn mixed_bootstrap_with_size_mismatch_is_rejected_without_mutation() {
        let (_directory, app) = test_app().await;
        let valid_contents = b"installed bootstrap attachment";
        let valid_hash = upload_test_blob(&app, valid_contents).await;
        let mismatched_contents = b"mismatched bootstrap attachment";
        let mismatched_hash = upload_test_blob(&app, mismatched_contents).await;
        let page_uuid = uuid::Uuid::from_u128(0xB007);
        let device_id = uuid::Uuid::from_u128(1);
        let hlc = Hlc::new(1, 0, device_id);
        let snapshot = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/snapshot")
                    .body(Body::empty())
                    .expect("source snapshot request"),
            )
            .await
            .expect("source snapshot response")
            .into_body()
            .collect()
            .await
            .expect("source snapshot body")
            .to_bytes();
        let mut snapshot: notes_sync::SyncSnapshot =
            serde_json::from_slice(&snapshot).expect("source snapshot JSON");
        snapshot
            .page_identities
            .push(notes_core::SnapshotPageIdentity {
                uuid: page_uuid,
                kind: notes_core::PageKind::Note,
            });
        snapshot.pages.push(notes_core::SnapshotPage {
            uuid: page_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Bootstrap page".into()),
            layout: PageLayout::Outline,
            title_hlc: Some(hlc.clone()),
            layout_hlc: Some(hlc.clone()),
            existence_hlc: hlc.clone(),
            created_at: 1,
            updated_at: 1,
        });
        snapshot.attachments.extend([
            notes_core::SnapshotAttachment {
                owner: AttachmentOwner::Page(page_uuid),
                blob_hash: valid_hash,
                attachment_uuid: uuid::Uuid::from_u128(0xA1),
                hlc: hlc.clone(),
                present: true,
                filename: Some("valid.bin".into()),
                mime: Some("application/octet-stream".into()),
                size: Some(valid_contents.len() as u64),
            },
            notes_core::SnapshotAttachment {
                owner: AttachmentOwner::Page(page_uuid),
                blob_hash: mismatched_hash,
                attachment_uuid: uuid::Uuid::from_u128(0xA2),
                hlc,
                present: true,
                filename: Some("missing.bin".into()),
                mime: Some("application/octet-stream".into()),
                size: Some(mismatched_contents.len() as u64 + 1),
            },
        ]);

        let response = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/bootstrap")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&BootstrapRequest { snapshot })
                            .expect("mixed bootstrap JSON"),
                    ))
                    .expect("mixed bootstrap request"),
            )
            .await
            .expect("mixed bootstrap response");
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let snapshot = app
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/snapshot")
                    .body(Body::empty())
                    .expect("post-rejection snapshot request"),
            )
            .await
            .expect("post-rejection snapshot response")
            .into_body()
            .collect()
            .await
            .expect("post-rejection snapshot body")
            .to_bytes();
        let snapshot: notes_sync::SyncSnapshot =
            serde_json::from_slice(&snapshot).expect("post-rejection snapshot JSON");
        assert!(snapshot.pages.is_empty());
        assert!(snapshot.attachments.is_empty());
    }

    #[tokio::test]
    async fn concurrent_identical_puts_publish_once_and_clean_all_temporaries() {
        let (directory, app) = test_app().await;
        let contents = b"concurrent durable blob";
        let hash = BlobHash::digest(contents);
        let uploads = (0..8).map(|_| {
            let app = app.clone();
            async move {
                app.oneshot(
                    authorized(Request::builder())
                        .method("PUT")
                        .uri(format!("/v1/blobs/{hash}"))
                        .body(Body::from(contents.as_slice()))
                        .expect("concurrent request"),
                )
                .await
                .expect("concurrent upload response")
                .status()
            }
        });
        let statuses = futures::future::join_all(uploads).await;
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CREATED)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::NO_CONTENT)
                .count(),
            7
        );
        let verified = BlobStore::new(directory.path())
            .open_verified(hash, 1_024)
            .expect("published blob verifies");
        assert_eq!(verified.blob.size, contents.len() as u64);
        assert_no_blob_temporaries(&directory, hash);

        let wrong_hash = BlobHash::digest(b"different expected bytes");
        let rejected = app
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{wrong_hash}"))
                    .body(Body::from(contents.as_slice()))
                    .expect("mismatched request"),
            )
            .await
            .expect("mismatched upload response");
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_no_blob_temporaries(&directory, wrong_hash);
    }

    #[tokio::test]
    async fn corrupt_existing_blob_is_never_overwritten_or_streamed() {
        let (directory, app) = test_app().await;
        let contents = b"expected attachment bytes";
        let corrupt = vec![b'x'; contents.len()];
        let hash = BlobHash::digest(contents);
        BlobStore::new(directory.path())
            .install_reader(contents.as_slice(), hash, 1_024)
            .expect("install referenced blob");
        reference_blob(
            &app,
            uuid::Uuid::from_u128(0xB10B),
            hash,
            contents.len() as u64,
        )
        .await;
        let path = BlobStore::new(directory.path()).path_for(hash);
        std::fs::create_dir_all(path.parent().expect("blob shard")).expect("create blob shard");
        std::fs::write(&path, &corrupt).expect("write corrupt existing blob");

        let upload = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{hash}"))
                    .body(Body::from(contents.as_slice()))
                    .expect("replacement request"),
            )
            .await
            .expect("replacement response");
        assert_eq!(upload.status(), StatusCode::CONFLICT);
        let error = typed_error(upload).await;
        assert_eq!(error.error.code, ApiErrorCode::Conflict);
        assert!(
            !error
                .error
                .message
                .contains(&directory.path().display().to_string())
        );
        assert_eq!(
            std::fs::read(&path).expect("read preserved corrupt blob"),
            corrupt
        );

        for method in ["GET", "HEAD"] {
            let response = app
                .clone()
                .oneshot(
                    authorized(Request::builder())
                        .method(method)
                        .uri(format!("/v1/blobs/{hash}"))
                        .body(Body::empty())
                        .expect("verified read request"),
                )
                .await
                .expect("verified read response");
            assert_eq!(response.status(), StatusCode::CONFLICT);
            if method == "GET" {
                let error = typed_error(response).await;
                assert_eq!(error.error.code, ApiErrorCode::Conflict);
                assert!(!error.error.message.contains(&path.display().to_string()));
            }
        }
        assert_no_blob_temporaries(&directory, hash);
    }

    #[tokio::test]
    async fn oversized_stored_blob_is_rejected_by_get_and_head() {
        let (directory, app) = test_app().await;
        let original = b"small referenced blob";
        let contents = vec![0x5a; 1_025];
        let hash = BlobHash::digest(original);
        BlobStore::new(directory.path())
            .install_reader(original.as_slice(), hash, 1_024)
            .expect("install referenced blob");
        reference_blob(
            &app,
            uuid::Uuid::from_u128(0xB10C),
            hash,
            original.len() as u64,
        )
        .await;
        let path = BlobStore::new(directory.path()).path_for(hash);
        std::fs::create_dir_all(path.parent().expect("blob shard")).expect("create blob shard");
        std::fs::write(&path, contents).expect("write oversized stored blob");

        for method in ["GET", "HEAD"] {
            let response = app
                .clone()
                .oneshot(
                    authorized(Request::builder())
                        .method(method)
                        .uri(format!("/v1/blobs/{hash}"))
                        .body(Body::empty())
                        .expect("oversized read request"),
                )
                .await
                .expect("oversized read response");
            assert_eq!(response.status(), StatusCode::CONFLICT);
            if method == "GET" {
                let error = typed_error(response).await;
                assert_eq!(error.error.code, ApiErrorCode::Conflict);
                assert_eq!(
                    error.error.message,
                    "stored blob exceeds the configured size limit"
                );
                assert!(!error.error.message.contains(&path.display().to_string()));
            }
        }
        assert_no_blob_temporaries(&directory, hash);
    }

    #[tokio::test]
    async fn historical_blob_access_survives_remove_but_remains_user_scoped() {
        let (_directory, app) = test_app().await;
        let page_uuid = uuid::Uuid::from_u128(100);
        let contents = b"portable attachment";
        let hash = BlobHash::digest(contents);
        let upload = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{hash}"))
                    .body(Body::from(contents.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("upload response");
        assert_eq!(upload.status(), StatusCode::CREATED);
        let operations = vec![
            page_operation(page_uuid),
            operation(
                2,
                OpKind::AttachmentAdd(AttachmentAdd {
                    owner: AttachmentOwner::Page(page_uuid),
                    blob_hash: hash,
                    filename: "attachment.txt".into(),
                    mime: "text/plain".into(),
                    size: contents.len() as u64,
                }),
            ),
        ];
        let response = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/ops")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&PushOps { ops: operations }).expect("request JSON"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let uppercase = hash.to_string().to_uppercase();
        let uppercase_upload = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{uppercase}"))
                    .body(Body::from(contents.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("uppercase upload response");
        assert_eq!(uppercase_upload.status(), StatusCode::BAD_REQUEST);
        let uppercase_download = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .uri(format!("/v1/blobs/{uppercase}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("uppercase download response");
        assert_eq!(uppercase_download.status(), StatusCode::BAD_REQUEST);

        let download = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .uri(format!("/v1/blobs/{hash}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("download response");
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(
            download
                .into_body()
                .collect()
                .await
                .expect("download body")
                .to_bytes(),
            contents.as_slice()
        );

        let unrelated_contents = b"unrelated blob";
        let unrelated_hash = BlobHash::digest(unrelated_contents);
        let unrelated_upload = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{unrelated_hash}"))
                    .body(Body::from(unrelated_contents.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("unrelated upload response");
        assert_eq!(unrelated_upload.status(), StatusCode::CREATED);
        let unrelated_download = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .uri(format!("/v1/blobs/{unrelated_hash}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("unrelated download response");
        assert_eq!(unrelated_download.status(), StatusCode::NOT_FOUND);

        let remove = operation(
            3,
            OpKind::AttachmentRemove(AttachmentRemove {
                owner: AttachmentOwner::Page(page_uuid),
                blob_hash: hash,
            }),
        );
        let removed = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/ops")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&PushOps { ops: vec![remove] })
                            .expect("remove request JSON"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("remove response");
        assert_eq!(removed.status(), StatusCode::OK);

        for method in ["GET", "HEAD"] {
            let historical = app
                .clone()
                .oneshot(
                    authorized(Request::builder())
                        .method(method)
                        .uri(format!("/v1/blobs/{hash}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("historical blob response");
            assert_eq!(historical.status(), StatusCode::OK);
        }

        let other_user = app
            .clone()
            .oneshot(
                authorized_as(Request::builder(), OTHER_TOKEN)
                    .uri(format!("/v1/blobs/{hash}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("other user response");
        assert_eq!(other_user.status(), StatusCode::NOT_FOUND);

        let snapshot = app
            .oneshot(
                authorized(Request::builder())
                    .uri("/v1/snapshot")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("snapshot response");
        let body = snapshot
            .into_body()
            .collect()
            .await
            .expect("snapshot body")
            .to_bytes();
        let snapshot: notes_sync::SyncSnapshot = serde_json::from_slice(&body).expect("snapshot");
        assert_eq!(snapshot.seq, 3);
        assert_eq!(snapshot.pages.len(), 1);
        assert!(snapshot.blocks.is_empty());
        assert_eq!(snapshot.attachments.len(), 1);
        assert!(!snapshot.attachments[0].present);
        assert_eq!(snapshot.attachments[0].blob_hash, hash);
    }
}

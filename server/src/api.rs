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
use notes_blob::{BlobStore, BlobStoreError, VerifiedBlob};
use notes_core::BlobHash;
use notes_core::db::SearchHit;
use notes_protocol::{
    AcceptedOps, AiIndexStatus, AiProviderProbeResult, AiProviderSettingsUpdate, AiRuntimeSettings,
    ApiErrorCode, ApiErrorDetail, ApiErrorResponse, BootstrapRequest, ChatEvent, ChatTurn,
    ClientMessage, OpsBatch, PushOps, SearchRequest, ServerErrorCode, ServerInfo, ServerMessage,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;

mod ink;

const JSON_BODY_LIMIT: usize = 2 * 1024 * 1024;
/// Bootstrap uploads a full workspace snapshot in one authenticated request;
/// a real corpus (thousands of pages) far exceeds the ordinary JSON limit.
const BOOTSTRAP_BODY_LIMIT: usize = 256 * 1024 * 1024;
const MAX_OPS_PAGE: usize = 1_000;

#[derive(Clone)]
pub struct AppState {
    pub registry: UserRegistry,
    pub data_dir: PathBuf,
    pub max_blob_bytes: u64,
    pub max_user_blob_bytes: u64,
    pub(crate) blob_ownership: crate::blob_ownership::BlobOwnership,
    pub ai: Option<Arc<crate::ai::AiRuntime>>,
    pub mcp_allowed_hosts: Vec<String>,
    pub public_origin: Option<crate::oauth::PublicOrigin>,
    pub oauth: Option<crate::oauth::store::OAuthStore>,
    pub shutdown: CancellationToken,
}

#[derive(Clone)]
pub(crate) struct AuthenticatedUser(pub(crate) Arc<UserState>);

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

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: ApiErrorCode::Forbidden,
            message: message.into(),
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

    /// 426 so an old client can tell "update the app" apart from every other
    /// refusal without reading the message text.
    fn format_unsupported(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UPGRADE_REQUIRED,
            code: ApiErrorCode::FormatUnsupported,
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
                notes_core::CoreError::SyncSequenceGap { .. } => Self::conflict(core.to_string()),
                notes_core::CoreError::Database(_) | notes_core::CoreError::InkStorage(_) => {
                    Self::internal(error)
                }
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
                HeaderValue::from_static("Bearer realm=\"tangleaf\""),
            );
        }
        response
    }
}

pub fn router(state: AppState) -> Router {
    let json_routes = Router::new()
        .route("/v1/ops", get(get_ops).post(push_ops))
        .route("/v1/ink/missing", post(ink::missing))
        .route("/v1/ink/download", post(ink::download))
        .route("/v1/snapshot", get(get_snapshot))
        .route("/v1/info", get(info))
        .route("/v1/ai/status", get(ai_status).put(update_ai_settings))
        .route("/v1/ai/provider", put(update_ai_provider))
        .route("/v1/ai/provider/probe", post(probe_ai_provider))
        .route("/v1/ai/reindex", post(reindex_ai))
        .route("/v1/search", post(search))
        .route("/v1/chat", post(chat))
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT));
    let bootstrap_routes = Router::new()
        .route("/v1/bootstrap", post(bootstrap))
        .layer(DefaultBodyLimit::max(BOOTSTRAP_BODY_LIMIT));
    let stream_routes = Router::new()
        .route("/v1/sync", get(sync_socket))
        .route("/v1/ink/upload", post(ink::upload))
        .route(
            "/v1/blobs/{hash}",
            put(put_blob).get(get_blob).head(head_blob),
        )
        .layer(DefaultBodyLimit::disable());
    let mcp_routes = Router::new().route_service(
        "/mcp",
        crate::mcp::service(state.mcp_allowed_hosts.clone(), state.ai.clone()),
    );
    let protected = Router::new()
        .merge(json_routes)
        .merge(bootstrap_routes)
        .merge(stream_routes)
        .merge(mcp_routes)
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .route("/v1/health", get(health))
        .merge(crate::oauth::routes())
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
        format_version: notes_sync::FORMAT_VERSION,
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
    require_admin(&user)?;
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
    Extension(user): Extension<AuthenticatedUser>,
    Json(settings): Json<AiProviderSettingsUpdate>,
) -> Result<Json<AiProviderProbeResult>, ApiError> {
    require_admin(&user)?;
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

fn require_admin(user: &AuthenticatedUser) -> Result<(), ApiError> {
    if user.0.admin {
        Ok(())
    } else {
        Err(ApiError::forbidden("administrator access is required"))
    }
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

/// Accepts either credential this server issues: a configured bearer token, or
/// an OAuth access token obtained through the authorization endpoints.
///
/// The unauthorized response points at the protected resource metadata when a
/// public origin is configured. That pointer is how an MCP client discovers
/// where to authorize; the spec requires it on a 401 specifically, and a client
/// that does not see it cannot start the flow at all.
async fn require_auth(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    // The pointer names the origin this request arrived on, so a workspace
    // reachable under more than one name sends each client to the document that
    // describes the URL it actually used.
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let unauthorized = || {
        let mut response = ApiError::unauthorized().into_response();
        if let Some(origin) = &state.public_origin
            && let Ok(value) = HeaderValue::from_str(&format!(
                "Bearer realm=\"tangleaf\", resource_metadata=\"{}\"",
                origin.resource_metadata(host.as_deref(), &state.mcp_allowed_hosts)
            ))
        {
            response.headers_mut().insert(WWW_AUTHENTICATE, value);
        }
        response
    };
    let presented = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let Some(presented) = presented else {
        return unauthorized();
    };
    let user = match state.registry.authenticate(&presented) {
        Some(user) => Some(user),
        None => authenticate_oauth(&state, &presented).await,
    };
    let Some(user) = user else {
        return unauthorized();
    };
    request.extensions_mut().insert(AuthenticatedUser(user));
    next.run(request).await
}

async fn authenticate_oauth(state: &AppState, presented: &str) -> Option<Arc<UserState>> {
    let claims = state
        .oauth
        .as_ref()?
        .token(presented, "access")
        .await
        .inspect_err(|error| tracing::warn!(?error, "reading an OAuth access token failed"))
        .ok()??;
    state.registry.user(&claims.user_id)
}

/// Operations are only useful to a client that can decode them. A client
/// declaring an older format — or none at all, which every pre-gate build does
/// — is refused here instead of being handed a batch it drops silently and
/// then reports as a decoding failure forever.
fn require_supported_format(headers: &axum::http::HeaderMap) -> Result<(), ApiError> {
    let declared = headers
        .get(notes_protocol::FORMAT_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u32>().ok());
    if declared.is_some_and(|version| version >= notes_sync::FORMAT_VERSION) {
        return Ok(());
    }
    Err(ApiError::format_unsupported(format!(
        "this server writes sync format {}; update the app to keep syncing",
        notes_sync::FORMAT_VERSION
    )))
}

async fn get_ops(
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<OpsQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Json<OpsBatch>, ApiError> {
    require_supported_format(&headers)?;
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
    headers: axum::http::HeaderMap,
    Json(request): Json<PushOps>,
) -> Result<Json<AcceptedOps>, ApiError> {
    require_supported_format(&headers)?;
    if request.ops.len() > 256 {
        return Err(ApiError::bad_request(
            "a sync batch cannot contain more than 256 operations",
        ));
    }
    validate_operation_blobs(&state, &request.ops).await?;
    ink::validate(&state, &user.0, ink_roots(&request.ops)).await?;
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
    ink::validate(
        &state,
        &user.0,
        request
            .snapshot
            .ink_versions
            .iter()
            .map(|v| v.publication.root_hash)
            .collect(),
    )
    .await?;
    user.0
        .bootstrap(request.snapshot)
        .await
        .map(Json)
        .map_err(ApiError::from_domain)
}

async fn sync_socket(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<SyncQuery>,
    headers: axum::http::HeaderMap,
    socket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    require_supported_format(&headers)?;
    Ok(socket
        .max_message_size(JSON_BODY_LIMIT)
        .on_upgrade(move |socket| websocket_session(state, user.0, query.since, socket)))
}

async fn websocket_session(state: AppState, user: Arc<UserState>, since: u64, socket: WebSocket) {
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
                    Ok(ClientMessage::Push { ops }) if ops.len() <= 256 => match ingest_ink_checked(&state,&user,ops).await {
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
    Extension(user): Extension<AuthenticatedUser>,
    Path(raw_hash): Path<String>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let hash = parse_blob_hash(&raw_hash)?;
    let staging = stage_blob(&state.data_dir, body, state.max_blob_bytes).await?;
    if staging.hash != hash {
        return Err(ApiError::bad_request(
            "request body SHA-256 does not match the blob URL",
        ));
    }
    let claim = state
        .blob_ownership
        .claim(&user.0.id, hash, staging.size, state.max_user_blob_bytes)
        .await
        .map_err(map_blob_ownership_error)?;
    let staging_path = staging.path.to_path_buf();
    let store = BlobStore::new(state.data_dir.clone());
    let maximum = state.max_blob_bytes;
    let installed =
        tokio::task::spawn_blocking(move || store.install_file(&staging_path, hash, maximum))
            .await
            .map_err(|error| ApiError::internal(error.into()))?;
    if let Err(error) = installed {
        if claim == crate::blob_ownership::ClaimOutcome::Claimed
            && let Err(release_error) = state.blob_ownership.release(&user.0.id, hash).await
        {
            tracing::error!(?release_error, user = %user.0.id, %hash, "rolling back blob ownership failed");
        }
        return Err(map_blob_install_error(error));
    }
    // Deliberately uniform for both physical installation and deduplication.
    Ok(StatusCode::CREATED)
}

struct StagedBlob {
    path: tempfile::TempPath,
    size: u64,
    hash: BlobHash,
}

async fn stage_blob(data_dir: &FilePath, body: Body, maximum: u64) -> Result<StagedBlob, ApiError> {
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
    let mut hasher = Sha256::new();
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
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| ApiError::internal(error.into()))?;
    }
    file.flush()
        .await
        .map_err(|error| ApiError::internal(error.into()))?;
    drop(file);
    Ok(StagedBlob {
        path: staging,
        size,
        hash: BlobHash::from_bytes(hasher.finalize().into()),
    })
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

/// Two independent conditions gate a download: the caller's own oplog must
/// reference the hash, and the caller must own the stored blob. A reference
/// alone is forgeable — anyone can declare another user's hash in their own
/// operations — so ownership is what actually authorizes the bytes. Both
/// failures answer 404 so the response never reveals that the blob exists.
async fn open_authorized_blob(
    state: &AppState,
    user: &UserState,
    hash: BlobHash,
) -> Result<VerifiedBlob, ApiError> {
    if !user_references_blob(user, &hash).await? {
        return Err(ApiError::not_found("blob not found"));
    }
    let owned = state
        .blob_ownership
        .owned(&user.id, vec![hash])
        .await
        .map_err(ApiError::internal)?;
    if !owned.contains_key(&hash) {
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

fn map_blob_ownership_error(error: anyhow::Error) -> ApiError {
    if let Some(quota) = error.downcast_ref::<crate::blob_ownership::QuotaExceeded>() {
        return ApiError::too_large(format!(
            "blob quota exceeded: {} of {} bytes used; upload needs {} bytes",
            quota.used, quota.limit, quota.requested
        ));
    }
    ApiError::internal(error)
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

fn ink_roots(ops: &[notes_core::Op]) -> Vec<BlobHash> {
    ops.iter()
        .filter_map(|op| {
            if let notes_core::OpKind::InkPublish(p) = &op.kind {
                Some(p.root_hash)
            } else {
                None
            }
        })
        .collect()
}
async fn ingest_ink_checked(
    state: &AppState,
    user: &UserState,
    ops: Vec<notes_core::Op>,
) -> anyhow::Result<Vec<notes_protocol::SequencedOp>> {
    // The socket carries the same operations as POST /v1/ops and therefore runs
    // the same declared-blob validation; skipping it here let a socket push
    // register attachment metadata the HTTP path would have rejected.
    validate_operation_blobs(state, &ops)
        .await
        .map_err(|e| notes_core::CoreError::invalid(e.message))?;
    ink::validate(state, user, ink_roots(&ops))
        .await
        .map_err(|e| notes_core::CoreError::invalid(e.message))?;
    user.ingest(ops).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, ServerConfig, UserConfig};
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
            max_user_blob_bytes: 1024,
            ai: None,
            mcp_allowed_hosts: Vec::new(),
            public_url: None,
            users: vec![
                UserConfig {
                    id: "owner".into(),
                    admin: true,
                    token_sha256: Some(
                        "1fe4109a7f6627feb6d833a37288ce43668a8d649bdfc658494388c3f4cd9a30".into(),
                    ),
                    token: None,
                    tokens: vec![],
                },
                UserConfig {
                    id: "other".into(),
                    admin: false,
                    token_sha256: None,
                    token: None,
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

    async fn test_app_with_ai(ai: AiConfig) -> (tempfile::TempDir, Router) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().expect("listen address"),
            log_filter: "info".into(),
            data_dir: directory.path().to_owned(),
            snapshot_every_ops: 2,
            max_blob_bytes: 1024,
            max_user_blob_bytes: 1024,
            ai: Some(ai),
            mcp_allowed_hosts: Vec::new(),
            public_url: None,
            users: vec![UserConfig {
                id: "owner".into(),
                admin: true,
                token_sha256: None,
                token: None,
                tokens: vec![TOKEN.into()],
            }],
        };
        let state = crate::build_state(&config).await.expect("server state");
        (directory, router(state))
    }

    fn provider_update_json(base_url: &str) -> serde_json::Value {
        serde_json::json!({
            "retrievalBaseUrl": base_url,
            "retrievalApiKey": null,
            "embeddingModel": "embedding-model",
            "embeddingDimensions": 1024,
            "rerankModel": "rerank-model",
            "completionProtocol": "openai",
            "completionBaseUrl": base_url,
            "completionApiKey": null,
            "chatModel": "chat-model",
            "extractionModel": "extraction-model"
        })
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
        request
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(
                notes_protocol::FORMAT_VERSION_HEADER,
                notes_sync::FORMAT_VERSION.to_string(),
            )
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
        assert_eq!(response.status(), StatusCode::CREATED);
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
    async fn non_admin_cannot_mutate_or_probe_ai_provider() {
        let (_directory, app) = test_app().await;
        for (method, uri) in [
            ("PUT", "/v1/ai/provider"),
            ("POST", "/v1/ai/provider/probe"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    authorized_as(Request::builder(), OTHER_TOKEN)
                        .method(method)
                        .uri(uri)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            provider_update_json("https://provider.example.test/v1").to_string(),
                        ))
                        .expect("provider request"),
                )
                .await
                .expect("provider response");
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                typed_error(response).await.error.code,
                ApiErrorCode::Forbidden
            );
        }
    }

    #[tokio::test]
    async fn probe_with_new_url_and_no_key_never_sends_the_stored_secret() {
        let trap = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider trap");
        let trap_url = format!("http://{}/v1", trap.local_addr().unwrap());
        let ai = AiConfig {
            retrieval_api_key: "stored-retrieval-secret".into(),
            completion_api_key: "stored-completion-secret".into(),
            retrieval_base_url: url::Url::parse("http://127.0.0.1:9/old").unwrap(),
            completion_base_url: url::Url::parse("http://127.0.0.1:9/old").unwrap(),
            ..AiConfig::default()
        };
        let (_directory, app) = test_app_with_ai(ai).await;

        let response = app
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/ai/provider/probe")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(provider_update_json(&trap_url).to_string()))
                    .expect("probe request"),
            )
            .await
            .expect("probe response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), trap.accept())
                .await
                .is_err(),
            "the new provider URL must not receive a request carrying a stored secret"
        );
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
            Some(&HeaderValue::from_static("Bearer realm=\"tangleaf\""))
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
        assert_eq!(statuses, vec![StatusCode::CREATED; 8]);
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
    async fn uploads_record_per_user_ownership_enforce_quota_and_hide_deduplication() {
        let (directory, app) = test_app().await;
        let shared = vec![b'a'; 600];
        let shared_hash = BlobHash::digest(&shared);
        let over_quota = vec![b'b'; 500];
        let over_quota_hash = BlobHash::digest(&over_quota);

        let owner_upload = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{shared_hash}"))
                    .body(Body::from(shared.clone()))
                    .expect("owner upload"),
            )
            .await
            .expect("owner upload response");
        assert_eq!(owner_upload.status(), StatusCode::CREATED);

        let hidden_from_other = app
            .clone()
            .oneshot(
                authorized_as(Request::builder(), OTHER_TOKEN)
                    .uri(format!("/v1/blobs/{shared_hash}"))
                    .body(Body::empty())
                    .expect("unowned download"),
            )
            .await
            .expect("unowned response");
        assert_eq!(hidden_from_other.status(), StatusCode::NOT_FOUND);

        let duplicate = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{shared_hash}"))
                    .body(Body::from(shared.clone()))
                    .expect("duplicate upload"),
            )
            .await
            .expect("duplicate response");
        assert_eq!(duplicate.status(), StatusCode::CREATED);

        let rejected = app
            .clone()
            .oneshot(
                authorized(Request::builder())
                    .method("PUT")
                    .uri(format!("/v1/blobs/{over_quota_hash}"))
                    .body(Body::from(over_quota.clone()))
                    .expect("over-quota upload"),
            )
            .await
            .expect("over-quota response");
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            typed_error(rejected).await.error.code,
            ApiErrorCode::PayloadTooLarge
        );
        assert!(
            !BlobStore::new(directory.path())
                .path_for(over_quota_hash)
                .exists(),
            "quota rejection must happen before physical installation"
        );

        let other_upload = app
            .clone()
            .oneshot(
                authorized_as(Request::builder(), OTHER_TOKEN)
                    .method("PUT")
                    .uri(format!("/v1/blobs/{shared_hash}"))
                    .body(Body::from(shared))
                    .expect("other owner upload"),
            )
            .await
            .expect("other owner response");
        assert_eq!(other_upload.status(), StatusCode::CREATED);
        let unreferenced_download = app
            .oneshot(
                authorized_as(Request::builder(), OTHER_TOKEN)
                    .uri(format!("/v1/blobs/{shared_hash}"))
                    .body(Body::empty())
                    .expect("owned download"),
            )
            .await
            .expect("unreferenced response");
        assert_eq!(unreferenced_download.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn corrupt_existing_blob_is_never_overwritten_or_streamed() {
        let (directory, app) = test_app().await;
        let contents = b"expected attachment bytes";
        let corrupt = vec![b'x'; contents.len()];
        let hash = upload_test_blob(&app, contents.as_slice()).await;
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
        let hash = upload_test_blob(&app, original.as_slice()).await;
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

    #[tokio::test]
    async fn bootstrap_accepts_bodies_larger_than_the_ordinary_json_limit() {
        // Regression: a real-corpus bootstrap snapshot exceeds JSON_BODY_LIMIT
        // (observed as HTTP 413 on the first ~17k-op workspace upload). The
        // bootstrap route carries its own, much larger limit; a payload above
        // the ordinary limit must reach the handler (any error must come from
        // JSON parsing, never from the body-size layer).
        let (_dir, app) = test_app().await;
        let oversized = vec![b' '; JSON_BODY_LIMIT + 1024];
        let response = app
            .oneshot(
                authorized(Request::builder())
                    .method("POST")
                    .uri("/v1/bootstrap")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(oversized))
                    .expect("bootstrap request"),
            )
            .await
            .expect("bootstrap response");
        assert_ne!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}

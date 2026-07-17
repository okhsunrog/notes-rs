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
use notes_core::db::SearchHit;
use notes_protocol::{
    AcceptedOps, AiIndexStatus, AiRuntimeSettings, BootstrapRequest, ChatEvent, ChatTurn,
    ClientMessage, OpsBatch, PushOps, ServerInfo, ServerMessage,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio_util::io::ReaderStream;
use tower_http::trace::TraceLayer;

const JSON_BODY_LIMIT: usize = 2 * 1024 * 1024;
const MAX_OPS_PAGE: usize = 1_000;

#[derive(Clone)]
pub struct AppState {
    pub registry: UserRegistry,
    pub data_dir: PathBuf,
    pub max_blob_bytes: u64,
    pub ai: Option<Arc<crate::ai::AiRuntime>>,
}

#[derive(Clone)]
struct AuthenticatedUser(Arc<UserState>);

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
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
struct SearchRequest {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChatRequest {
    #[serde(default)]
    history: Vec<ChatTurn>,
    message: String,
    #[serde(default)]
    allow_writes: bool,
    active_node_uuid: Option<uuid::Uuid>,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "a valid bearer token is required".into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }

    fn too_large(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "unavailable",
            message: message.into(),
        }
    }

    fn internal(error: anyhow::Error) -> Self {
        tracing::error!(error = ?error, "request failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "the server could not complete the request".into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ErrorBody {
                error: ErrorDetail {
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

async fn info(State(state): State<AppState>) -> Json<ServerInfo> {
    Json(ServerInfo {
        ai_enabled: state.ai.is_some(),
    })
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

const fn default_search_limit() -> u32 {
    20
}

async fn search(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let ai = state
        .ai
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("server AI is not configured"))?;
    ai.search(&user.0.id, request.query, request.limit)
        .await
        .map(Json)
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("must contain") || message.contains("must be between") {
                ApiError::bad_request(message)
            } else {
                ApiError::internal(error)
            }
        })
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
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
                cancellation_for_emit.store(true, std::sync::atomic::Ordering::Release);
            }
        };
        if let Err(error) = ai
            .chat(
                &user_id,
                request.history,
                request.message,
                request.allow_writes,
                request.active_node_uuid,
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
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<PushOps>,
) -> Result<Json<AcceptedOps>, ApiError> {
    if request.ops.len() > 256 {
        return Err(ApiError::bad_request(
            "a sync batch cannot contain more than 256 operations",
        ));
    }
    let ops = user
        .0
        .ingest(request.ops)
        .await
        .map_err(ApiError::internal)?;
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
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<BootstrapRequest>,
) -> Result<Json<notes_sync::SyncSnapshot>, ApiError> {
    user.0
        .bootstrap(request.snapshot)
        .await
        .map(Json)
        .map_err(|error| {
            if error.to_string().contains("not empty") {
                ApiError::conflict("server workspace has already been initialized")
            } else {
                ApiError::bad_request(error.to_string())
            }
        })
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
            let _ = send_server_error(&mut sender, "catch_up_failed", "could not load operations")
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
                        if send_server_error(&mut sender, "unsupported_message", "send JSON text messages").await.is_err() { return; }
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
                            if send_server_error(&mut sender, "ingest_failed", "operation batch was rejected").await.is_err() { return; }
                        }
                    },
                    Ok(ClientMessage::Push { .. }) => {
                        if send_server_error(&mut sender, "batch_too_large", "a batch cannot exceed 256 operations").await.is_err() { return; }
                    }
                    Ok(ClientMessage::Ping) => {
                        if send_server_message(&mut sender, &ServerMessage::Pong).await.is_err() { return; }
                    }
                    Err(_) => {
                        if send_server_error(&mut sender, "invalid_message", "message does not match the sync protocol").await.is_err() { return; }
                    }
                }
            }
            published = operations.recv() => match published {
                Ok(operation) => {
                    if send_server_message(&mut sender, &ServerMessage::Ops { ops: vec![operation] }).await.is_err() { return; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = send_server_error(&mut sender, "resync_required", "the client fell behind; reconnect to catch up").await;
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
    code: impl Into<String>,
    message: impl Into<String>,
) -> Result<(), axum::Error> {
    send_server_message(
        sender,
        &ServerMessage::Error {
            code: code.into(),
            message: message.into(),
        },
    )
    .await
}

async fn put_blob(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    validate_blob_hash(&hash)?;
    let target = blob_path(&state.data_dir, &hash);
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|error| ApiError::internal(error.into()))?
    {
        return Ok(StatusCode::NO_CONTENT);
    }
    let parent = target.parent().expect("blob path has a parent");
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| ApiError::internal(error.into()))?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::now_v7()));
    let result = write_blob(&temporary, body, state.max_blob_bytes).await;
    let (actual_hash, _) = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error);
        }
    };
    if actual_hash != hash {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ApiError::bad_request(
            "request body SHA-256 does not match the blob URL",
        ));
    }
    match tokio::fs::rename(&temporary, &target).await {
        Ok(()) => Ok(StatusCode::CREATED),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = tokio::fs::remove_file(&temporary).await;
            Ok(StatusCode::NO_CONTENT)
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            Err(ApiError::internal(error.into()))
        }
    }
}

async fn write_blob(path: &FilePath, body: Body, maximum: u64) -> Result<(String, u64), ApiError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| ApiError::internal(error.into()))?;
    let mut stream = body.into_data_stream();
    let mut digest = Sha256::new();
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
        digest.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| ApiError::internal(error.into()))?;
    }
    file.flush()
        .await
        .map_err(|error| ApiError::internal(error.into()))?;
    Ok((format!("{:x}", digest.finalize()), size))
}

async fn head_blob(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    blob_response_headers(&state, &user.0, &hash).await
}

async fn get_blob(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    let mut response = blob_response_headers(&state, &user.0, &hash).await?;
    let file = tokio::fs::File::open(blob_path(&state.data_dir, &hash))
        .await
        .map_err(|error| ApiError::internal(error.into()))?;
    *response.body_mut() = Body::from_stream(ReaderStream::new(file));
    Ok(response)
}

async fn blob_response_headers(
    state: &AppState,
    user: &UserState,
    hash: &str,
) -> Result<Response, ApiError> {
    validate_blob_hash(hash)?;
    if !user_references_blob(user, hash).await? {
        return Err(ApiError::not_found("blob not found"));
    }
    let metadata = tokio::fs::metadata(blob_path(&state.data_dir, hash))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found("blob not found")
            } else {
                ApiError::internal(error.into())
            }
        })?;
    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&metadata.len().to_string()).expect("valid content length"),
    );
    Ok(response)
}

async fn user_references_blob(user: &UserState, hash: &str) -> Result<bool, ApiError> {
    let hash = hash.to_owned();
    user.notes
        .call(move |database| {
            database.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM attachment_lww
                    WHERE blob_hash = ?1 AND present = 1
                 )",
                [hash],
                |row| row.get(0),
            )
        })
        .await
        .map_err(ApiError::internal)
}

fn validate_blob_hash(hash: &str) -> Result<(), ApiError> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ApiError::bad_request(
            "blob hash must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

fn blob_path(data_dir: &FilePath, hash: &str) -> PathBuf {
    data_dir.join("blobs").join(&hash[..2]).join(hash)
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
    use notes_core::{Hlc, NodeKind, Op, OpKind};
    use notes_sync::{AttachmentAdd, NodeCreate};
    use tower::ServiceExt;

    const TOKEN: &str = "test-token-with-at-least-thirty-two-characters";

    async fn test_app() -> (tempfile::TempDir, Router) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().expect("listen address"),
            log_filter: "info".into(),
            data_dir: directory.path().to_owned(),
            snapshot_every_ops: 2,
            max_blob_bytes: 1024,
            ai: None,
            users: vec![UserConfig {
                id: "owner".into(),
                tokens: vec![TOKEN.into()],
            }],
        };
        let state = crate::build_state(&config).await.expect("server state");
        (directory, router(state))
    }

    fn operation(index: u128, kind: OpKind) -> Op {
        let device_id = uuid::Uuid::from_u128(1);
        Op {
            op_id: uuid::Uuid::from_u128(index + 10),
            device_id,
            hlc: Hlc::new(index as u64, 0, device_id),
            format_version: notes_sync::FORMAT_VERSION,
            kind,
        }
    }

    fn page_operation(page_uuid: uuid::Uuid) -> Op {
        operation(
            1,
            OpKind::NodeCreate(NodeCreate {
                uuid: page_uuid,
                node_kind: NodeKind::Page,
                title: Some("Synced page".into()),
                content: String::new(),
                content_json: None,
                parent_uuid: None,
                position: None,
                created_at: 1,
            }),
        )
    }

    fn authorized(request: axum::http::request::Builder) -> axum::http::request::Builder {
        request.header(AUTHORIZATION, format!("Bearer {TOKEN}"))
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
        assert!(!info.ai_enabled);
    }

    #[tokio::test]
    async fn snapshots_and_streams_only_referenced_content_addressed_blobs() {
        let (_directory, app) = test_app().await;
        let page_uuid = uuid::Uuid::from_u128(100);
        let contents = b"portable attachment";
        let hash = format!("{:x}", Sha256::digest(contents));
        let operations = vec![
            page_operation(page_uuid),
            operation(
                2,
                OpKind::AttachmentAdd(AttachmentAdd {
                    node_uuid: page_uuid,
                    blob_hash: hash.clone(),
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
        assert_eq!(snapshot.seq, 2);
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.attachments.len(), 1);
    }
}

//! Transport-only request, response, and event types shared by every host.

pub mod ink;

use notes_core::{Op, SyncSnapshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SequencedOp {
    pub seq: u64,
    pub envelope: Op,
}

/// Request header carrying the sync format version a client can decode.
///
/// The same header is sent on the operation endpoints and on the sync
/// WebSocket handshake, so both paths refuse an unreadable exchange the same
/// way instead of streaming operations the client silently drops.
pub const FORMAT_VERSION_HEADER: &str = "x-sync-format-version";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub workspace_uuid: uuid::Uuid,
    pub ai_enabled: bool,
    /// Sync format the server writes. A server predating this field reports 0,
    /// which no client treats as a reason to stop.
    #[serde(default)]
    pub format_version: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AiGenerationState {
    Building,
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CompletionProtocol {
    Openai,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderSettings {
    pub retrieval_base_url: url::Url,
    pub retrieval_api_key_configured: bool,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub rerank_model: String,
    pub completion_protocol: CompletionProtocol,
    pub completion_base_url: url::Url,
    pub completion_api_key_configured: bool,
    pub chat_model: String,
    pub extraction_model: String,
}

/// Complete non-secret provider configuration plus optional write-only secrets.
/// A missing secret preserves the currently stored value only when its base URL is unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiProviderSettingsUpdate {
    pub retrieval_base_url: url::Url,
    pub retrieval_api_key: Option<String>,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub rerank_model: String,
    pub completion_protocol: CompletionProtocol,
    pub completion_base_url: url::Url,
    pub completion_api_key: Option<String>,
    pub chat_model: String,
    pub extraction_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AiProbeCheck {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderProbeResult {
    pub embeddings: AiProbeCheck,
    pub reranking: AiProbeCheck,
    pub chat: AiProbeCheck,
    pub extraction: AiProbeCheck,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AiRuntimeSettings {
    pub automatic_embeddings: bool,
    pub entity_extraction: bool,
    pub query_rewriting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AiIndexStatus {
    pub provider: AiProviderSettings,
    pub generation_id: uuid::Uuid,
    pub generation_state: AiGenerationState,
    pub settings: AiRuntimeSettings,
    pub pending_embeddings: u64,
    pub failed_embeddings: u64,
    pub indexed_documents: u64,
    pub source_documents: u64,
    pub pending_extractions: u64,
    pub failed_extractions: u64,
}

fn default_search_limit() -> u32 {
    20
}

const fn default_rerank() -> bool {
    true
}

const fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    #[serde(default = "default_rerank", skip_serializing_if = "is_true")]
    pub rerank: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpsBatch {
    pub ops: Vec<SequencedOp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PushOps {
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AcceptedOps {
    pub ops: Vec<SequencedOp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BootstrapRequest {
    pub snapshot: SyncSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Push { ops: Vec<Op> },
    Ping,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ServerErrorCode {
    CatchUpFailed,
    UnsupportedMessage,
    IngestFailed,
    BatchTooLarge,
    InvalidMessage,
    ResyncRequired,
    Conflict,
}

impl ServerErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CatchUpFailed => "catch_up_failed",
            Self::UnsupportedMessage => "unsupported_message",
            Self::IngestFailed => "ingest_failed",
            Self::BatchTooLarge => "batch_too_large",
            Self::InvalidMessage => "invalid_message",
            Self::ResyncRequired => "resync_required",
            Self::Conflict => "conflict",
        }
    }
}

impl std::fmt::Display for ServerErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ops {
        ops: Vec<SequencedOp>,
    },
    Ack {
        ops: Vec<SequencedOp>,
    },
    Pong,
    Error {
        code: ServerErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    PayloadTooLarge,
    /// The caller decodes an older sync format than this server writes.
    /// Terminal: retrying the same build can only fail again.
    FormatUnsupported,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiErrorResponse {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiErrorDetail {
    pub code: ApiErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatTurn {
    User { text: String },
    Assistant { text: String },
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    TextDelta {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolStart {
        id: String,
        name: String,
        #[specta(type = specta_typescript::Unknown)]
        args: serde_json::Value,
    },
    ToolEnd {
        id: String,
        result: String,
    },
    Done {
        text: String,
    },
    Error {
        message: String,
    },
    Usage {
        #[serde(rename = "inputTokens")]
        input_tokens: u64,
        #[serde(rename = "outputTokens")]
        output_tokens: u64,
        #[serde(rename = "totalTokens")]
        total_tokens: u64,
    },
    Cancelled,
}

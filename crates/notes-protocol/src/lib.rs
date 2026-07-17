//! Transport-only request, response, and event types shared by every host.

use notes_core::{Op, SyncSnapshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SequencedOp {
    pub seq: u64,
    pub envelope: Op,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub embedding_provider_id: String,
    pub embedding_dimensions: usize,
    pub ai_enabled: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ops { ops: Vec<SequencedOp> },
    Ack { ops: Vec<SequencedOp> },
    Pong,
    Error { code: String, message: String },
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

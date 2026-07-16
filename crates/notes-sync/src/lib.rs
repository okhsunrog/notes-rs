//! Sync protocol boundary for notes-rs.
//!
//! Versioned wire types, HLC merge semantics, and the transport-independent
//! client state machine shared by desktop and the future server host.

mod machine;
mod transport;

pub use machine::{LoopbackServer, SequencedOp, SyncClient, SyncStats};
pub use transport::{HttpTransport, SyncSocket};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub embedding_provider_id: String,
    pub embedding_dimensions: usize,
    pub ai_enabled: bool,
}

pub use notes_core::Hlc;
pub use notes_core::operation::FORMAT_VERSION;
pub use notes_core::operation::{
    AttachmentAdd, AttachmentRemove, EdgeAdd, EdgeRemove, NodeCreate, NodeDelete, NodeMove,
    NodeSetContent, NodeSetTitle,
};
pub use notes_core::{
    ApplyOutcome, Op, OpKind, Origin, SnapshotAttachment, SnapshotEdge, SnapshotNode,
    SnapshotTombstone, SyncSnapshot, acknowledge_server_op, apply, apply_batch, apply_sequenced,
    configure_sync, export_sync_snapshot, import_sync_snapshot, local_ops, pending_outbox,
    sync_cursor,
};

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

//! Sync protocol boundary for notes-rs.
//!
//! Versioned wire types, HLC merge semantics, and the transport-independent
//! client state machine shared by desktop and the future server host.

mod machine;
mod transport;

pub use machine::{AppliedRemoteOperation, LoopbackServer, SyncClient, SyncStats, SyncTransport};
pub use transport::{HttpTransport, SyncSocket, TransportError, is_transport_failure};

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

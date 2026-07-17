//! Sync protocol boundary for notes-rs.
//!
//! Versioned wire types, HLC merge semantics, and the transport-independent
//! client state machine shared by desktop and the future server host.

mod machine;
mod transport;

pub use machine::{AppliedRemoteOperation, LoopbackServer, SyncClient, SyncStats, SyncTransport};
pub use transport::{
    HttpTransport, SyncSocket, TransportError, is_transport_failure, transport_error,
};

pub use notes_core::Hlc;
pub use notes_core::operation::FORMAT_VERSION;
pub use notes_core::operation::{
    AttachmentAdd, AttachmentRemove, BlockCreate, BlockDelete, BlockMove, BlockSetMarkdown,
    BlockSetStyle, PageAliasSet, PageCreate, PageDelete, PageSetLayout, PageSetTitle,
};
pub use notes_core::{
    ApplyOutcome, AttachmentOwner, BlockStyle, JournalDate, ObjectKind, Op, OpKind, OrderKey,
    Origin, PageAlias, PageKind, PageLayout, SnapshotAttachment, SnapshotBlock, SnapshotPage,
    SnapshotPageAlias, SnapshotPageIdentity, SnapshotTombstone, SyncSnapshot, TaskState,
    acknowledge_server_op, apply, apply_batch, apply_sequenced, configure_sync,
    export_sync_snapshot, import_sync_snapshot, pending_outbox, sync_cursor,
};

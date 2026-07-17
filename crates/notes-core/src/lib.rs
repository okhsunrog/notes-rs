//! Host-independent notes-rs domain model and SQLite storage engine.

pub mod db;
mod error;
pub mod hlc;
pub mod model;
pub mod operation;
pub mod sqlite;
pub mod stem;

pub use error::{CoreError, CoreResult};
pub use hlc::Hlc;
pub use model::{NodeKind, ReorderDirection};
pub use operation::{
    ApplyOutcome, Op, OpKind, Origin, SnapshotAttachment, SnapshotEdge, SnapshotNode,
    SnapshotTombstone, SyncSnapshot, acknowledge_server_op, acknowledge_server_ops, apply,
    apply_batch, apply_sequenced, apply_sequenced_batch, configure_sync, export_sync_snapshot,
    import_sync_snapshot, local_ops, pending_outbox, sync_cursor,
};
pub use sqlite::Connection;

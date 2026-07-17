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
pub use model::{
    AttachmentOwner, BlockStyle, JournalDate, ObjectKind, OrderKey, PageKind, PageLayout,
    ReorderDirection, journal_page_uuid,
};
pub use operation::{
    ApplyOutcome, AttachmentAdd, AttachmentRemove, BlockCreate, BlockDelete, BlockMove,
    BlockSetMarkdown, BlockSetStyle, Op, OpKind, Origin, PageCreate, PageDelete, PageSetLayout,
    PageSetTitle, SnapshotAttachment, SnapshotBlock, SnapshotBlockStructure, SnapshotPage,
    SnapshotPageIdentity, SnapshotTombstone, SyncSnapshot, acknowledge_server_op,
    acknowledge_server_ops, apply, apply_batch, apply_sequenced, apply_sequenced_batch,
    configure_sync, content_references_changed, export_sync_snapshot, import_sync_snapshot,
    pending_outbox, sync_cursor, validate_attachment_filename, validate_blob_hash,
};
pub use sqlite::Connection;

//! Host-independent notes-rs domain model and SQLite storage engine.

pub mod db;
mod error;
pub mod external_import;
pub mod hlc;
pub mod model;
pub mod operation;
pub mod sqlite;
pub mod stem;

pub use error::{CoreError, CoreResult};
pub use external_import::{
    ExternalBlockId, ExternalImportAttachment, ExternalImportAttachmentOwner, ExternalImportBatch,
    ExternalImportBlock, ExternalImportDigest, ExternalImportFormat, ExternalImportIdentityContext,
    ExternalImportOutcome, ExternalImportPage, ExternalImportPageKind, ExternalImportProvenance,
    ExternalImportReceipt, ExternalPageId, apply_external_import,
    external_import_destination_is_empty, external_import_receipt,
};
pub use hlc::Hlc;
pub use model::{
    AttachmentOwner, BlockStyle, ContentRevision, DocumentRevision, JournalDate, ObjectKind,
    OrderKey, PageAlias, PageKind, PageLayout, PageListFilter, ReorderDirection, TaskState,
    journal_page_uuid,
};
pub use notes_blob::BlobHash;
pub use operation::{
    ApplyOutcome, AttachmentAdd, AttachmentRemove, BlockCreate, BlockDelete, BlockMove,
    BlockSetMarkdown, BlockSetStyle, Op, OpKind, Origin, PageAliasSet, PageCreate, PageDelete,
    PageSetLayout, PageSetTitle, SnapshotAttachment, SnapshotBlock, SnapshotBlockStructure,
    SnapshotPage, SnapshotPageAlias, SnapshotPageIdentity, SnapshotTombstone, SyncSnapshot,
    acknowledge_server_op, acknowledge_server_ops, apply, apply_batch, apply_sequenced,
    apply_sequenced_batch, attachment_uuid, bind_sync_workspace, configure_sync,
    content_references_changed, decode_persisted_envelope, encode_persisted_envelope,
    export_sync_snapshot, import_sync_snapshot, pending_outbox, project_sync_snapshot,
    sync_bound_workspace, sync_cursor, validate_attachment_filename,
};
pub use sqlite::Connection;
pub use stem::SearchTokenMode;

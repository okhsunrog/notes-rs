use crate::model::{
    AttachmentOwner, BlockStyle, ContentRevision, ObjectKind, OrderKey, PageKind, PageLayout,
    ReorderDirection, TaskState,
};
use crate::operation::{self, OpKind};
use crate::sqlite::Connection;
use anyhow::{Context, Result};
use notes_blob::BlobHash;
use serde::{Deserialize, Serialize};
use std::path::Path;

mod archive;
mod attachments;
mod blocks;
mod document;
mod graph;
mod history;
mod journals;
mod migrations;
mod pages;
mod search;
mod workspace;

pub use archive::{
    ARCHIVE_VERSION, ArchiveImportStats, export_archive, import_archive,
    import_archive_with_precommit,
};
pub use attachments::{
    AttachmentImageCache, attachment_path_ref_count, cleanup_unreferenced_attachment_blobs,
    create_attachment, create_attachment_with_ops, delete_attachment, delete_attachment_with_ops,
    get_attachment, get_attachment_image_cache, list_attachments, upsert_attachment_image_cache,
};
pub use blocks::{
    BlockContent, create_block, create_block_with_ops, delete_block, delete_block_with_ops,
    get_block, get_blocks, indent_block, indent_block_with_ops, list_block_children, move_block,
    move_block_in_direction, move_block_in_direction_with_ops, move_block_with_ops, outdent_block,
    outdent_block_with_ops, reorder_block, reorder_block_with_ops, set_block_content,
    set_block_content_if_revision, set_block_content_if_revision_with_ops, set_block_style,
    set_block_style_with_ops, set_task_state, set_task_state_with_ops, split_block,
    split_block_with_ops,
};
pub use document::{
    DocumentUnitDraft, MAX_DOCUMENT_DEPTH, MAX_DOCUMENT_MARKDOWN_BYTES, MAX_DOCUMENT_UNITS,
    PageDocumentReplaceOutcome, PageDocumentSnapshot, get_page_document, replace_page_document,
    replace_page_document_with_outcome,
};
pub use graph::{find_backlinks, graph_snapshot, neighbors, read_ancestors, read_subtree};
pub use history::{HistoryMoveResult, HistoryStatus, history_status, redo_history, undo_history};
pub use journals::{
    JournalListLimit, append_to_journal, append_to_journal_with_ops, ensure_journal,
    ensure_journal_with_ops, get_journal, list_journals,
};
pub use pages::{
    CreateNoteResult, CreatedNote, DeletedPage, create_note, create_note_with_ops, create_page,
    create_page_with_ops, delete_page, get_containing_page, get_or_create_page_by_title,
    get_or_create_page_by_title_with_ops, get_page, get_page_by_title, list_pages,
    list_pages_filtered, rename_page, rename_page_if_revision, rename_page_if_revision_with_ops,
    set_page_layout, set_page_layout_with_ops,
};
pub use search::{search_blocks_fts, search_fts, search_pages_by_title};
pub(crate) use workspace::transaction_workspace_uuid;
pub use workspace::workspace_uuid;

pub async fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let conn = Connection::open(path.as_ref())
        .await
        .context("opening sqlite database")?;
    initialize_connection(conn).await
}

pub(crate) async fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()
        .await
        .context("opening in-memory sqlite database")?;
    initialize_connection(conn).await
}

async fn initialize_connection(conn: Connection) -> Result<Connection> {
    conn.call(|database| -> rusqlite::Result<()> {
        database.pragma_update(None, "journal_mode", "WAL")?;
        database.pragma_update(None, "synchronous", "NORMAL")?;
        database.pragma_update(None, "foreign_keys", "ON")?;
        database.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    })
    .await?;
    migrations::migrate(&conn).await?;
    workspace::initialize_workspace(&conn).await?;
    Ok(conn)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub uuid: uuid::Uuid,
    pub kind: crate::model::PageKind,
    pub title: Option<String>,
    pub layout: PageLayout,
    pub title_revision: ContentRevision,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub uuid: uuid::Uuid,
    pub page_uuid: uuid::Uuid,
    pub parent_uuid: Option<uuid::Uuid>,
    pub order_key: OrderKey,
    pub style: BlockStyle,
    pub markdown: String,
    pub markdown_revision: ContentRevision,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub uuid: uuid::Uuid,
    pub owner: AttachmentOwner,
    #[specta(type = String)]
    pub blob_hash: BlobHash,
    pub filename: String,
    pub mime: String,
    pub size: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(tag = "kind", content = "record", rename_all = "lowercase")]
pub enum Content {
    Page(Page),
    Block(Block),
}

impl Content {
    pub const fn uuid(&self) -> uuid::Uuid {
        match self {
            Self::Page(page) => page.uuid,
            Self::Block(block) => block.uuid,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Self::Page(page) => page.title.as_deref().unwrap_or(""),
            Self::Block(block) => &block.markdown,
        }
    }

    pub const fn page_uuid(&self) -> uuid::Uuid {
        match self {
            Self::Page(page) => page.uuid,
            Self::Block(block) => block.page_uuid,
        }
    }
}

pub(crate) const PAGE_COLUMNS: &str = "pages.uuid, pages.title, pages.layout, pages.created_at, pages.updated_at, \
     (SELECT page_kind FROM page_identities WHERE page_uuid = pages.uuid), \
     (SELECT journal_date FROM page_identities WHERE page_uuid = pages.uuid), \
     COALESCE(pages.title_hlc, pages.existence_hlc)";
pub(crate) const BLOCK_COLUMNS: &str = "uuid, page_uuid, parent_uuid, order_key, style, markdown, created_at, updated_at, \
     COALESCE(markdown_hlc, existence_hlc)";
pub(crate) const QUALIFIED_BLOCK_COLUMNS: &str = "blocks.uuid, blocks.page_uuid, blocks.parent_uuid, blocks.order_key, blocks.style, \
     blocks.markdown, blocks.created_at, blocks.updated_at, \
     COALESCE(blocks.markdown_hlc, blocks.existence_hlc)";

pub(crate) fn row_to_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<Page> {
    let page_kind = row.get::<_, String>(5)?;
    let journal_date = row.get::<_, Option<crate::model::JournalDate>>(6)?;
    let kind = match (page_kind.as_str(), journal_date) {
        ("note", None) => PageKind::Note,
        ("journal", Some(date)) => PageKind::Journal { date },
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(Page {
        uuid: row.get(0)?,
        kind,
        title: row.get(1)?,
        layout: row.get(2)?,
        title_revision: row.get(7)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

pub(crate) fn row_to_block(row: &rusqlite::Row<'_>) -> rusqlite::Result<Block> {
    Ok(Block {
        uuid: row.get(0)?,
        page_uuid: row.get(1)?,
        parent_uuid: row.get(2)?,
        order_key: row.get(3)?,
        style: row.get(4)?,
        markdown: row.get(5)?,
        markdown_revision: row.get(8)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub(crate) fn row_blob_hash(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<BlobHash> {
    let value = row.get_ref(index)?;
    let bytes = value.as_blob()?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Blob,
            Box::new(error),
        )
    })?;
    Ok(BlobHash::from_bytes(bytes))
}

pub(crate) const fn blob_hash_bytes(hash: &BlobHash) -> &[u8] {
    hash.as_bytes().as_slice()
}

pub async fn get_content(conn: &Connection, uuid: uuid::Uuid) -> Result<Option<Content>> {
    if let Some(page) = get_page(conn, uuid).await? {
        return Ok(Some(Content::Page(page)));
    }
    Ok(get_block(conn, uuid).await?.map(Content::Block))
}

pub async fn get_contents(conn: &Connection, uuids: Vec<uuid::Uuid>) -> Result<Vec<Content>> {
    let mut records = Vec::new();
    for uuid in uuids {
        if let Some(record) = get_content(conn, uuid).await? {
            records.push(record);
        }
    }
    Ok(records)
}

#[doc(hidden)]
#[derive(Debug)]
pub struct AppliedMutation<T> {
    pub value: T,
    pub operations: Vec<OpKind>,
}

async fn apply_local(conn: &Connection, kinds: Vec<OpKind>) -> Result<Vec<OpKind>> {
    operation::apply_local_kinds(conn, kinds.clone()).await?;
    Ok(kinds)
}

async fn apply_local_action(
    conn: &Connection,
    action: &str,
    kinds: Vec<OpKind>,
) -> Result<Vec<OpKind>> {
    if kinds.is_empty() {
        return Ok(kinds);
    }
    let action = action.to_owned();
    conn.call_domain(move |database| -> crate::CoreResult<Vec<OpKind>> {
        let transaction = database.transaction()?;
        apply_local_action_in_transaction(&transaction, &action, kinds.clone())?;
        transaction.commit()?;
        Ok(kinds)
    })
    .await
}

pub(crate) fn apply_local_action_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    action: &str,
    kinds: Vec<OpKind>,
) -> crate::CoreResult<()> {
    let inverse = history::capture_inverse_kinds(transaction, &kinds)?;
    operation::apply_local_kinds_in_transaction(transaction, kinds.clone())?;
    history::record_action(transaction, action, kinds, inverse)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub content: Content,
    pub score: f64,
    #[serde(default)]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GraphItem {
    pub uuid: uuid::Uuid,
    pub kind: ObjectKind,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GraphRelation {
    Contains,
    PageLink,
    BlockReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub source_uuid: uuid::Uuid,
    pub target_uuid: uuid::Uuid,
    pub relation: GraphRelation,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct GraphSnapshot {
    pub items: Vec<GraphItem>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataArchive {
    pub format: String,
    pub version: u32,
    pub workspace_uuid: uuid::Uuid,
    pub exported_at: i64,
    pub page_identities: Vec<crate::operation::SnapshotPageIdentity>,
    pub page_aliases: Vec<crate::operation::SnapshotPageAlias>,
    pub pages: Vec<Page>,
    pub blocks: Vec<Block>,
    pub attachments: Vec<Attachment>,
    pub external_import_receipts: Vec<crate::ExternalImportReceipt>,
}

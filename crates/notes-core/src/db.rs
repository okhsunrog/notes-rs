use crate::model::{
    AttachmentOwner, BlockStyle, ObjectKind, OrderKey, PageKind, PageLayout, ReorderDirection,
    TaskState,
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
    attachment_path_ref_count, create_attachment, delete_attachment, get_attachment,
    list_attachments,
};
pub use blocks::{
    BlockContent, create_block, delete_block, get_block, get_blocks, indent_block,
    list_block_children, move_block, move_block_in_direction, outdent_block, reorder_block,
    set_block_content, set_block_style, set_task_state, split_block,
};
pub use graph::{find_backlinks, graph_snapshot, neighbors, read_ancestors, read_subtree};
pub use history::{HistoryStatus, history_status, redo_history, undo_history};
pub use journals::{
    JournalListLimit, append_to_journal, ensure_journal, get_journal, list_journals,
};
pub use pages::{
    CreatedNote, DeletedPage, create_note, create_page, delete_page, get_containing_page,
    get_or_create_page_by_title, get_page, get_page_by_title, list_pages, list_pages_filtered,
    rename_page, set_page_layout,
};
pub use search::{search_blocks_fts, search_fts, search_pages_by_title};
pub(crate) use workspace::transaction_workspace_uuid;
pub use workspace::workspace_uuid;

pub async fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let conn = Connection::open(path.as_ref())
        .await
        .context("opening sqlite database")?;
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
     (SELECT journal_date FROM page_identities WHERE page_uuid = pages.uuid)";
pub(crate) const BLOCK_COLUMNS: &str =
    "uuid, page_uuid, parent_uuid, order_key, style, markdown, created_at, updated_at";
pub(crate) const QUALIFIED_BLOCK_COLUMNS: &str = "blocks.uuid, blocks.page_uuid, blocks.parent_uuid, blocks.order_key, blocks.style, \
     blocks.markdown, blocks.created_at, blocks.updated_at";

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

async fn apply_local(conn: &Connection, kinds: Vec<OpKind>) -> Result<()> {
    operation::apply_local_kinds(conn, kinds).await
}

async fn apply_local_action(conn: &Connection, action: &str, kinds: Vec<OpKind>) -> Result<()> {
    if kinds.is_empty() {
        return Ok(());
    }
    let action = action.to_owned();
    conn.call_domain(move |database| -> crate::CoreResult<()> {
        let transaction = database.transaction()?;
        apply_local_action_in_transaction(&transaction, &action, kinds)?;
        transaction.commit()?;
        Ok(())
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

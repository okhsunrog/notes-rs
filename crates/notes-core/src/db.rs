use crate::model::{
    AttachmentOwner, BlockStyle, ObjectKind, OrderKey, PageLayout, ReorderDirection,
};
use crate::operation::{self, OpKind};
use crate::sqlite::Connection;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

mod archive;
mod attachments;
mod blocks;
mod graph;
mod history;
mod migrations;
mod pages;
mod search;

pub use archive::{export_archive, import_archive};
pub use attachments::{
    attachment_path_ref_count, create_attachment, delete_attachment, get_attachment,
    list_attachments,
};
pub use blocks::{
    BlockContent, create_block, delete_block, get_block, get_blocks, indent_block,
    list_block_children, move_block, move_block_in_direction, outdent_block, reorder_block,
    set_block_content, set_block_style, split_block,
};
pub use graph::{find_backlinks, graph_snapshot, neighbors, read_ancestors, read_subtree};
pub use history::{HistoryStatus, history_status, redo_history, undo_history};
pub use pages::{
    CreatedNote, DeletedPage, create_note, create_page, delete_page, get_containing_page,
    get_or_create_page_by_title, get_page, get_page_by_title, list_pages, rename_page,
    set_page_layout,
};
pub use search::{search_blocks_fts, search_fts, search_pages_by_title};

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
    Ok(conn)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub uuid: uuid::Uuid,
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
    pub blob_hash: String,
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

pub(crate) const PAGE_COLUMNS: &str = "uuid, title, layout, created_at, updated_at";
pub(crate) const BLOCK_COLUMNS: &str =
    "uuid, page_uuid, parent_uuid, order_key, style, markdown, created_at, updated_at";
pub(crate) const QUALIFIED_BLOCK_COLUMNS: &str = "blocks.uuid, blocks.page_uuid, blocks.parent_uuid, blocks.order_key, blocks.style, \
     blocks.markdown, blocks.created_at, blocks.updated_at";

pub(crate) fn row_to_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<Page> {
    Ok(Page {
        uuid: row.get(0)?,
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
    pub exported_at: i64,
    pub pages: Vec<Page>,
    pub blocks: Vec<Block>,
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, String>,
}

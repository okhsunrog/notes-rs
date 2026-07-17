use super::*;
use crate::PageListFilter;
use crate::operation::{
    BlockCreate, BlockDelete, PageCreate, PageDelete, PageSetLayout, PageSetTitle,
};
use rusqlite::OptionalExtension;

pub async fn get_page(conn: &Connection, uuid: uuid::Uuid) -> Result<Option<Page>> {
    conn.call(move |database| {
        let sql = format!("SELECT {PAGE_COLUMNS} FROM pages WHERE uuid = ?1");
        database.query_row(&sql, [uuid], row_to_page).optional()
    })
    .await
}

pub async fn get_page_by_title(conn: &Connection, title: String) -> Result<Option<Page>> {
    let title = title.trim().to_owned();
    if title.is_empty() {
        return Ok(None);
    }
    let normalized_title = crate::model::normalize_title(&title);
    conn.call(move |database| {
        let Some(page_uuid) = operation::resolve_page_alias(database, &normalized_title)? else {
            return Ok(None);
        };
        let sql = format!("SELECT {PAGE_COLUMNS} FROM pages WHERE uuid = ?1");
        database
            .query_row(&sql, [page_uuid], row_to_page)
            .optional()
    })
    .await
}

pub async fn list_pages(conn: &Connection, limit: u32) -> Result<Vec<Page>> {
    list_pages_filtered(conn, PageListFilter::Notes, limit).await
}

pub async fn list_pages_filtered(
    conn: &Connection,
    filter: PageListFilter,
    limit: u32,
) -> Result<Vec<Page>> {
    conn.call(move |database| {
        let predicate = match filter {
            PageListFilter::Notes => "WHERE page_identity.page_kind = 'note'",
            PageListFilter::Journals => "WHERE page_identity.page_kind = 'journal'",
            PageListFilter::All => "",
        };
        let sql = format!(
            "SELECT {PAGE_COLUMNS} FROM pages
               JOIN page_identities AS page_identity ON page_identity.page_uuid = pages.uuid
               {predicate}
              ORDER BY pages.updated_at DESC, pages.uuid LIMIT ?1"
        );
        database
            .prepare(&sql)?
            .query_map([limit], row_to_page)?
            .collect()
    })
    .await
}

pub async fn create_page(conn: &Connection, title: String) -> Result<Page> {
    let title = title.trim().to_owned();
    if title.is_empty() {
        return Err(crate::CoreError::invalid("title is required").into());
    }
    if let Some(page) = get_page_by_title(conn, title.clone()).await? {
        return Ok(page);
    }
    let uuid = uuid::Uuid::now_v7();
    apply_local_action(
        conn,
        "create page",
        vec![OpKind::PageCreate(PageCreate {
            uuid,
            kind: PageKind::Note,
            title: Some(title),
            layout: PageLayout::Outline,
            created_at: chrono::Utc::now().timestamp(),
        })],
    )
    .await?;
    get_page(conn, uuid)
        .await?
        .context("created page disappeared")
}

pub async fn get_or_create_page_by_title(conn: &Connection, title: String) -> Result<Page> {
    if let Some(page) = get_page_by_title(conn, title.clone()).await? {
        return Ok(page);
    }
    create_page(conn, title).await
}

pub async fn rename_page(
    conn: &Connection,
    uuid: uuid::Uuid,
    title: Option<String>,
) -> Result<Page> {
    let page = get_page(conn, uuid)
        .await?
        .ok_or_else(|| crate::CoreError::not_found("page not found"))?;
    if page.kind.is_journal() {
        return Err(
            crate::CoreError::invalid("journal page titles are derived from their date").into(),
        );
    }
    let title = title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty());
    if page.title != title {
        apply_local(
            conn,
            vec![OpKind::PageSetTitle(PageSetTitle { uuid, title })],
        )
        .await?;
    }
    get_page(conn, uuid)
        .await?
        .context("renamed page disappeared")
}

pub async fn set_page_layout(
    conn: &Connection,
    uuid: uuid::Uuid,
    layout: PageLayout,
) -> Result<Page> {
    let page = get_page(conn, uuid)
        .await?
        .ok_or_else(|| crate::CoreError::not_found("page not found"))?;
    if page.layout != layout {
        apply_local(
            conn,
            vec![OpKind::PageSetLayout(PageSetLayout { uuid, layout })],
        )
        .await?;
    }
    get_page(conn, uuid)
        .await?
        .context("updated page disappeared")
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CreatedNote {
    pub page: Page,
    pub initial_block: Block,
}

pub async fn create_note(conn: &Connection) -> Result<CreatedNote> {
    let page_uuid = uuid::Uuid::now_v7();
    let block_uuid = uuid::Uuid::now_v7();
    let now = chrono::Utc::now().timestamp();
    apply_local_action(
        conn,
        "create note",
        vec![
            OpKind::PageCreate(PageCreate {
                uuid: page_uuid,
                kind: PageKind::Note,
                title: None,
                layout: PageLayout::Outline,
                created_at: now,
            }),
            OpKind::BlockCreate(BlockCreate {
                uuid: block_uuid,
                page_uuid,
                parent_uuid: None,
                order_key: OrderKey::first(),
                style: BlockStyle::Paragraph,
                markdown: String::new(),
                created_at: now,
            }),
        ],
    )
    .await?;
    Ok(CreatedNote {
        page: get_page(conn, page_uuid)
            .await?
            .context("created page disappeared")?,
        initial_block: get_block(conn, block_uuid)
            .await?
            .context("created initial block disappeared")?,
    })
}

pub struct DeletedPage {
    pub page_uuid: uuid::Uuid,
    pub block_uuids: Vec<uuid::Uuid>,
    pub attachments: Vec<Attachment>,
}

pub async fn delete_page(conn: &Connection, uuid: uuid::Uuid) -> Result<Option<DeletedPage>> {
    conn.call_domain(move |database| -> crate::CoreResult<Option<DeletedPage>> {
        let transaction = database.transaction()?;
        let page_exists = transaction
            .query_row("SELECT 1 FROM pages WHERE uuid = ?1", [uuid], |_| Ok(()))
            .optional()?
            .is_some();
        if !page_exists {
            return Ok(None);
        }
        let blocks = {
            let sql = format!(
                "WITH RECURSIVE tree(uuid, depth) AS (
                   SELECT uuid, 0 FROM blocks
                    WHERE page_uuid = ?1 AND parent_uuid IS NULL
                   UNION ALL
                   SELECT child.uuid, tree.depth + 1
                     FROM tree JOIN blocks child ON child.parent_uuid = tree.uuid
                 )
                 SELECT {QUALIFIED_BLOCK_COLUMNS}
                   FROM tree JOIN blocks ON blocks.uuid = tree.uuid
                  ORDER BY tree.depth DESC, blocks.order_key, blocks.uuid"
            );
            let mut statement = transaction.prepare(&sql)?;
            statement
                .query_map([uuid], row_to_block)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let attachments = {
            let sql = format!(
                "SELECT {} FROM attachments
                  WHERE page_uuid = ?1
                     OR block_uuid IN (SELECT uuid FROM blocks WHERE page_uuid = ?1)
                  ORDER BY created_at, uuid",
                super::attachments::ATTACHMENT_COLUMNS
            );
            let mut statement = transaction.prepare(&sql)?;
            statement
                .query_map([uuid], super::attachments::row_to_attachment)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut kinds = attachments
            .iter()
            .map(|attachment| {
                OpKind::AttachmentRemove(crate::operation::AttachmentRemove {
                    owner: attachment.owner,
                    blob_hash: attachment.blob_hash,
                })
            })
            .collect::<Vec<_>>();
        kinds.extend(blocks.iter().map(|block| {
            OpKind::BlockDelete(BlockDelete {
                uuid: block.uuid,
                page_uuid: block.page_uuid,
            })
        }));
        kinds.push(OpKind::PageDelete(PageDelete { uuid }));
        apply_local_action_in_transaction(&transaction, "delete page", kinds)?;
        let deleted = DeletedPage {
            page_uuid: uuid,
            block_uuids: blocks.into_iter().map(|block| block.uuid).collect(),
            attachments,
        };
        transaction.commit()?;
        Ok(Some(deleted))
    })
    .await
}

pub async fn get_containing_page(
    conn: &Connection,
    block_uuid: uuid::Uuid,
) -> Result<Option<Page>> {
    conn.call(move |database| {
        let sql = format!(
            "SELECT {PAGE_COLUMNS} FROM pages
              WHERE uuid = (SELECT page_uuid FROM blocks WHERE uuid = ?1)"
        );
        database
            .query_row(&sql, [block_uuid], row_to_page)
            .optional()
    })
    .await
}

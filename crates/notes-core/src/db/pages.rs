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
    Ok(create_page_with_ops(conn, title).await?.value)
}

#[doc(hidden)]
pub async fn create_page_with_ops(
    conn: &Connection,
    title: String,
) -> Result<AppliedMutation<Page>> {
    let title = title.trim().to_owned();
    if title.is_empty() {
        return Err(crate::CoreError::invalid("title is required").into());
    }
    if let Some(page) = get_page_by_title(conn, title.clone()).await? {
        return Ok(AppliedMutation {
            value: page,
            operations: Vec::new(),
        });
    }
    let uuid = uuid::Uuid::now_v7();
    let operations = apply_local_action(
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
    let value = get_page(conn, uuid)
        .await?
        .context("created page disappeared")?;
    Ok(AppliedMutation { value, operations })
}

pub async fn get_or_create_page_by_title(conn: &Connection, title: String) -> Result<Page> {
    Ok(get_or_create_page_by_title_with_ops(conn, title)
        .await?
        .value)
}

#[doc(hidden)]
pub async fn get_or_create_page_by_title_with_ops(
    conn: &Connection,
    title: String,
) -> Result<AppliedMutation<Page>> {
    if let Some(page) = get_page_by_title(conn, title.clone()).await? {
        return Ok(AppliedMutation {
            value: page,
            operations: Vec::new(),
        });
    }
    create_page_with_ops(conn, title).await
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

/// Rename a page only when the caller still represents its current title.
/// The read, revision comparison, and local operation emission are atomic.
pub async fn rename_page_if_revision(
    conn: &Connection,
    uuid: uuid::Uuid,
    title: Option<String>,
    expected_revision: ContentRevision,
) -> Result<(Page, bool)> {
    let applied = rename_page_if_revision_with_ops(conn, uuid, title, expected_revision).await?;
    let changed = !applied.operations.is_empty();
    Ok((applied.value, changed))
}

#[doc(hidden)]
pub async fn rename_page_if_revision_with_ops(
    conn: &Connection,
    uuid: uuid::Uuid,
    title: Option<String>,
    expected_revision: ContentRevision,
) -> Result<AppliedMutation<Page>> {
    let title = title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty());
    conn.call_domain(
        move |database| -> crate::CoreResult<AppliedMutation<Page>> {
            let transaction = database.transaction()?;
            let sql = format!("SELECT {PAGE_COLUMNS} FROM pages WHERE uuid = ?1");
            let page = transaction
                .query_row(&sql, [uuid], row_to_page)
                .optional()?
                .ok_or_else(|| crate::CoreError::not_found("page not found"))?;
            if page.kind.is_journal() {
                return Err(crate::CoreError::invalid(
                    "journal page titles are derived from their date",
                ));
            }
            if page.title == title {
                transaction.commit()?;
                return Ok(AppliedMutation {
                    value: page,
                    operations: Vec::new(),
                });
            }
            if page.title_revision != expected_revision {
                return Err(crate::CoreError::conflict(
                    "page title changed since editing began",
                ));
            }
            if let Some(title) = title.as_deref() {
                let normalized_title = crate::model::normalize_title(title);
                if operation::resolve_page_alias(&transaction, &normalized_title)?
                    .is_some_and(|owner_uuid| owner_uuid != uuid)
                {
                    return Err(crate::CoreError::conflict(
                        "another page already owns this title",
                    ));
                }
            }
            let operations = vec![OpKind::PageSetTitle(PageSetTitle { uuid, title })];
            operation::apply_local_kinds_in_transaction(&transaction, operations.clone())?;
            let page = transaction.query_row(&sql, [uuid], row_to_page)?;
            transaction.commit()?;
            Ok(AppliedMutation {
                value: page,
                operations,
            })
        },
    )
    .await
}

pub async fn set_page_layout(
    conn: &Connection,
    uuid: uuid::Uuid,
    layout: PageLayout,
) -> Result<Page> {
    Ok(set_page_layout_with_ops(conn, uuid, layout).await?.value)
}

#[doc(hidden)]
pub async fn set_page_layout_with_ops(
    conn: &Connection,
    uuid: uuid::Uuid,
    layout: PageLayout,
) -> Result<AppliedMutation<Page>> {
    let page = get_page(conn, uuid)
        .await?
        .ok_or_else(|| crate::CoreError::not_found("page not found"))?;
    let operations = if page.layout != layout {
        apply_local(
            conn,
            vec![OpKind::PageSetLayout(PageSetLayout { uuid, layout })],
        )
        .await?
    } else {
        Vec::new()
    };
    let value = get_page(conn, uuid)
        .await?
        .context("updated page disappeared")?;
    Ok(AppliedMutation { value, operations })
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CreatedNote {
    pub page: Page,
    pub initial_block: Block,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CreateNoteResult {
    Created { note: CreatedNote },
    Existing { page: Page },
}

impl CreateNoteResult {
    pub fn into_created(self) -> Option<CreatedNote> {
        match self {
            Self::Created { note } => Some(note),
            Self::Existing { .. } => None,
        }
    }
}

pub async fn create_note(conn: &Connection, title: Option<String>) -> Result<CreateNoteResult> {
    Ok(create_note_with_ops(conn, title).await?.value)
}

#[doc(hidden)]
pub async fn create_note_with_ops(
    conn: &Connection,
    title: Option<String>,
) -> Result<AppliedMutation<CreateNoteResult>> {
    let title = title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty());
    conn.call_domain(
        move |database| -> crate::CoreResult<AppliedMutation<CreateNoteResult>> {
            let transaction = database.transaction()?;
            if let Some(normalized_title) = title.as_deref().map(crate::model::normalize_title)
                && let Some(page_uuid) =
                    operation::resolve_page_alias(&transaction, &normalized_title)?
            {
                let sql = format!("SELECT {PAGE_COLUMNS} FROM pages WHERE uuid = ?1");
                let page = transaction.query_row(&sql, [page_uuid], row_to_page)?;
                transaction.commit()?;
                return Ok(AppliedMutation {
                    value: CreateNoteResult::Existing { page },
                    operations: Vec::new(),
                });
            }

            let page_uuid = uuid::Uuid::now_v7();
            let block_uuid = uuid::Uuid::now_v7();
            let now = chrono::Utc::now().timestamp();
            let operations = vec![
                OpKind::PageCreate(PageCreate {
                    uuid: page_uuid,
                    kind: PageKind::Note,
                    title,
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
            ];
            apply_local_action_in_transaction(&transaction, "create note", operations.clone())?;
            let page = transaction
                .query_row(
                    &format!("SELECT {PAGE_COLUMNS} FROM pages WHERE uuid = ?1"),
                    [page_uuid],
                    row_to_page,
                )
                .optional()?
                .ok_or_else(|| crate::CoreError::not_found("created page disappeared"))?;
            let initial_block = transaction
                .query_row(
                    &format!("SELECT {BLOCK_COLUMNS} FROM blocks WHERE uuid = ?1"),
                    [block_uuid],
                    row_to_block,
                )
                .optional()?
                .ok_or_else(|| crate::CoreError::not_found("created initial block disappeared"))?;
            transaction.commit()?;
            Ok(AppliedMutation {
                value: CreateNoteResult::Created {
                    note: CreatedNote {
                        page,
                        initial_block,
                    },
                },
                operations,
            })
        },
    )
    .await
}

pub struct DeletedPage {
    pub page_uuid: uuid::Uuid,
    pub block_uuids: Vec<uuid::Uuid>,
    pub attachments: Vec<Attachment>,
    pub operations: Vec<OpKind>,
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
        apply_local_action_in_transaction(&transaction, "delete page", kinds.clone())?;
        let deleted = DeletedPage {
            page_uuid: uuid,
            block_uuids: blocks.into_iter().map(|block| block.uuid).collect(),
            attachments,
            operations: kinds,
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

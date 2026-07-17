use super::*;
use crate::operation::{
    AttachmentAdd, AttachmentRemove, BlockCreate, BlockDelete, PageCreate, PageDelete,
};

pub const ARCHIVE_FORMAT: &str = "notes-rs";
pub const ARCHIVE_VERSION: u32 = 3;

pub async fn export_archive(conn: &Connection) -> Result<DataArchive> {
    conn.call(|database| {
        let pages = {
            let sql = format!("SELECT {PAGE_COLUMNS} FROM pages ORDER BY uuid");
            database
                .prepare(&sql)?
                .query_map([], row_to_page)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let blocks = {
            let sql = format!("SELECT {BLOCK_COLUMNS} FROM blocks ORDER BY page_uuid, parent_uuid, order_key, uuid");
            database
                .prepare(&sql)?
                .query_map([], row_to_block)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let attachments = {
            let mut statement = database.prepare(
                "SELECT uuid, page_uuid, block_uuid, blob_hash, filename, mime, size, created_at
                   FROM attachments ORDER BY uuid",
            )?;
            statement
                .query_map([], |row| {
                    let owner = match (
                        row.get::<_, Option<uuid::Uuid>>(1)?,
                        row.get::<_, Option<uuid::Uuid>>(2)?,
                    ) {
                        (Some(uuid), None) => AttachmentOwner::Page(uuid),
                        (None, Some(uuid)) => AttachmentOwner::Block(uuid),
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    };
                    let size = u64::try_from(row.get::<_, i64>(6)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                    Ok(Attachment {
                        uuid: row.get(0)?,
                        owner,
                        blob_hash: row.get(3)?,
                        filename: row.get(4)?,
                        mime: row.get(5)?,
                        size,
                        created_at: row.get(7)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(DataArchive {
            format: ARCHIVE_FORMAT.into(),
            version: ARCHIVE_VERSION,
            exported_at: chrono::Utc::now().timestamp(),
            pages,
            blocks,
            attachments,
            files: Default::default(),
        })
    })
    .await
}

pub async fn import_archive(conn: &Connection, archive: DataArchive) -> Result<()> {
    if archive.format != ARCHIVE_FORMAT || archive.version != ARCHIVE_VERSION {
        return Err(crate::CoreError::invalid(format!(
            "unsupported archive format {} version {}",
            archive.format, archive.version
        ))
        .into());
    }
    conn.call_domain(move |database| -> crate::CoreResult<()> {
        let transaction = database.transaction()?;
        let current_attachments = transaction
            .prepare("SELECT page_uuid, block_uuid, blob_hash FROM attachments ORDER BY uuid")?
            .query_map([], |row| {
                let owner = match (
                    row.get::<_, Option<uuid::Uuid>>(0)?,
                    row.get::<_, Option<uuid::Uuid>>(1)?,
                ) {
                    (Some(uuid), None) => AttachmentOwner::Page(uuid),
                    (None, Some(uuid)) => AttachmentOwner::Block(uuid),
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok((owner, row.get::<_, String>(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let current_blocks = transaction
            .prepare("SELECT uuid, page_uuid FROM blocks ORDER BY uuid")?
            .query_map([], |row| {
                Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, uuid::Uuid>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let current_pages = transaction
            .prepare("SELECT uuid FROM pages ORDER BY uuid")?
            .query_map([], |row| row.get::<_, uuid::Uuid>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut kinds = current_attachments
            .into_iter()
            .map(|(owner, blob_hash)| {
                OpKind::AttachmentRemove(AttachmentRemove { owner, blob_hash })
            })
            .collect::<Vec<_>>();
        kinds.extend(
            current_blocks
                .into_iter()
                .map(|(uuid, page_uuid)| OpKind::BlockDelete(BlockDelete { uuid, page_uuid })),
        );
        kinds.extend(
            current_pages
                .into_iter()
                .map(|uuid| OpKind::PageDelete(PageDelete { uuid })),
        );
        kinds.extend(archive.pages.into_iter().map(|page| {
            OpKind::PageCreate(PageCreate {
                uuid: page.uuid,
                title: page.title,
                layout: page.layout,
                created_at: page.created_at,
            })
        }));
        kinds.extend(archive.blocks.into_iter().map(|block| {
            OpKind::BlockCreate(BlockCreate {
                uuid: block.uuid,
                page_uuid: block.page_uuid,
                parent_uuid: block.parent_uuid,
                order_key: block.order_key,
                style: block.style,
                markdown: block.markdown,
                created_at: block.created_at,
            })
        }));
        kinds.extend(archive.attachments.into_iter().map(|attachment| {
            OpKind::AttachmentAdd(AttachmentAdd {
                owner: attachment.owner,
                blob_hash: attachment.blob_hash,
                filename: attachment.filename,
                mime: attachment.mime,
                size: attachment.size,
            })
        }));
        operation::apply_local_kinds_in_transaction(&transaction, kinds)?;
        transaction.execute("DELETE FROM history_undo", [])?;
        transaction.execute("DELETE FROM history_redo", [])?;
        transaction.commit()?;
        Ok(())
    })
    .await
}

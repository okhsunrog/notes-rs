use super::*;
use crate::operation::{AttachmentAdd, AttachmentRemove};
use rusqlite::OptionalExtension;

pub(crate) fn row_to_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<Attachment> {
    let page_uuid = row.get::<_, Option<uuid::Uuid>>(1)?;
    let block_uuid = row.get::<_, Option<uuid::Uuid>>(2)?;
    let owner = match (page_uuid, block_uuid) {
        (Some(uuid), None) => AttachmentOwner::Page(uuid),
        (None, Some(uuid)) => AttachmentOwner::Block(uuid),
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                1,
                "attachment owner".into(),
                rusqlite::types::Type::Null,
            ));
        }
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
        blob_hash: row_blob_hash(row, 3)?,
        filename: row.get(4)?,
        mime: row.get(5)?,
        size,
        created_at: row.get(7)?,
    })
}

pub(crate) const ATTACHMENT_COLUMNS: &str =
    "uuid, page_uuid, block_uuid, blob_hash, filename, mime, size, created_at";

pub async fn get_attachment(conn: &Connection, uuid: uuid::Uuid) -> Result<Option<Attachment>> {
    conn.call(move |database| {
        let sql = format!("SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE uuid = ?1");
        database
            .query_row(&sql, [uuid], row_to_attachment)
            .optional()
    })
    .await
}

pub async fn create_attachment(
    conn: &Connection,
    owner: AttachmentOwner,
    blob_hash: BlobHash,
    filename: String,
    mime: String,
    size: u64,
) -> Result<Attachment> {
    let owner_exists = match owner {
        AttachmentOwner::Page(uuid) => get_page(conn, uuid).await?.is_some(),
        AttachmentOwner::Block(uuid) => get_block(conn, uuid).await?.is_some(),
    };
    if !owner_exists {
        return Err(crate::CoreError::not_found("attachment owner was not found").into());
    }
    apply_local(
        conn,
        vec![OpKind::AttachmentAdd(AttachmentAdd {
            owner,
            blob_hash,
            filename,
            mime,
            size,
        })],
    )
    .await?;
    conn.call(move |database| {
        let sql = format!(
            "SELECT {ATTACHMENT_COLUMNS} FROM attachments
              WHERE blob_hash = ?1 AND ((page_uuid = ?2) OR (block_uuid = ?2))"
        );
        database.query_row(
            &sql,
            rusqlite::params![blob_hash_bytes(&blob_hash), owner.uuid()],
            row_to_attachment,
        )
    })
    .await
}

pub async fn list_attachments(
    conn: &Connection,
    owner: AttachmentOwner,
) -> Result<Vec<Attachment>> {
    conn.call(move |database| {
        let column = match owner {
            AttachmentOwner::Page(_) => "page_uuid",
            AttachmentOwner::Block(_) => "block_uuid",
        };
        let sql = format!(
            "SELECT {ATTACHMENT_COLUMNS} FROM attachments
              WHERE {column} = ?1 ORDER BY created_at, uuid"
        );
        database
            .prepare(&sql)?
            .query_map([owner.uuid()], row_to_attachment)?
            .collect()
    })
    .await
}

pub async fn attachment_path_ref_count(
    conn: &Connection,
    blob_hash: BlobHash,
    filename: String,
) -> Result<i64> {
    conn.call(move |database| {
        database.query_row(
            "SELECT COUNT(*) FROM attachments WHERE blob_hash = ?1 AND filename = ?2",
            rusqlite::params![blob_hash_bytes(&blob_hash), filename],
            |row| row.get(0),
        )
    })
    .await
}

pub async fn delete_attachment(conn: &Connection, uuid: uuid::Uuid) -> Result<Option<Attachment>> {
    let attachment = get_attachment(conn, uuid).await?;
    let Some(attachment) = attachment else {
        return Ok(None);
    };
    apply_local(
        conn,
        vec![OpKind::AttachmentRemove(AttachmentRemove {
            owner: attachment.owner,
            blob_hash: attachment.blob_hash,
        })],
    )
    .await?;
    Ok(Some(attachment))
}

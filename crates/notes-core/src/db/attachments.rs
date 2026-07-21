use super::*;
use crate::operation::{AttachmentAdd, AttachmentRemove};
use rusqlite::OptionalExtension;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentImageCache {
    pub blob_hash: BlobHash,
    pub format_version: u32,
    pub byte_size: u64,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub preview_hash: Option<BlobHash>,
    pub preview_size: Option<u64>,
    pub preview_width: Option<u32>,
    pub preview_height: Option<u32>,
}

fn integer<T>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<T>
where
    T: TryFrom<i64>,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    T::try_from(row.get::<_, i64>(index)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn optional_integer<T>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<T>>
where
    T: TryFrom<i64>,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            T::try_from(value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

fn row_to_image_cache(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentImageCache> {
    Ok(AttachmentImageCache {
        blob_hash: row_blob_hash(row, 0)?,
        format_version: integer(row, 1)?,
        byte_size: integer(row, 2)?,
        mime: row.get(3)?,
        width: integer(row, 4)?,
        height: integer(row, 5)?,
        preview_hash: row
            .get_ref(6)?
            .as_blob_or_null()?
            .map(|bytes| {
                let bytes: [u8; 32] = bytes.try_into().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Blob,
                        Box::new(error),
                    )
                })?;
                Ok::<BlobHash, rusqlite::Error>(BlobHash::from_bytes(bytes))
            })
            .transpose()?,
        preview_size: optional_integer(row, 7)?,
        preview_width: optional_integer(row, 8)?,
        preview_height: optional_integer(row, 9)?,
    })
}

pub async fn get_attachment_image_cache(
    conn: &Connection,
    blob_hash: BlobHash,
) -> Result<Option<AttachmentImageCache>> {
    conn.call(move |database| {
        database
            .query_row(
                "SELECT blob_hash, format_version, byte_size, mime, width, height,
                        preview_hash, preview_size, preview_width, preview_height
                   FROM attachment_image_cache WHERE blob_hash = ?1",
                [blob_hash_bytes(&blob_hash)],
                row_to_image_cache,
            )
            .optional()
    })
    .await
}

pub async fn upsert_attachment_image_cache(
    conn: &Connection,
    cached: AttachmentImageCache,
) -> Result<()> {
    let byte_size = i64::try_from(cached.byte_size).context("cached image size fits SQLite")?;
    let preview_size = cached
        .preview_size
        .map(i64::try_from)
        .transpose()
        .context("cached preview size fits SQLite")?;
    conn.call(move |database| {
        database.execute(
            "INSERT INTO attachment_image_cache(
               blob_hash, format_version, byte_size, mime, width, height,
               preview_hash, preview_size, preview_width, preview_height
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(blob_hash) DO UPDATE SET
               format_version = excluded.format_version,
               byte_size = excluded.byte_size,
               mime = excluded.mime,
               width = excluded.width,
               height = excluded.height,
               preview_hash = excluded.preview_hash,
               preview_size = excluded.preview_size,
               preview_width = excluded.preview_width,
               preview_height = excluded.preview_height",
            rusqlite::params![
                blob_hash_bytes(&cached.blob_hash),
                i64::from(cached.format_version),
                byte_size,
                cached.mime,
                i64::from(cached.width),
                i64::from(cached.height),
                cached.preview_hash.as_ref().map(blob_hash_bytes),
                preview_size,
                cached.preview_width.map(i64::from),
                cached.preview_height.map(i64::from),
            ],
        )?;
        Ok(())
    })
    .await
}

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
    Ok(
        create_attachment_with_ops(conn, owner, blob_hash, filename, mime, size)
            .await?
            .value,
    )
}

#[doc(hidden)]
pub async fn create_attachment_with_ops(
    conn: &Connection,
    owner: AttachmentOwner,
    blob_hash: BlobHash,
    filename: String,
    mime: String,
    size: u64,
) -> Result<AppliedMutation<Attachment>> {
    let owner_exists = match owner {
        AttachmentOwner::Page(uuid) => get_page(conn, uuid).await?.is_some(),
        AttachmentOwner::Block(uuid) => get_block(conn, uuid).await?.is_some(),
    };
    if !owner_exists {
        return Err(crate::CoreError::not_found("attachment owner was not found").into());
    }
    let operations = apply_local(
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
    let value = conn
        .call(move |database| {
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
        .await?;
    Ok(AppliedMutation { value, operations })
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

/// Runs host-owned cleanup for candidate blobs that have no live attachment
/// metadata while holding the serialized SQLite worker boundary.
///
/// This prevents an attachment mutation from racing between the reference
/// check and filesystem removal. Candidates are sorted and deduplicated before
/// the callback is invoked.
pub async fn cleanup_unreferenced_attachment_blobs<F>(
    conn: &Connection,
    mut candidates: Vec<BlobHash>,
    mut cleanup: F,
) -> Result<u64>
where
    F: FnMut(BlobHash) -> Result<()> + Send + 'static,
{
    candidates.sort_unstable();
    candidates.dedup();
    conn.call_domain(move |database| -> Result<u64> {
        let mut referenced =
            database.prepare("SELECT EXISTS(SELECT 1 FROM attachments WHERE blob_hash = ?1)")?;
        let mut cleaned = 0_u64;
        for blob_hash in candidates {
            let is_referenced: bool =
                referenced.query_row([blob_hash_bytes(&blob_hash)], |row| row.get(0))?;
            if !is_referenced {
                cleanup(blob_hash)?;
                cleaned += 1;
            }
        }
        Ok(cleaned)
    })
    .await
}

pub async fn delete_attachment(conn: &Connection, uuid: uuid::Uuid) -> Result<Option<Attachment>> {
    Ok(delete_attachment_with_ops(conn, uuid).await?.value)
}

#[doc(hidden)]
pub async fn delete_attachment_with_ops(
    conn: &Connection,
    uuid: uuid::Uuid,
) -> Result<AppliedMutation<Option<Attachment>>> {
    let attachment = get_attachment(conn, uuid).await?;
    let Some(attachment) = attachment else {
        return Ok(AppliedMutation {
            value: None,
            operations: Vec::new(),
        });
    };
    let operations = apply_local(
        conn,
        vec![OpKind::AttachmentRemove(AttachmentRemove {
            owner: attachment.owner,
            blob_hash: attachment.blob_hash,
        })],
    )
    .await?;
    Ok(AppliedMutation {
        value: Some(attachment),
        operations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn image_cache_round_trips_source_and_preview_metadata() {
        let connection = super::super::open_in_memory().await.expect("database");
        let source_hash = BlobHash::digest(b"source image");
        let preview_hash = BlobHash::digest(b"preview image");
        let cached = AttachmentImageCache {
            blob_hash: source_hash,
            format_version: 1,
            byte_size: 12_345,
            mime: "image/png".into(),
            width: 1_920,
            height: 1_080,
            preview_hash: Some(preview_hash),
            preview_size: Some(4_321),
            preview_width: Some(1_024),
            preview_height: Some(576),
        };

        upsert_attachment_image_cache(&connection, cached.clone())
            .await
            .expect("store image metadata");
        assert_eq!(
            get_attachment_image_cache(&connection, source_hash)
                .await
                .expect("read image metadata"),
            Some(cached)
        );
    }
}

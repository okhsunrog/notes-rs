use super::{BLOCK_COLUMNS, Block, row_to_block};
use crate::{CoreError, CoreResult, DocumentRevision, Hlc, sqlite::Connection};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const DOCUMENT_REVISION_DOMAIN: &[u8] = b"notes-rs/page-document/v1";

/// One transactionally consistent, deterministic projection of a page's
/// complete current block tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PageDocumentSnapshot {
    pub page_uuid: uuid::Uuid,
    pub revision: DocumentRevision,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
struct RevisionBlock {
    block: Block,
    markdown_hlc: Option<Hlc>,
    style_hlc: Option<Hlc>,
    structure_hlc: Option<Hlc>,
    existence_hlc: Hlc,
}

/// Read the whole page document from one SQLite snapshot. Persisted page and
/// block queries deliberately share the same transaction so a concurrent WAL
/// writer cannot produce a page generation from one state and blocks from
/// another.
pub async fn get_page_document(
    conn: &Connection,
    page_uuid: uuid::Uuid,
) -> anyhow::Result<Option<PageDocumentSnapshot>> {
    conn.call_domain(
        move |database| -> CoreResult<Option<PageDocumentSnapshot>> {
            let transaction = database.transaction()?;
            let snapshot = read_page_document(&transaction, page_uuid)?;
            transaction.commit()?;
            Ok(snapshot)
        },
    )
    .await
}

fn read_page_document(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
) -> CoreResult<Option<PageDocumentSnapshot>> {
    let generation = transaction
        .query_row(
            "SELECT existence_hlc FROM pages WHERE uuid = ?1",
            [page_uuid],
            |row| crate::operation::sql_hlc(row.get(0)?, 0),
        )
        .optional()?;
    let Some(generation) = generation else {
        return Ok(None);
    };

    let rows = {
        let sql = format!(
            "SELECT {BLOCK_COLUMNS}, markdown_hlc, style_hlc, structure_hlc, existence_hlc
               FROM blocks
              WHERE page_uuid = ?1
              ORDER BY order_key, uuid"
        );
        let mut statement = transaction.prepare(&sql)?;
        statement
            .query_map([page_uuid], row_to_revision_block)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let rows = deterministic_preorder(rows)?;
    let revision = document_revision(page_uuid, &generation, &rows);
    Ok(Some(PageDocumentSnapshot {
        page_uuid,
        revision,
        blocks: rows.into_iter().map(|row| row.block).collect(),
    }))
}

fn row_to_revision_block(row: &rusqlite::Row<'_>) -> rusqlite::Result<RevisionBlock> {
    Ok(RevisionBlock {
        block: row_to_block(row)?,
        markdown_hlc: optional_hlc(row.get(9)?, 9)?,
        style_hlc: optional_hlc(row.get(10)?, 10)?,
        structure_hlc: optional_hlc(row.get(11)?, 11)?,
        existence_hlc: crate::operation::sql_hlc(row.get(12)?, 12)?,
    })
}

fn optional_hlc(value: Option<String>, index: usize) -> rusqlite::Result<Option<Hlc>> {
    value
        .map(|value| crate::operation::sql_hlc(value, index))
        .transpose()
}

fn deterministic_preorder(rows: Vec<RevisionBlock>) -> CoreResult<Vec<RevisionBlock>> {
    let expected_len = rows.len();
    let mut by_uuid = HashMap::with_capacity(expected_len);
    let mut children = HashMap::<Option<uuid::Uuid>, Vec<uuid::Uuid>>::new();
    for row in rows {
        let uuid = row.block.uuid;
        children
            .entry(row.block.parent_uuid)
            .or_default()
            .push(uuid);
        if by_uuid.insert(uuid, row).is_some() {
            return Err(CoreError::conflict(
                "page document contains a duplicate block UUID",
            ));
        }
    }

    let mut stack = children.get(&None).cloned().unwrap_or_default();
    stack.reverse();
    let mut ordered = Vec::with_capacity(expected_len);
    while let Some(uuid) = stack.pop() {
        let row = by_uuid.remove(&uuid).ok_or_else(|| {
            CoreError::conflict("page document contains a repeated structural membership")
        })?;
        if let Some(descendants) = children.get(&Some(uuid)) {
            stack.extend(descendants.iter().rev().copied());
        }
        ordered.push(row);
    }

    if !by_uuid.is_empty() {
        return Err(CoreError::conflict(
            "page document contains an orphaned or cyclic block structure",
        ));
    }
    Ok(ordered)
}

fn document_revision(
    page_uuid: uuid::Uuid,
    generation: &Hlc,
    blocks: &[RevisionBlock],
) -> DocumentRevision {
    let mut digest = Sha256::new();
    digest_field(&mut digest, DOCUMENT_REVISION_DOMAIN);
    digest_field(&mut digest, page_uuid.as_bytes());
    digest_field(&mut digest, generation.to_string().as_bytes());
    digest_field(&mut digest, &(blocks.len() as u64).to_be_bytes());
    for row in blocks {
        let block = &row.block;
        digest_field(&mut digest, block.uuid.as_bytes());
        digest_field(&mut digest, block.page_uuid.as_bytes());
        digest_optional_uuid(&mut digest, block.parent_uuid);
        digest_field(&mut digest, block.order_key.to_string().as_bytes());
        digest_field(&mut digest, block.style.storage_value().as_bytes());
        digest_field(&mut digest, block.markdown.as_bytes());
        digest_optional_hlc(&mut digest, row.markdown_hlc.as_ref());
        digest_optional_hlc(&mut digest, row.style_hlc.as_ref());
        digest_optional_hlc(&mut digest, row.structure_hlc.as_ref());
        digest_field(&mut digest, row.existence_hlc.to_string().as_bytes());
    }
    DocumentRevision::from_digest(digest.finalize().into())
}

fn digest_optional_uuid(digest: &mut Sha256, value: Option<uuid::Uuid>) {
    match value {
        None => digest_field(digest, &[0]),
        Some(value) => {
            digest_field(digest, &[1]);
            digest_field(digest, value.as_bytes());
        }
    }
}

fn digest_optional_hlc(digest: &mut Sha256, value: Option<&Hlc>) {
    match value {
        None => digest_field(digest, &[0]),
        Some(value) => {
            digest_field(digest, &[1]);
            digest_field(digest, value.to_string().as_bytes());
        }
    }
}

fn digest_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockStyle, db};

    #[tokio::test]
    async fn one_read_transaction_does_not_mix_a_concurrent_wal_write() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("notes.db");
        let connection = db::open(&path).await.expect("open database");
        let note = db::create_note(&connection).await.expect("create note");
        let block_uuid = note.initial_block.uuid;
        let page_uuid = note.page.uuid;
        let path_for_writer = path.clone();

        let during_write = connection
            .call_domain(move |database| -> CoreResult<PageDocumentSnapshot> {
                let transaction = database.transaction()?;
                let before = transaction.query_row(
                    "SELECT markdown FROM blocks WHERE uuid = ?1",
                    [block_uuid],
                    |row| row.get::<_, String>(0),
                )?;
                assert!(before.is_empty());

                std::thread::spawn(move || {
                    let writer = rusqlite::Connection::open(path_for_writer).expect("open writer");
                    writer
                        .execute(
                            "UPDATE blocks
                                SET markdown = 'concurrent',
                                    style = 'bullet',
                                    markdown_hlc = '0000000000000002-00000000-00000000000000000000000000000002',
                                    style_hlc = '0000000000000002-00000001-00000000000000000000000000000002'
                              WHERE uuid = ?1",
                            [block_uuid],
                        )
                        .expect("concurrent WAL update");
                })
                .join()
                .expect("join writer");

                let snapshot = read_page_document(&transaction, page_uuid)?
                    .expect("page exists in reader snapshot");
                transaction.commit()?;
                Ok(snapshot)
            })
            .await
            .expect("read during concurrent write");
        assert_eq!(during_write.blocks[0].markdown, "");
        assert_eq!(during_write.blocks[0].style, BlockStyle::Paragraph);

        let after_write = get_page_document(&connection, page_uuid)
            .await
            .expect("read after write")
            .expect("page remains");
        assert_eq!(after_write.blocks[0].markdown, "concurrent");
        assert_eq!(after_write.blocks[0].style, BlockStyle::Bullet);
        assert_ne!(during_write.revision, after_write.revision);
    }
}

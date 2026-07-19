use super::{BLOCK_COLUMNS, Block, apply_local_action_in_transaction, row_to_block};
use crate::operation::{
    BlockCreate, BlockDelete, BlockMove, BlockSetMarkdown, BlockSetStyle, OpKind,
};
use crate::{
    BlockStyle, CoreError, CoreResult, DocumentRevision, Hlc, OrderKey, sqlite::Connection,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};

const DOCUMENT_REVISION_DOMAIN: &[u8] = b"notes-rs/page-document/v1";

/// Semantic limits for one atomic continuous-document edit. They bound IPC
/// decoding work, operation batches, history rows, and pathological tree input.
pub const MAX_DOCUMENT_UNITS: usize = 20_000;
pub const MAX_DOCUMENT_MARKDOWN_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DOCUMENT_DEPTH: u32 = 128;

/// One transactionally consistent, deterministic projection of a page's
/// complete current block tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PageDocumentSnapshot {
    pub page_uuid: uuid::Uuid,
    pub revision: DocumentRevision,
    pub blocks: Vec<Block>,
}

/// One desired semantic unit in a complete document replacement. Existing
/// units retain their UUID; new units receive a UUIDv7 only after the complete
/// draft graph has passed validation.
#[derive(Debug, Clone, Deserialize, PartialEq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DocumentUnitDraft {
    pub previous_uuid: Option<uuid::Uuid>,
    pub parent_index: Option<u32>,
    pub style: BlockStyle,
    pub markdown: String,
}

/// Host-only mutation metadata used to emit precise Tauri invalidations while
/// the public document intent returns only its fresh snapshot.
#[derive(Debug)]
pub struct PageDocumentReplaceOutcome {
    pub snapshot: PageDocumentSnapshot,
    pub changed: bool,
    pub block_uuids: Vec<uuid::Uuid>,
    pub deleted_block_uuids: Vec<uuid::Uuid>,
    pub container_uuids: Vec<uuid::Uuid>,
    pub structure_changed: bool,
    pub graph_changed: bool,
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

/// Atomically replace one page's complete current block document when the
/// caller still owns the exact snapshot revision it edited.
pub async fn replace_page_document(
    conn: &Connection,
    page_uuid: uuid::Uuid,
    expected_revision: DocumentRevision,
    units: Vec<DocumentUnitDraft>,
) -> anyhow::Result<PageDocumentSnapshot> {
    Ok(
        replace_page_document_with_outcome(conn, page_uuid, expected_revision, units)
            .await?
            .snapshot,
    )
}

#[doc(hidden)]
pub async fn replace_page_document_with_outcome(
    conn: &Connection,
    page_uuid: uuid::Uuid,
    expected_revision: DocumentRevision,
    units: Vec<DocumentUnitDraft>,
) -> anyhow::Result<PageDocumentReplaceOutcome> {
    conn.call_domain(move |database| -> CoreResult<PageDocumentReplaceOutcome> {
        let transaction = database.transaction()?;
        let current = read_page_document(&transaction, page_uuid)?
            .ok_or_else(|| CoreError::not_found("page was not found"))?;
        if current.revision != expected_revision {
            return Err(CoreError::conflict(
                "page document changed since editing began",
            ));
        }

        validate_document_units(&transaction, page_uuid, &current, &units)?;
        let plan = plan_document_replace(page_uuid, &current, units);
        if plan.kinds.is_empty() {
            transaction.commit()?;
            return Ok(PageDocumentReplaceOutcome {
                snapshot: current,
                changed: false,
                block_uuids: Vec::new(),
                deleted_block_uuids: Vec::new(),
                container_uuids: Vec::new(),
                structure_changed: false,
                graph_changed: false,
            });
        }

        apply_local_action_in_transaction(&transaction, "edit document", plan.kinds)?;
        let snapshot = read_page_document(&transaction, page_uuid)?
            .ok_or_else(|| CoreError::not_found("edited page disappeared"))?;
        transaction.commit()?;
        Ok(PageDocumentReplaceOutcome {
            snapshot,
            changed: true,
            block_uuids: plan.block_uuids.into_iter().collect(),
            deleted_block_uuids: plan.deleted_block_uuids.into_iter().collect(),
            container_uuids: plan.container_uuids.into_iter().collect(),
            structure_changed: plan.structure_changed,
            graph_changed: plan.graph_changed,
        })
    })
    .await
}

struct DocumentReplacePlan {
    kinds: Vec<OpKind>,
    block_uuids: BTreeSet<uuid::Uuid>,
    deleted_block_uuids: BTreeSet<uuid::Uuid>,
    container_uuids: BTreeSet<uuid::Uuid>,
    structure_changed: bool,
    graph_changed: bool,
}

#[derive(Debug)]
struct DesiredUnit {
    uuid: uuid::Uuid,
    parent_uuid: Option<uuid::Uuid>,
    order_key: Option<OrderKey>,
    style: BlockStyle,
    markdown: String,
    existing: bool,
}

fn validate_document_units(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
    current: &PageDocumentSnapshot,
    units: &[DocumentUnitDraft],
) -> CoreResult<()> {
    if units.len() > MAX_DOCUMENT_UNITS {
        return Err(CoreError::invalid(format!(
            "page document exceeds the {MAX_DOCUMENT_UNITS} unit limit"
        )));
    }

    let mut markdown_bytes = 0_usize;
    let mut depths = Vec::with_capacity(units.len());
    let mut previous_uuids = HashSet::with_capacity(units.len());
    let current_uuids = current
        .blocks
        .iter()
        .map(|block| block.uuid)
        .collect::<HashSet<_>>();
    for (index, unit) in units.iter().enumerate() {
        markdown_bytes = markdown_bytes
            .checked_add(unit.markdown.len())
            .ok_or_else(|| CoreError::invalid("page document Markdown size overflow"))?;
        if markdown_bytes > MAX_DOCUMENT_MARKDOWN_BYTES {
            return Err(CoreError::invalid(format!(
                "page document exceeds the {MAX_DOCUMENT_MARKDOWN_BYTES} byte Markdown limit"
            )));
        }

        let depth = match unit.parent_index {
            None => 1,
            Some(parent_index) => {
                let parent_index = usize::try_from(parent_index).map_err(|_| {
                    CoreError::invalid("document parent index does not fit this platform")
                })?;
                if parent_index >= index {
                    return Err(CoreError::invalid(
                        "document parent index must reference an earlier unit",
                    ));
                }
                depths[parent_index] + 1
            }
        };
        if depth > MAX_DOCUMENT_DEPTH {
            return Err(CoreError::invalid(format!(
                "page document exceeds the {MAX_DOCUMENT_DEPTH} level depth limit"
            )));
        }
        depths.push(depth);

        let Some(previous_uuid) = unit.previous_uuid else {
            continue;
        };
        if !previous_uuids.insert(previous_uuid) {
            return Err(CoreError::invalid(
                "document contains a duplicate previous block UUID",
            ));
        }
        if current_uuids.contains(&previous_uuid) {
            continue;
        }
        let owner = transaction
            .query_row(
                "SELECT page_uuid FROM blocks WHERE uuid = ?1",
                [previous_uuid],
                |row| row.get::<_, uuid::Uuid>(0),
            )
            .optional()?;
        return match owner {
            Some(owner) if owner != page_uuid => Err(CoreError::invalid(
                "document previous block belongs to a different page",
            )),
            _ => Err(CoreError::invalid(
                "document previous block is not current on this page",
            )),
        };
    }
    Ok(())
}

fn plan_document_replace(
    page_uuid: uuid::Uuid,
    current: &PageDocumentSnapshot,
    units: Vec<DocumentUnitDraft>,
) -> DocumentReplacePlan {
    let current_by_uuid = current
        .blocks
        .iter()
        .map(|block| (block.uuid, block))
        .collect::<HashMap<_, _>>();
    let assigned_uuids = units
        .iter()
        .map(|unit| unit.previous_uuid.unwrap_or_else(uuid::Uuid::now_v7))
        .collect::<Vec<_>>();
    let mut desired = units
        .into_iter()
        .enumerate()
        .map(|(index, unit)| {
            let uuid = unit.previous_uuid.unwrap_or(assigned_uuids[index]);
            let parent_uuid = unit
                .parent_index
                .map(|parent_index| assigned_uuids[parent_index as usize]);
            DesiredUnit {
                uuid,
                parent_uuid,
                order_key: None,
                style: unit.style,
                markdown: unit.markdown,
                existing: current_by_uuid.contains_key(&uuid),
            }
        })
        .collect::<Vec<_>>();
    assign_desired_order_keys(&mut desired, current, &current_by_uuid);
    let desired_uuids = desired
        .iter()
        .filter(|unit| unit.existing)
        .map(|unit| unit.uuid)
        .collect::<HashSet<_>>();
    let now = chrono::Utc::now().timestamp();
    let mut kinds = Vec::new();
    let mut block_uuids = BTreeSet::new();
    let mut deleted_block_uuids = BTreeSet::new();
    let mut container_uuids = BTreeSet::new();
    let mut structure_changed = false;
    let mut graph_changed = false;

    // Parent indices are strictly earlier, so this emits all creates in the
    // required parent-before-child order before any retained block moves.
    for unit in desired.iter().filter(|unit| !unit.existing) {
        kinds.push(OpKind::BlockCreate(BlockCreate {
            uuid: unit.uuid,
            page_uuid,
            parent_uuid: unit.parent_uuid,
            order_key: unit
                .order_key
                .clone()
                .expect("every desired unit receives an order key"),
            style: unit.style,
            markdown: unit.markdown.clone(),
            created_at: now,
        }));
        block_uuids.insert(unit.uuid);
        container_uuids.insert(unit.parent_uuid.unwrap_or(page_uuid));
        structure_changed = true;
        graph_changed = true;
    }

    for unit in desired.iter().filter(|unit| unit.existing) {
        let current_block = current_by_uuid[&unit.uuid];
        if current_block.markdown != unit.markdown {
            graph_changed |= crate::operation::content_references_changed(
                &current_block.markdown,
                &unit.markdown,
            );
            kinds.push(OpKind::BlockSetMarkdown(BlockSetMarkdown {
                uuid: unit.uuid,
                markdown: unit.markdown.clone(),
            }));
            block_uuids.insert(unit.uuid);
            container_uuids.insert(current_block.parent_uuid.unwrap_or(current_block.page_uuid));
        }
        if current_block.style != unit.style {
            kinds.push(OpKind::BlockSetStyle(BlockSetStyle {
                uuid: unit.uuid,
                style: unit.style,
            }));
            block_uuids.insert(unit.uuid);
            container_uuids.insert(current_block.parent_uuid.unwrap_or(current_block.page_uuid));
        }
    }

    // Moves precede deletes so retained descendants can leave parents which
    // disappear from the desired document.
    for unit in desired.iter().filter(|unit| unit.existing) {
        let current_block = current_by_uuid[&unit.uuid];
        let order_key = unit
            .order_key
            .as_ref()
            .expect("every desired unit receives an order key");
        if current_block.parent_uuid != unit.parent_uuid || current_block.order_key != *order_key {
            kinds.push(OpKind::BlockMove(BlockMove {
                uuid: unit.uuid,
                page_uuid,
                parent_uuid: unit.parent_uuid,
                order_key: order_key.clone(),
            }));
            block_uuids.insert(unit.uuid);
            container_uuids.insert(current_block.parent_uuid.unwrap_or(current_block.page_uuid));
            container_uuids.insert(unit.parent_uuid.unwrap_or(page_uuid));
            structure_changed = true;
        }
    }

    // Current snapshot order is preorder; reversing it gives child-before-
    // parent deletion and therefore parent-before-child inverse creation.
    for block in current
        .blocks
        .iter()
        .rev()
        .filter(|block| !desired_uuids.contains(&block.uuid))
    {
        kinds.push(OpKind::BlockDelete(BlockDelete {
            uuid: block.uuid,
            page_uuid,
        }));
        block_uuids.insert(block.uuid);
        deleted_block_uuids.insert(block.uuid);
        container_uuids.insert(block.parent_uuid.unwrap_or(page_uuid));
        structure_changed = true;
        graph_changed = true;
    }

    DocumentReplacePlan {
        kinds,
        block_uuids,
        deleted_block_uuids,
        container_uuids,
        structure_changed,
        graph_changed,
    }
}

/// Preserve every existing key when the retained siblings keep their parent
/// and relative order. New or actually rearranged units consume deterministic
/// gaps between those stable anchors; only a group without sufficient key
/// space is rebalanced as a whole.
fn assign_desired_order_keys(
    desired: &mut [DesiredUnit],
    current: &PageDocumentSnapshot,
    current_by_uuid: &HashMap<uuid::Uuid, &Block>,
) {
    let mut groups = HashMap::<Option<uuid::Uuid>, Vec<usize>>::new();
    for (index, unit) in desired.iter().enumerate() {
        groups.entry(unit.parent_uuid).or_default().push(index);
    }

    for (parent_uuid, indices) in groups {
        let stable_candidates = indices
            .iter()
            .filter_map(|index| {
                let unit = &desired[*index];
                current_by_uuid
                    .get(&unit.uuid)
                    .filter(|block| block.parent_uuid == parent_uuid)
                    .map(|_| unit.uuid)
            })
            .collect::<HashSet<_>>();
        let desired_stable_order = indices
            .iter()
            .map(|index| desired[*index].uuid)
            .filter(|uuid| stable_candidates.contains(uuid))
            .collect::<Vec<_>>();
        let current_stable_order = current
            .blocks
            .iter()
            .filter(|block| {
                block.parent_uuid == parent_uuid && stable_candidates.contains(&block.uuid)
            })
            .map(|block| block.uuid)
            .collect::<Vec<_>>();

        if desired_stable_order == current_stable_order {
            for index in &indices {
                let unit = &mut desired[*index];
                if stable_candidates.contains(&unit.uuid) {
                    unit.order_key = Some(current_by_uuid[&unit.uuid].order_key.clone());
                }
            }
        }

        if !fill_order_key_gaps(desired, &indices) {
            for (ordinal, index) in indices.iter().enumerate() {
                desired[*index].order_key = Some(OrderKey::from_ordinal(ordinal + 1));
            }
        }
    }
}

fn fill_order_key_gaps(desired: &mut [DesiredUnit], indices: &[usize]) -> bool {
    let mut segment_start = 0;
    while segment_start < indices.len() {
        if desired[indices[segment_start]].order_key.is_some() {
            segment_start += 1;
            continue;
        }

        let segment_end = (segment_start + 1..indices.len())
            .find(|position| desired[indices[*position]].order_key.is_some())
            .unwrap_or(indices.len());
        let lower = segment_start.checked_sub(1).map(|position| {
            desired[indices[position]]
                .order_key
                .as_ref()
                .expect("preceding desired key is assigned")
                .value()
        });
        let upper = indices.get(segment_end).map(|index| {
            desired[*index]
                .order_key
                .as_ref()
                .expect("following desired key is assigned")
                .value()
        });
        let Some(keys) = order_keys_between(lower, upper, segment_end - segment_start) else {
            return false;
        };
        for (position, key) in (segment_start..segment_end).zip(keys) {
            desired[indices[position]].order_key = Some(key);
        }
        segment_start = segment_end;
    }
    true
}

fn order_keys_between(
    lower: Option<u64>,
    upper: Option<u64>,
    count: usize,
) -> Option<Vec<OrderKey>> {
    if count == 0 {
        return Some(Vec::new());
    }

    // Coordinates reserve zero and 2^64 + 1 as virtual bounds around the
    // complete u64 key space. This also permits a real key of zero.
    let lower = lower.map_or(0, |value| u128::from(value) + 1);
    let upper = upper.map_or(u128::from(u64::MAX) + 2, |value| u128::from(value) + 1);
    if upper <= lower || upper - lower - 1 < count as u128 {
        return None;
    }
    let span = upper - lower;
    let divisor = count as u128 + 1;
    let mut keys = Vec::with_capacity(count);
    for index in 1..=count as u128 {
        let coordinate = lower + span * index / divisor;
        let value = u64::try_from(coordinate - 1).ok()?;
        keys.push(
            format!("{value:016X}")
                .parse()
                .expect("formatted u64 is a valid order key"),
        );
    }
    Some(keys)
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
        let note = db::create_note(&connection, None)
            .await
            .expect("create note")
            .into_created()
            .expect("untitled note is created");
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

use super::*;
use crate::operation::{BlockCreate, PageCreate};
use crate::{JournalDate, journal_page_uuid};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// Bounded reverse-chronological Journal page size.
/// The upper bound keeps desktop and mobile callers from accidentally loading
/// an unbounded multi-year timeline in one RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(transparent)]
pub struct JournalListLimit(u32);

impl JournalListLimit {
    pub const DEFAULT: u32 = 30;
    pub const MAX: u32 = 200;

    pub fn new(value: u32) -> crate::CoreResult<Self> {
        if (1..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(crate::CoreError::invalid(format!(
                "journal list limit must be between 1 and {}",
                Self::MAX
            )))
        }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Default for JournalListLimit {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

impl TryFrom<u32> for JournalListLimit {
    type Error = crate::CoreError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for JournalListLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

pub async fn get_journal(conn: &Connection, date: JournalDate) -> Result<Option<Page>> {
    conn.call(move |database| get_journal_in_database(database, &date))
        .await
}

pub async fn ensure_journal(conn: &Connection, date: JournalDate) -> Result<Page> {
    Ok(ensure_journal_with_ops(conn, date).await?.value)
}

#[doc(hidden)]
pub async fn ensure_journal_with_ops(
    conn: &Connection,
    date: JournalDate,
) -> Result<AppliedMutation<Page>> {
    conn.call_domain(
        move |database| -> crate::CoreResult<AppliedMutation<Page>> {
            let transaction = database.transaction()?;
            if let Some(journal) = get_journal_in_database(&transaction, &date)? {
                transaction.commit()?;
                return Ok(AppliedMutation {
                    value: journal,
                    operations: Vec::new(),
                });
            }
            let workspace_uuid = transaction_workspace_uuid(&transaction)?;
            let page_uuid = journal_page_uuid(workspace_uuid, &date);
            let operations = vec![OpKind::PageCreate(PageCreate {
                uuid: page_uuid,
                kind: PageKind::Journal { date: date.clone() },
                title: None,
                layout: PageLayout::Outline,
                created_at: chrono::Utc::now().timestamp(),
            })];
            apply_local_action_in_transaction(&transaction, "ensure journal", operations.clone())?;
            let journal = get_journal_in_database(&transaction, &date)?
                .ok_or_else(|| crate::CoreError::not_found("ensured journal page disappeared"))?;
            transaction.commit()?;
            Ok(AppliedMutation {
                value: journal,
                operations,
            })
        },
    )
    .await
}

pub async fn list_journals(
    conn: &Connection,
    before_date: Option<JournalDate>,
    limit: JournalListLimit,
) -> Result<Vec<Page>> {
    conn.call(move |database| {
        let sql = format!(
            "SELECT {PAGE_COLUMNS} FROM pages
               JOIN page_identities AS journal_identity
                 ON journal_identity.page_uuid = pages.uuid
              WHERE journal_identity.page_kind = 'journal'
                AND (?1 IS NULL OR journal_identity.journal_date < ?1)
              ORDER BY journal_identity.journal_date DESC
              LIMIT ?2"
        );
        database
            .prepare(&sql)?
            .query_map(
                rusqlite::params![before_date, limit.get()],
                row_to_journal_page,
            )?
            .collect()
    })
    .await
}

pub async fn append_to_journal(
    conn: &Connection,
    date: JournalDate,
    content: BlockContent,
    style: BlockStyle,
) -> Result<Block> {
    Ok(append_to_journal_with_ops(conn, date, content, style)
        .await?
        .value)
}

#[doc(hidden)]
pub async fn append_to_journal_with_ops(
    conn: &Connection,
    date: JournalDate,
    content: BlockContent,
    style: BlockStyle,
) -> Result<AppliedMutation<Block>> {
    if content.markdown.trim().is_empty() {
        return Err(crate::CoreError::invalid("journal capture cannot be blank").into());
    }
    conn.call_domain(
        move |database| -> crate::CoreResult<AppliedMutation<Block>> {
            let transaction = database.transaction()?;
            let existing = get_journal_in_database(&transaction, &date)?;
            let workspace_uuid = transaction_workspace_uuid(&transaction)?;
            let page_uuid = journal_page_uuid(workspace_uuid, &date);
            let now = chrono::Utc::now().timestamp();
            let mut kinds = Vec::new();
            if existing.is_none() {
                kinds.push(OpKind::PageCreate(PageCreate {
                    uuid: page_uuid,
                    kind: PageKind::Journal { date: date.clone() },
                    title: None,
                    layout: PageLayout::Outline,
                    created_at: now,
                }));
            }

            let last_order_key = transaction
                .query_row(
                    "SELECT order_key FROM blocks
                  WHERE page_uuid = ?1 AND parent_uuid IS NULL
                  ORDER BY order_key DESC, uuid DESC LIMIT 1",
                    [page_uuid],
                    |row| row.get::<_, OrderKey>(0),
                )
                .optional()?;
            let order_key = super::blocks::next_append_order_key(last_order_key.as_ref())?;
            let block_uuid = uuid::Uuid::now_v7();
            kinds.push(OpKind::BlockCreate(BlockCreate {
                uuid: block_uuid,
                page_uuid,
                parent_uuid: None,
                order_key,
                style,
                markdown: content.markdown,
                created_at: now,
            }));

            apply_local_action_in_transaction(&transaction, "append to journal", kinds.clone())?;
            let block = get_block_in_database(&transaction, block_uuid)?
                .ok_or_else(|| crate::CoreError::not_found("appended journal block disappeared"))?;
            transaction.commit()?;
            Ok(AppliedMutation {
                value: block,
                operations: kinds,
            })
        },
    )
    .await
}

fn get_journal_in_database(
    database: &rusqlite::Connection,
    date: &JournalDate,
) -> rusqlite::Result<Option<Page>> {
    let sql = format!(
        "SELECT {PAGE_COLUMNS} FROM pages
           JOIN page_identities AS journal_identity
             ON journal_identity.page_uuid = pages.uuid
          WHERE journal_identity.page_kind = 'journal'
            AND journal_identity.journal_date = ?1"
    );
    database
        .query_row(&sql, [date], row_to_journal_page)
        .optional()
}

fn row_to_journal_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<Page> {
    let page = row_to_page(row)?;
    if !page.kind.is_journal() || page.title.is_some() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(page)
}

fn get_block_in_database(
    database: &rusqlite::Connection,
    uuid: uuid::Uuid,
) -> rusqlite::Result<Option<Block>> {
    let sql = format!("SELECT {BLOCK_COLUMNS} FROM blocks WHERE uuid = ?1");
    database.query_row(&sql, [uuid], row_to_block).optional()
}

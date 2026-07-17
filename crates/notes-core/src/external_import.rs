//! Atomic external-import boundary.
//!
//! Import parsers stay outside `notes-core`. They provide a fully materialized,
//! typed plan; this module validates the whole plan, emits ordinary synced
//! operations in one SQLite transaction, and records a durable local receipt.

use crate::model::{BlockStyle, JournalDate, OrderKey, PageAlias, PageKind, PageLayout};
use crate::operation::{
    self, BlockCreate, OpKind, PageAliasSet, PageCreate, validate_page_identity,
};
use crate::{Connection, CoreError, CoreResult};
use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalImportFormat {
    Logseq,
}

impl ExternalImportFormat {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Logseq => "logseq",
        }
    }
}

impl fmt::Display for ExternalImportFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ExternalImportFormat {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "logseq" => Ok(Self::Logseq),
            _ => Err(CoreError::invalid(format!(
                "unsupported external import format: {value}"
            ))),
        }
    }
}

/// Strong SHA-256 value shared by manifest and import-plan identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExternalImportDigest([u8; 32]);

impl ExternalImportDigest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn from_slice(bytes: &[u8]) -> CoreResult<Self> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CoreError::invalid("external import digest must contain 32 bytes"))?;
        Ok(Self(bytes))
    }

    fn parse_hex(value: &str) -> CoreResult<Self> {
        if value.len() != 64 {
            return Err(CoreError::invalid(
                "external import digest must contain 64 hexadecimal characters",
            ));
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
        }
        Ok(Self(bytes))
    }

    fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut value = String::with_capacity(64);
        for byte in self.0 {
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        value
    }
}

impl fmt::Display for ExternalImportDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for ExternalImportDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ExternalImportDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse_hex(&value).map_err(serde::de::Error::custom)
    }
}

fn hex_value(value: u8) -> CoreResult<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(CoreError::invalid(
            "external import digest contains a non-hexadecimal character",
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ExternalPageId(uuid::Uuid);

impl ExternalPageId {
    pub fn new(uuid: uuid::Uuid) -> CoreResult<Self> {
        if uuid.is_nil() {
            Err(CoreError::invalid("external page UUID cannot be nil"))
        } else {
            Ok(Self(uuid))
        }
    }

    pub const fn uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl<'de> Deserialize<'de> for ExternalPageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(uuid::Uuid::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ExternalBlockId(uuid::Uuid);

impl ExternalBlockId {
    pub fn new(uuid: uuid::Uuid) -> CoreResult<Self> {
        if uuid.is_nil() {
            Err(CoreError::invalid("external block UUID cannot be nil"))
        } else {
            Ok(Self(uuid))
        }
    }

    pub const fn uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl<'de> Deserialize<'de> for ExternalBlockId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(uuid::Uuid::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalImportIdentityContext {
    pub workspace_uuid: uuid::Uuid,
    pub import_namespace_uuid: uuid::Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalImportProvenance {
    pub format: ExternalImportFormat,
    pub manifest_digest: ExternalImportDigest,
    pub planner_version: u32,
    pub identity: ExternalImportIdentityContext,
    /// Complete parser-owned provenance. Core stores and compares this
    /// structured value but deliberately does not know Logseq-specific fields.
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExternalImportPageKind {
    Note { title: String },
    Journal { date: JournalDate },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalImportBlock {
    pub id: ExternalBlockId,
    pub parent_id: Option<ExternalBlockId>,
    pub order_key: OrderKey,
    pub style: BlockStyle,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalImportPage {
    pub id: ExternalPageId,
    pub kind: ExternalImportPageKind,
    pub layout: PageLayout,
    pub aliases: Vec<PageAlias>,
    pub blocks: Vec<ExternalImportBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalImportBatch {
    pub provenance: ExternalImportProvenance,
    pub pages: Vec<ExternalImportPage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalImportOutcome {
    Applied {
        receipt_uuid: uuid::Uuid,
        page_count: u64,
        block_count: u64,
        alias_count: u64,
        operation_count: u64,
        structure_reconciliations: u32,
    },
    ExactNoOp {
        receipt_uuid: uuid::Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalImportReceipt {
    pub receipt_uuid: uuid::Uuid,
    pub format: ExternalImportFormat,
    pub manifest_digest: ExternalImportDigest,
    pub plan_digest: ExternalImportDigest,
    pub planner_version: u32,
    pub identity: ExternalImportIdentityContext,
    pub provenance: serde_json::Value,
    pub imported_at: i64,
}

/// Returns the durable receipt for one external-import format, if that format
/// has been committed in this workspace.
///
/// The receipt is local provenance rather than synced note content. Callers
/// use its identity context to resume a deterministic dry run after restart;
/// this query never creates or mutates import state.
pub async fn external_import_receipt(
    conn: &Connection,
    format: ExternalImportFormat,
) -> Result<Option<ExternalImportReceipt>> {
    conn.call_domain(move |database| stored_receipt(database, format))
        .await
}

struct ValidatedBatch {
    batch: ExternalImportBatch,
    plan_digest: ExternalImportDigest,
    provenance_json: String,
    page_count: u64,
    block_count: u64,
    alias_count: u64,
}

impl ValidatedBatch {
    fn new(batch: ExternalImportBatch) -> CoreResult<Self> {
        validate_batch(&batch)?;
        let plan_json = serde_json::to_vec(&batch.pages)
            .map_err(|error| CoreError::invalid(format!("serializing import plan: {error}")))?;
        let plan_digest = ExternalImportDigest::from_bytes(Sha256::digest(plan_json).into());
        let provenance_json =
            serde_json::to_string(&batch.provenance.payload).map_err(|error| {
                CoreError::invalid(format!("serializing external import provenance: {error}"))
            })?;
        let page_count = batch.pages.len() as u64;
        let block_count = batch
            .pages
            .iter()
            .map(|page| page.blocks.len() as u64)
            .sum();
        let alias_count = batch
            .pages
            .iter()
            .map(|page| page.aliases.len() as u64)
            .sum();
        Ok(Self {
            batch,
            plan_digest,
            provenance_json,
            page_count,
            block_count,
            alias_count,
        })
    }

    fn kinds(&self, commit_timestamp: i64) -> Vec<OpKind> {
        let capacity = self
            .batch
            .pages
            .iter()
            .fold(self.batch.pages.len(), |total, page| {
                total
                    .saturating_add(page.aliases.len())
                    .saturating_add(page.blocks.len())
            });
        let mut kinds = Vec::with_capacity(capacity);
        for page in &self.batch.pages {
            let (kind, title) = match &page.kind {
                ExternalImportPageKind::Note { title } => (PageKind::Note, Some(title.clone())),
                ExternalImportPageKind::Journal { date } => {
                    (PageKind::Journal { date: date.clone() }, None)
                }
            };
            kinds.push(OpKind::PageCreate(PageCreate {
                uuid: page.id.uuid(),
                kind,
                title,
                layout: page.layout,
                created_at: commit_timestamp,
            }));
        }
        for page in &self.batch.pages {
            for alias in &page.aliases {
                kinds.push(OpKind::PageAliasSet(PageAliasSet {
                    uuid: page.id.uuid(),
                    alias: alias.clone(),
                    present: true,
                }));
            }
        }
        for page in &self.batch.pages {
            for block in &page.blocks {
                kinds.push(OpKind::BlockCreate(BlockCreate {
                    uuid: block.id.uuid(),
                    page_uuid: page.id.uuid(),
                    parent_uuid: block.parent_id.map(ExternalBlockId::uuid),
                    order_key: block.order_key.clone(),
                    style: block.style,
                    markdown: block.markdown.clone(),
                    created_at: commit_timestamp,
                }));
            }
        }
        kinds
    }
}

/// Applies a complete external import atomically.
///
/// A new import is accepted only into an empty workspace. Repeating the exact
/// same typed plan returns `ExactNoOp`; a changed plan or provenance is refused
/// instead of attempting an implicit merge.
pub async fn apply_external_import(
    conn: &Connection,
    batch: ExternalImportBatch,
) -> Result<ExternalImportOutcome> {
    let validated = ValidatedBatch::new(batch)?;
    conn.call_domain(move |database| -> CoreResult<ExternalImportOutcome> {
        let transaction = database.transaction()?;
        let workspace_uuid = crate::db::transaction_workspace_uuid(&transaction)?;
        let identity = validated.batch.provenance.identity;
        if identity.workspace_uuid != workspace_uuid {
            return Err(CoreError::conflict(format!(
                "external import belongs to workspace {}, but this replica is {workspace_uuid}",
                identity.workspace_uuid
            )));
        }
        for page in &validated.batch.pages {
            let kind = match &page.kind {
                ExternalImportPageKind::Note { .. } => PageKind::Note,
                ExternalImportPageKind::Journal { date } => {
                    PageKind::Journal { date: date.clone() }
                }
            };
            validate_page_identity(workspace_uuid, page.id.uuid(), &kind)?;
        }

        if let Some(receipt) = stored_receipt(
            &transaction,
            validated.batch.provenance.format,
        )? {
            let exact = receipt.manifest_digest
                == validated.batch.provenance.manifest_digest
                && receipt.plan_digest == validated.plan_digest
                && receipt.planner_version == validated.batch.provenance.planner_version
                && receipt.identity.workspace_uuid == identity.workspace_uuid
                && receipt.identity.import_namespace_uuid == identity.import_namespace_uuid
                && receipt.provenance == validated.batch.provenance.payload;
            if exact {
                transaction.commit()?;
                return Ok(ExternalImportOutcome::ExactNoOp {
                    receipt_uuid: receipt.receipt_uuid,
                });
            }
            return Err(CoreError::conflict(
                "this import format already has a receipt for a different manifest, plan, or identity context",
            ));
        }

        ensure_workspace_empty(&transaction)?;
        let commit_timestamp = chrono::Utc::now().timestamp();
        let kinds = validated.kinds(commit_timestamp);
        for kind in &kinds {
            operation::validate_kind(kind)?;
        }
        let applied = operation::apply_local_kinds_deferred_in_transaction(&transaction, kinds)?;
        let receipt_uuid = uuid::Uuid::now_v7();
        transaction.execute(
            "INSERT INTO external_import_receipts(
                receipt_uuid, import_format, manifest_digest, plan_digest,
                planner_version, identity_workspace_uuid, import_namespace_uuid,
                provenance_json, imported_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                receipt_uuid,
                validated.batch.provenance.format.as_str(),
                validated.batch.provenance.manifest_digest.as_bytes().as_slice(),
                validated.plan_digest.as_bytes().as_slice(),
                validated.batch.provenance.planner_version,
                identity.workspace_uuid,
                identity.import_namespace_uuid,
                validated.provenance_json,
                commit_timestamp,
            ],
        )?;
        let outcome = ExternalImportOutcome::Applied {
            receipt_uuid,
            page_count: validated.page_count,
            block_count: validated.block_count,
            alias_count: validated.alias_count,
            operation_count: applied.operations.len() as u64,
            structure_reconciliations: applied.stats.structure_reconciliations,
        };
        transaction.commit()?;
        Ok(outcome)
    })
    .await
}

fn stored_receipt(
    database: &rusqlite::Connection,
    format: ExternalImportFormat,
) -> CoreResult<Option<ExternalImportReceipt>> {
    let row = database
        .query_row(
            "SELECT receipt_uuid, manifest_digest, plan_digest, planner_version,
                    identity_workspace_uuid, import_namespace_uuid, provenance_json, imported_at
               FROM external_import_receipts WHERE import_format = ?1",
            [format.as_str()],
            |row| {
                Ok((
                    row.get::<_, uuid::Uuid>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, uuid::Uuid>(4)?,
                    row.get::<_, uuid::Uuid>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            receipt_uuid,
            manifest_digest,
            plan_digest,
            planner_version,
            identity_workspace_uuid,
            import_namespace_uuid,
            provenance_json,
            imported_at,
        )| {
            if receipt_uuid.is_nil() {
                return Err(CoreError::invalid(
                    "stored external import receipt UUID cannot be nil",
                ));
            }
            if planner_version == 0 {
                return Err(CoreError::invalid(
                    "stored external import planner version must be greater than zero",
                ));
            }
            if identity_workspace_uuid.is_nil() || import_namespace_uuid.is_nil() {
                return Err(CoreError::invalid(
                    "stored external import identity UUIDs cannot be nil",
                ));
            }
            let provenance: serde_json::Value =
                serde_json::from_str(&provenance_json).map_err(|error| {
                    CoreError::invalid(format!("stored import provenance is invalid JSON: {error}"))
                })?;
            if !provenance.is_object() {
                return Err(CoreError::invalid(
                    "stored external import provenance must be a JSON object",
                ));
            }
            Ok(ExternalImportReceipt {
                receipt_uuid,
                format,
                manifest_digest: ExternalImportDigest::from_slice(&manifest_digest)?,
                plan_digest: ExternalImportDigest::from_slice(&plan_digest)?,
                planner_version,
                identity: ExternalImportIdentityContext {
                    workspace_uuid: identity_workspace_uuid,
                    import_namespace_uuid,
                },
                provenance,
                imported_at,
            })
        },
    )
    .transpose()
}

fn ensure_workspace_empty(transaction: &rusqlite::Transaction<'_>) -> CoreResult<()> {
    let has_state: bool = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM page_identities
            UNION ALL SELECT 1 FROM pages
            UNION ALL SELECT 1 FROM blocks
            UNION ALL SELECT 1 FROM page_alias_lww
            UNION ALL SELECT 1 FROM tombstones
            UNION ALL SELECT 1 FROM applied_ops
            UNION ALL SELECT 1 FROM sync_outbox
            UNION ALL SELECT 1 FROM history_undo
            UNION ALL SELECT 1 FROM history_redo
            UNION ALL SELECT 1 FROM external_import_receipts
         )",
        [],
        |row| row.get(0),
    )?;
    if has_state {
        Err(CoreError::conflict(
            "external import requires an empty workspace; implicit merge is not supported",
        ))
    } else {
        Ok(())
    }
}

fn validate_batch(batch: &ExternalImportBatch) -> CoreResult<()> {
    if batch.pages.is_empty() {
        return Err(CoreError::invalid(
            "external import must contain at least one page",
        ));
    }
    if batch.provenance.planner_version == 0 {
        return Err(CoreError::invalid(
            "external import planner version must be greater than zero",
        ));
    }
    if batch.provenance.identity.workspace_uuid.is_nil()
        || batch.provenance.identity.import_namespace_uuid.is_nil()
    {
        return Err(CoreError::invalid(
            "external import identity UUIDs cannot be nil",
        ));
    }
    if !batch.provenance.payload.is_object() {
        return Err(CoreError::invalid(
            "external import provenance payload must be a JSON object",
        ));
    }

    let mut page_ids = HashSet::new();
    let mut block_pages = HashMap::new();
    let mut journal_dates = HashSet::new();
    let mut alias_owners = BTreeMap::<String, ExternalPageId>::new();
    for page in &batch.pages {
        if !page_ids.insert(page.id) {
            return Err(CoreError::invalid(format!(
                "external import contains duplicate page UUID {}",
                page.id.uuid()
            )));
        }
        match &page.kind {
            ExternalImportPageKind::Note { title } => {
                if title.trim().is_empty() {
                    return Err(CoreError::invalid("external note title cannot be blank"));
                }
                insert_alias_owner(
                    &mut alias_owners,
                    crate::model::normalize_title(title),
                    page.id,
                )?;
            }
            ExternalImportPageKind::Journal { date } => {
                if !journal_dates.insert(date) {
                    return Err(CoreError::invalid(format!(
                        "external import contains duplicate journal date {date}"
                    )));
                }
            }
        }
        let mut page_aliases = BTreeSet::new();
        for alias in &page.aliases {
            if !page_aliases.insert(alias.as_str()) {
                return Err(CoreError::invalid(format!(
                    "page {} contains duplicate alias {}",
                    page.id.uuid(),
                    alias
                )));
            }
            insert_alias_owner(&mut alias_owners, alias.as_str().to_owned(), page.id)?;
        }
        for block in &page.blocks {
            if page_ids.contains(&ExternalPageId(block.id.uuid())) {
                return Err(CoreError::invalid(format!(
                    "UUID {} is used by both a page and a block",
                    block.id.uuid()
                )));
            }
            if block_pages.insert(block.id, page.id).is_some() {
                return Err(CoreError::invalid(format!(
                    "external import contains duplicate block UUID {}",
                    block.id.uuid()
                )));
            }
        }
    }
    for page_id in &page_ids {
        if block_pages.contains_key(&ExternalBlockId(page_id.uuid())) {
            return Err(CoreError::invalid(format!(
                "UUID {} is used by both a page and a block",
                page_id.uuid()
            )));
        }
    }

    for page in &batch.pages {
        let page_blocks = page
            .blocks
            .iter()
            .map(|block| (block.id, block))
            .collect::<HashMap<_, _>>();
        let mut sibling_orders = HashSet::new();
        for block in &page.blocks {
            if let Some(parent) = block.parent_id {
                if parent == block.id {
                    return Err(CoreError::invalid("a block cannot be its own parent"));
                }
                if block_pages.get(&parent) != Some(&page.id) {
                    return Err(CoreError::invalid(format!(
                        "block {} has a missing or cross-page parent {}",
                        block.id.uuid(),
                        parent.uuid()
                    )));
                }
            }
            if !sibling_orders.insert((block.parent_id, block.order_key.clone())) {
                return Err(CoreError::invalid(format!(
                    "page {} contains duplicate sibling order key {}",
                    page.id.uuid(),
                    block.order_key
                )));
            }
        }
        let mut proven_acyclic = HashSet::new();
        for start in page_blocks.keys() {
            if proven_acyclic.contains(start) {
                continue;
            }
            let mut path = Vec::new();
            let mut visiting = HashSet::new();
            let mut cursor = Some(*start);
            while let Some(block_id) = cursor {
                if proven_acyclic.contains(&block_id) {
                    break;
                }
                if !visiting.insert(block_id) {
                    return Err(CoreError::invalid(format!(
                        "external import contains a block-parent cycle at {}",
                        block_id.uuid()
                    )));
                }
                path.push(block_id);
                cursor = page_blocks.get(&block_id).and_then(|block| block.parent_id);
            }
            proven_acyclic.extend(path);
        }
    }
    Ok(())
}

fn insert_alias_owner(
    owners: &mut BTreeMap<String, ExternalPageId>,
    alias: String,
    page_id: ExternalPageId,
) -> CoreResult<()> {
    if let Some(previous) = owners.insert(alias.clone(), page_id)
        && previous != page_id
    {
        return Err(CoreError::invalid(format!(
            "external import alias {alias:?} refers to multiple pages"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn digest(byte: u8) -> ExternalImportDigest {
        ExternalImportDigest::from_bytes([byte; 32])
    }

    async fn database() -> (tempfile::NamedTempFile, Connection) {
        let file = tempfile::NamedTempFile::new().expect("temporary database");
        let connection = db::open(file.path()).await.expect("open database");
        (file, connection)
    }

    async fn batch(connection: &Connection, block_count: usize) -> ExternalImportBatch {
        let workspace_uuid = db::workspace_uuid(connection)
            .await
            .expect("workspace UUID");
        let page_id = ExternalPageId::new(uuid::Uuid::now_v7()).unwrap();
        let blocks = (0..block_count)
            .map(|index| ExternalImportBlock {
                id: ExternalBlockId::new(uuid::Uuid::now_v7()).unwrap(),
                parent_id: None,
                order_key: OrderKey::from_ordinal(index + 1),
                style: BlockStyle::Bullet,
                markdown: format!("block {index}"),
            })
            .collect();
        ExternalImportBatch {
            provenance: ExternalImportProvenance {
                format: ExternalImportFormat::Logseq,
                manifest_digest: digest(7),
                planner_version: 1,
                identity: ExternalImportIdentityContext {
                    workspace_uuid,
                    import_namespace_uuid: uuid::Uuid::now_v7(),
                },
                payload: serde_json::json!({"manifest": {"version": 1}}),
            },
            pages: vec![ExternalImportPage {
                id: page_id,
                kind: ExternalImportPageKind::Note {
                    title: "Imported".into(),
                },
                layout: PageLayout::Outline,
                aliases: vec![PageAlias::new("Imported.md").unwrap()],
                blocks,
            }],
        }
    }

    #[tokio::test]
    async fn receipt_query_returns_none_before_an_import() {
        let (_file, connection) = database().await;

        assert_eq!(
            external_import_receipt(&connection, ExternalImportFormat::Logseq)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn receipt_query_returns_the_committed_typed_identity_and_digests() {
        let (_file, connection) = database().await;
        let input = batch(&connection, 2).await;
        let expected_plan_digest = ValidatedBatch::new(input.clone()).unwrap().plan_digest;
        let expected_provenance = input.provenance.clone();
        let outcome = apply_external_import(&connection, input).await.unwrap();
        let receipt_uuid = match outcome {
            ExternalImportOutcome::Applied { receipt_uuid, .. } => receipt_uuid,
            ExternalImportOutcome::ExactNoOp { .. } => panic!("first import must be applied"),
        };

        let receipt = external_import_receipt(&connection, ExternalImportFormat::Logseq)
            .await
            .unwrap()
            .expect("committed receipt");
        assert_eq!(receipt.receipt_uuid, receipt_uuid);
        assert_eq!(receipt.format, ExternalImportFormat::Logseq);
        assert_eq!(receipt.manifest_digest, expected_provenance.manifest_digest);
        assert_eq!(receipt.plan_digest, expected_plan_digest);
        assert_eq!(receipt.planner_version, expected_provenance.planner_version);
        assert_eq!(receipt.identity, expected_provenance.identity);
        assert_eq!(receipt.provenance, expected_provenance.payload);
        assert!(receipt.imported_at > 0);
    }

    #[tokio::test]
    async fn receipt_query_rejects_corrupt_digest_and_identity_storage() {
        let (_digest_file, digest_connection) = database().await;
        let digest_input = batch(&digest_connection, 1).await;
        apply_external_import(&digest_connection, digest_input)
            .await
            .unwrap();
        digest_connection
            .call(|database| {
                database.execute_batch("PRAGMA ignore_check_constraints = ON;")?;
                database.execute(
                    "UPDATE external_import_receipts SET manifest_digest = X'00'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let digest_error =
            external_import_receipt(&digest_connection, ExternalImportFormat::Logseq)
                .await
                .expect_err("invalid digest must be rejected");
        assert!(matches!(
            digest_error.downcast_ref::<CoreError>(),
            Some(CoreError::InvalidInput(_))
        ));
        assert!(digest_error.to_string().contains("32 bytes"));

        let (_identity_file, identity_connection) = database().await;
        let identity_input = batch(&identity_connection, 1).await;
        apply_external_import(&identity_connection, identity_input)
            .await
            .unwrap();
        identity_connection
            .call(|database| {
                database.execute_batch("PRAGMA ignore_check_constraints = ON;")?;
                database.execute(
                    "UPDATE external_import_receipts SET import_namespace_uuid = zeroblob(16)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let identity_error =
            external_import_receipt(&identity_connection, ExternalImportFormat::Logseq)
                .await
                .expect_err("nil identity UUID must be rejected");
        assert!(matches!(
            identity_error.downcast_ref::<CoreError>(),
            Some(CoreError::InvalidInput(_))
        ));
        assert!(identity_error.to_string().contains("cannot be nil"));
    }

    #[tokio::test]
    async fn invalid_late_parent_rolls_back_without_any_domain_mutation() {
        let (_file, connection) = database().await;
        let mut input = batch(&connection, 2).await;
        input.pages[0].blocks[1].parent_id =
            Some(ExternalBlockId::new(uuid::Uuid::now_v7()).unwrap());
        let error = apply_external_import(&connection, input)
            .await
            .expect_err("invalid late block");
        assert!(error.to_string().contains("missing or cross-page parent"));

        let counts = connection
            .call(|database| {
                database.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM pages),
                       (SELECT COUNT(*) FROM blocks),
                       (SELECT COUNT(*) FROM applied_ops),
                       (SELECT COUNT(*) FROM sync_outbox),
                       (SELECT COUNT(*) FROM external_import_receipts),
                       (SELECT COUNT(*) FROM local_device)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
            })
            .await
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0, 0, 0));
    }

    #[tokio::test]
    async fn late_database_failure_rolls_back_operations_outbox_and_receipt() {
        let (_file, connection) = database().await;
        operation::configure_sync(&connection, "https://sync.example")
            .await
            .unwrap();
        connection
            .call(|database| {
                database.execute_batch(
                    "CREATE TRIGGER fail_external_import_receipt
                       BEFORE INSERT ON external_import_receipts
                       BEGIN SELECT RAISE(ABORT, 'receipt failpoint'); END;",
                )
            })
            .await
            .unwrap();
        let input = batch(&connection, 3).await;
        let error = apply_external_import(&connection, input)
            .await
            .expect_err("late receipt failure");
        assert!(error.to_string().contains("receipt failpoint"));
        let counts = connection
            .call(|database| {
                database.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM page_identities),
                       (SELECT COUNT(*) FROM pages),
                       (SELECT COUNT(*) FROM blocks),
                       (SELECT COUNT(*) FROM page_alias_lww),
                       (SELECT COUNT(*) FROM applied_ops),
                       (SELECT COUNT(*) FROM sync_outbox),
                       (SELECT COUNT(*) FROM external_import_receipts),
                       (SELECT COUNT(*) FROM local_device)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                        ))
                    },
                )
            })
            .await
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0, 0, 0, 0, 0));
    }

    #[tokio::test]
    async fn emits_normal_ops_outbox_and_no_interactive_history() {
        let (_file, connection) = database().await;
        operation::configure_sync(&connection, "https://sync.example")
            .await
            .unwrap();
        let input = batch(&connection, 2).await;
        let expected_ops = 1 + 1 + 2;
        let outcome = apply_external_import(&connection, input).await.unwrap();
        assert!(matches!(
            outcome,
            ExternalImportOutcome::Applied {
                operation_count,
                structure_reconciliations: 1,
                ..
            } if operation_count == expected_ops
        ));
        let ops = operation::pending_outbox(&connection, 100).await.unwrap();
        assert_eq!(ops.len() as u64, expected_ops);
        assert!(
            ops.iter()
                .all(|op| op.format_version == operation::FORMAT_VERSION)
        );
        assert!(ops.iter().all(|op| op.op_id.get_version_num() == 7));
        assert!(ops.windows(2).all(|pair| pair[0].hlc < pair[1].hlc));
        let counts = connection
            .call(|database| {
                database.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM applied_ops),
                       (SELECT COUNT(*) FROM history_undo),
                       (SELECT COUNT(*) FROM history_redo)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
            })
            .await
            .unwrap();
        assert_eq!(counts, (expected_ops as i64, 0, 0));
        let timestamps = connection
            .call(|database| {
                database.query_row(
                    "SELECT
                       (SELECT created_at FROM pages LIMIT 1),
                       (SELECT MIN(created_at) FROM blocks),
                       (SELECT MAX(created_at) FROM blocks),
                       (SELECT imported_at FROM external_import_receipts LIMIT 1)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
            })
            .await
            .unwrap();
        assert_eq!(timestamps.0, timestamps.1);
        assert_eq!(timestamps.0, timestamps.2);
        assert_eq!(timestamps.0, timestamps.3);
    }

    #[tokio::test]
    async fn caller_supplied_creation_timestamps_are_rejected_by_serde() {
        let (_file, connection) = database().await;
        let input = batch(&connection, 1).await;
        let mut page_timestamp = serde_json::to_value(&input).unwrap();
        page_timestamp["pages"][0]["createdAt"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ExternalImportBatch>(page_timestamp).is_err());

        let mut block_timestamp = serde_json::to_value(&input).unwrap();
        block_timestamp["pages"][0]["blocks"][0]["createdAt"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ExternalImportBatch>(block_timestamp).is_err());
    }

    #[tokio::test]
    async fn large_tree_runs_one_final_structure_reconciliation() {
        let (_file, connection) = database().await;
        let mut input = batch(&connection, 2_000).await;
        for index in 1..input.pages[0].blocks.len() {
            input.pages[0].blocks[index].parent_id = Some(input.pages[0].blocks[index - 1].id);
            input.pages[0].blocks[index].order_key = OrderKey::first();
        }
        let outcome = apply_external_import(&connection, input).await.unwrap();
        assert!(matches!(
            outcome,
            ExternalImportOutcome::Applied {
                block_count: 2_000,
                structure_reconciliations: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn emitted_operations_converge_on_a_second_replica() {
        let (_first_file, first) = database().await;
        operation::configure_sync(&first, "https://sync.example")
            .await
            .unwrap();
        let input = batch(&first, 20).await;
        let workspace_uuid = input.provenance.identity.workspace_uuid;
        apply_external_import(&first, input.clone()).await.unwrap();
        let operations = operation::pending_outbox(&first, 1_000).await.unwrap();

        let (_second_file, second) = database().await;
        second
            .call(move |database| {
                database.execute(
                    "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                    [workspace_uuid],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        operation::apply_batch(&second, &operations, operation::Origin::Remote)
            .await
            .unwrap();
        let first_snapshot = operation::export_sync_snapshot(&first, 0).await.unwrap();
        let second_snapshot = operation::export_sync_snapshot(&second, 0).await.unwrap();
        assert_eq!(first_snapshot, second_snapshot);

        let (_snapshot_file, snapshot_replica) = database().await;
        snapshot_replica
            .call(move |database| {
                database.execute(
                    "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                    [workspace_uuid],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        operation::import_sync_snapshot(&snapshot_replica, first_snapshot)
            .await
            .unwrap();
        assert_eq!(
            db::get_page_by_title(&snapshot_replica, "Imported.md".into())
                .await
                .unwrap()
                .unwrap()
                .uuid,
            input.pages[0].id.uuid()
        );

        let first_outcome = apply_external_import(&first, input).await.unwrap();
        assert!(matches!(
            first_outcome,
            ExternalImportOutcome::ExactNoOp { .. }
        ));
        assert_eq!(
            operation::pending_outbox(&first, 1_000)
                .await
                .unwrap()
                .len(),
            operations.len()
        );
    }

    #[tokio::test]
    async fn archive_roundtrip_preserves_exact_rerun_receipt() {
        let (_source_file, source) = database().await;
        let input = batch(&source, 3).await;
        apply_external_import(&source, input.clone()).await.unwrap();
        let source_receipt = external_import_receipt(&source, ExternalImportFormat::Logseq)
            .await
            .unwrap()
            .expect("source receipt");
        let archive = db::export_archive(&source).await.unwrap();
        assert_eq!(archive.external_import_receipts.len(), 1);

        let (_destination_file, destination) = database().await;
        let workspace_uuid = input.provenance.identity.workspace_uuid;
        destination
            .call(move |database| {
                database.execute(
                    "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                    [workspace_uuid],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let mut stale_input = batch(&destination, 1).await;
        stale_input.provenance.manifest_digest = digest(9);
        apply_external_import(&destination, stale_input)
            .await
            .unwrap();
        db::import_archive(&destination, archive).await.unwrap();
        assert_eq!(
            external_import_receipt(&destination, ExternalImportFormat::Logseq)
                .await
                .unwrap(),
            Some(source_receipt)
        );
        assert_eq!(
            db::get_page_by_title(&destination, "Imported.md".into())
                .await
                .unwrap()
                .expect("archived alias resolves")
                .uuid,
            input.pages[0].id.uuid()
        );
        assert!(matches!(
            apply_external_import(&destination, input).await.unwrap(),
            ExternalImportOutcome::ExactNoOp { .. }
        ));
    }
}

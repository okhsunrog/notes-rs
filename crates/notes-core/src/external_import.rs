//! Atomic external-import boundary.
//!
//! Import parsers stay outside `notes-core`. They provide a fully materialized,
//! typed plan; this module validates the whole plan, emits ordinary synced
//! operations in one SQLite transaction, and records a durable local receipt.

use crate::model::{
    AttachmentOwner, BlockStyle, JournalDate, OrderKey, PageAlias, PageKind, PageLayout,
};
use crate::operation::{
    self, AttachmentAdd, BlockCreate, OpKind, PageAliasSet, PageCreate, attachment_uuid,
    validate_attachment_filename, validate_page_identity,
};
use crate::{BlobHash, Connection, CoreError, CoreResult};
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

/// A typed reference to an owner created by the same import batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ExternalImportAttachmentOwner {
    Page(ExternalPageId),
    Block(ExternalBlockId),
}

impl ExternalImportAttachmentOwner {
    const fn domain_owner(self) -> AttachmentOwner {
        match self {
            Self::Page(id) => AttachmentOwner::Page(id.uuid()),
            Self::Block(id) => AttachmentOwner::Block(id.uuid()),
        }
    }
}

/// Attachment metadata materialized by an external importer. Blob bytes are
/// installed separately before this atomic metadata transaction is attempted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalImportAttachment {
    pub attachment_uuid: uuid::Uuid,
    pub owner: ExternalImportAttachmentOwner,
    pub blob_hash: BlobHash,
    pub filename: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalImportBatch {
    pub provenance: ExternalImportProvenance,
    pub pages: Vec<ExternalImportPage>,
    pub attachments: Vec<ExternalImportAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalImportOutcome {
    Applied {
        receipt_uuid: uuid::Uuid,
        page_count: u64,
        block_count: u64,
        alias_count: u64,
        attachment_count: u64,
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

/// Reports whether a first external import may use the empty-workspace path.
///
/// Hosts use this read-only check for preview UX. The apply transaction repeats
/// the same predicate, so this result is never treated as authorization to
/// commit after concurrent local activity.
pub async fn external_import_destination_is_empty(conn: &Connection) -> Result<bool> {
    conn.call_domain(|database| -> CoreResult<bool> { Ok(!workspace_has_import_state(database)?) })
        .await
}

#[derive(Debug)]
struct ValidatedBatch {
    batch: ExternalImportBatch,
    plan_digest: ExternalImportDigest,
    provenance_json: String,
    page_count: u64,
    block_count: u64,
    alias_count: u64,
    attachment_count: u64,
}

#[derive(Serialize)]
struct MaterializedPlan<'a> {
    pages: &'a [ExternalImportPage],
    attachments: &'a [ExternalImportAttachment],
}

impl ValidatedBatch {
    fn new(batch: ExternalImportBatch) -> CoreResult<Self> {
        validate_batch(&batch)?;
        let plan_json = serde_json::to_vec(&MaterializedPlan {
            pages: &batch.pages,
            attachments: &batch.attachments,
        })
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
        let attachment_count = batch.attachments.len() as u64;
        Ok(Self {
            batch,
            plan_digest,
            provenance_json,
            page_count,
            block_count,
            alias_count,
            attachment_count,
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
            })
            .saturating_add(self.batch.attachments.len());
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
        for attachment in &self.batch.attachments {
            kinds.push(OpKind::AttachmentAdd(AttachmentAdd {
                owner: attachment.owner.domain_owner(),
                blob_hash: attachment.blob_hash,
                filename: attachment.filename.clone(),
                mime: attachment.mime.clone(),
                size: attachment.size,
            }));
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
            attachment_count: validated.attachment_count,
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
    if workspace_has_import_state(transaction)? {
        Err(CoreError::conflict(
            "external import requires an empty workspace; implicit merge is not supported",
        ))
    } else {
        Ok(())
    }
}

fn workspace_has_import_state(database: &rusqlite::Connection) -> CoreResult<bool> {
    database
        .query_row(
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
        )
        .map_err(CoreError::from)
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

    let mut attachment_keys = HashSet::new();
    let mut attachment_uuids = HashSet::new();
    for attachment in &batch.attachments {
        let owner = attachment.owner.domain_owner();
        let owner_exists = match attachment.owner {
            ExternalImportAttachmentOwner::Page(id) => page_ids.contains(&id),
            ExternalImportAttachmentOwner::Block(id) => block_pages.contains_key(&id),
        };
        if !owner_exists {
            return Err(CoreError::invalid(format!(
                "external attachment owner {} does not exist in the import plan",
                owner.uuid()
            )));
        }
        if !attachment_keys.insert((owner, attachment.blob_hash)) {
            return Err(CoreError::invalid(format!(
                "external import contains duplicate attachment for owner {} and hash {}",
                owner.uuid(),
                attachment.blob_hash
            )));
        }
        if attachment.attachment_uuid.is_nil()
            || !attachment_uuids.insert(attachment.attachment_uuid)
        {
            return Err(CoreError::invalid(
                "external import contains a nil or duplicate attachment UUID",
            ));
        }
        if attachment.attachment_uuid != attachment_uuid(owner, &attachment.blob_hash) {
            return Err(CoreError::invalid(format!(
                "external attachment UUID {} does not match its owner and hash",
                attachment.attachment_uuid
            )));
        }
        validate_attachment_filename(&attachment.filename)?;
        if attachment.mime.trim().is_empty() {
            return Err(CoreError::invalid("attachment MIME type is required"));
        }
        if attachment.size > i64::MAX as u64 {
            return Err(CoreError::invalid("attachment size is too large"));
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
        let blob_hash = BlobHash::from_bytes([0xab; 32]);
        let owner = ExternalImportAttachmentOwner::Page(page_id);
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
            attachments: vec![ExternalImportAttachment {
                attachment_uuid: attachment_uuid(owner.domain_owner(), &blob_hash),
                owner,
                blob_hash,
                filename: "asset.png".into(),
                mime: "image/png".into(),
                size: 128,
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
    async fn destination_readiness_uses_the_apply_empty_workspace_predicate() {
        let (_file, connection) = database().await;
        assert!(
            external_import_destination_is_empty(&connection)
                .await
                .unwrap()
        );

        db::create_note(&connection, None).await.unwrap();
        assert!(
            !external_import_destination_is_empty(&connection)
                .await
                .unwrap()
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
    async fn late_attachment_failure_rolls_back_the_whole_import() {
        let (_file, connection) = database().await;
        operation::configure_sync(&connection, "https://sync.example")
            .await
            .unwrap();
        connection
            .call(|database| {
                database.execute_batch(
                    "CREATE TRIGGER fail_external_import_attachment
                       BEFORE INSERT ON attachments
                       BEGIN SELECT RAISE(ABORT, 'attachment failpoint'); END;",
                )
            })
            .await
            .unwrap();

        let input = batch(&connection, 2).await;
        let error = apply_external_import(&connection, input)
            .await
            .expect_err("late attachment failure");
        assert!(error.to_string().contains("attachment failpoint"));
        let counts = connection
            .call(|database| {
                database.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM page_identities),
                       (SELECT COUNT(*) FROM pages),
                       (SELECT COUNT(*) FROM blocks),
                       (SELECT COUNT(*) FROM page_alias_lww),
                       (SELECT COUNT(*) FROM attachment_lww),
                       (SELECT COUNT(*) FROM attachments),
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
                            row.get::<_, i64>(8)?,
                            row.get::<_, i64>(9)?,
                        ))
                    },
                )
            })
            .await
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0, 0, 0, 0, 0, 0, 0));
    }

    #[tokio::test]
    async fn attachment_metadata_is_part_of_the_exact_rerun_digest() {
        let (_file, connection) = database().await;
        let input = batch(&connection, 1).await;
        let original_digest = ValidatedBatch::new(input.clone()).unwrap().plan_digest;
        apply_external_import(&connection, input.clone())
            .await
            .unwrap();
        assert!(matches!(
            apply_external_import(&connection, input.clone())
                .await
                .unwrap(),
            ExternalImportOutcome::ExactNoOp { .. }
        ));

        let mut changed = input;
        changed.attachments[0].filename = "renamed.png".into();
        assert_ne!(
            ValidatedBatch::new(changed.clone()).unwrap().plan_digest,
            original_digest
        );
        let error = apply_external_import(&connection, changed)
            .await
            .expect_err("changed attachment metadata cannot be an exact rerun");
        assert!(error.to_string().contains("different manifest, plan"));
    }

    #[tokio::test]
    async fn attachment_hash_json_is_strictly_canonical() {
        let (_file, connection) = database().await;
        let input = batch(&connection, 1).await;
        let mut invalid = serde_json::to_value(&input).unwrap();
        invalid["attachments"][0]["blobHash"] = serde_json::json!("bad");
        assert!(serde_json::from_value::<ExternalImportBatch>(invalid).is_err());

        let mut uppercase = serde_json::to_value(&input).unwrap();
        uppercase["attachments"][0]["blobHash"] =
            serde_json::json!(input.attachments[0].blob_hash.to_string().to_uppercase());
        assert!(serde_json::from_value::<ExternalImportBatch>(uppercase).is_err());
    }

    #[tokio::test]
    async fn attachment_validation_rejects_missing_owners_and_duplicate_identities() {
        let (_file, connection) = database().await;
        let input = batch(&connection, 1).await;

        let mut missing_owner = input.clone();
        missing_owner.attachments[0].owner = ExternalImportAttachmentOwner::Block(
            ExternalBlockId::new(uuid::Uuid::now_v7()).unwrap(),
        );
        assert!(
            ValidatedBatch::new(missing_owner)
                .expect_err("missing owner")
                .to_string()
                .contains("does not exist")
        );

        let mut duplicate_key = input.clone();
        duplicate_key
            .attachments
            .push(duplicate_key.attachments[0].clone());
        assert!(
            ValidatedBatch::new(duplicate_key)
                .expect_err("duplicate owner and hash")
                .to_string()
                .contains("duplicate attachment")
        );

        let mut mismatched_uuid = input;
        mismatched_uuid.attachments[0].attachment_uuid = uuid::Uuid::now_v7();
        assert!(
            ValidatedBatch::new(mismatched_uuid)
                .expect_err("mismatched attachment UUID")
                .to_string()
                .contains("does not match")
        );

        let input = batch(&connection, 1).await;
        let mut duplicate_uuid = input.clone();
        let mut second = duplicate_uuid.attachments[0].clone();
        second.blob_hash = BlobHash::from_bytes([0x43; 32]);
        duplicate_uuid.attachments.push(second);
        assert!(
            ValidatedBatch::new(duplicate_uuid)
                .expect_err("duplicate attachment UUID")
                .to_string()
                .contains("duplicate attachment UUID")
        );

        let mut blank_filename = input.clone();
        blank_filename.attachments[0].filename = " ".into();
        assert!(ValidatedBatch::new(blank_filename).is_err());
        let mut blank_mime = input.clone();
        blank_mime.attachments[0].mime = " ".into();
        assert!(ValidatedBatch::new(blank_mime).is_err());
        let mut oversized = input;
        oversized.attachments[0].size = u64::MAX;
        assert!(ValidatedBatch::new(oversized).is_err());
    }

    #[tokio::test]
    async fn emits_normal_ops_outbox_and_no_interactive_history() {
        let (_file, connection) = database().await;
        operation::configure_sync(&connection, "https://sync.example")
            .await
            .unwrap();
        let input = batch(&connection, 2).await;
        let expected_ops = 1 + 1 + 2 + 1;
        let outcome = apply_external_import(&connection, input).await.unwrap();
        assert!(matches!(
            outcome,
            ExternalImportOutcome::Applied {
                attachment_count: 1,
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
        assert!(matches!(
            ops.last().map(|operation| &operation.kind),
            Some(OpKind::AttachmentAdd(_))
        ));
        assert_eq!(
            db::list_attachments(
                &connection,
                AttachmentOwner::Page(db::list_pages(&connection, 10).await.unwrap()[0].uuid)
            )
            .await
            .unwrap()
            .len(),
            1
        );
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
        assert_eq!(first_snapshot.attachments.len(), 1);
        assert_eq!(
            first_snapshot.attachments[0].blob_hash,
            input.attachments[0].blob_hash
        );

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
        assert_eq!(archive.attachments.len(), 1);
        assert_eq!(
            archive.attachments[0].blob_hash,
            input.attachments[0].blob_hash
        );

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
        assert_eq!(
            db::list_attachments(
                &destination,
                AttachmentOwner::Page(db::list_pages(&destination, 10).await.unwrap()[0].uuid)
            )
            .await
            .unwrap()
            .len(),
            1
        );
    }
}

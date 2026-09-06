use super::*;
use rusqlite::params;
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InkVersionInfo {
    pub version_uuid: uuid::Uuid,
    pub parents: Vec<uuid::Uuid>,
    pub root_hash: String,
    pub device_name: String,
    pub device_id: uuid::Uuid,
    #[specta(type = specta_typescript::Number)]
    pub modified_at_ms: f64,
    pub available: bool,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InkNoteStatus {
    pub page_uuid: uuid::Uuid,
    pub revision: Option<String>,
    pub unpublished_changes: bool,
    pub publication_requested: bool,
    pub base_version: Option<uuid::Uuid>,
    pub heads: Vec<InkVersionInfo>,
}
use serde::Serialize;
impl Store {
    /// Read the initial history and pin the editor's base in one transaction.
    pub fn open_editor(&self, editing: bool) -> CommandResult<InkHistorySnapshot> {
        let mut conn = storage::open(self)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        storage::ensure_document(&tx, self.document)?;
        let (history, _) = storage::history::result(&tx, self.document)?;
        if editing {
            tx.execute(
                "UPDATE ink_documents SET editing=1 WHERE page_uuid=?1",
                [self.document],
            )?;
        }
        tx.commit()?;
        Ok(history)
    }
    pub fn close_editor(&self) -> CommandResult<()> {
        let mut conn = storage::open(self)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE ink_documents SET editing=0 WHERE page_uuid=?1",
            [self.document],
        )?;
        versions::adopt_single_head(&tx, self.document)?;
        tx.commit()?;
        Ok(())
    }
    pub fn status(&self) -> CommandResult<InkNoteStatus> {
        let mut conn = storage::open(self)?;
        let tx = conn.transaction()?;
        let (revision,unpublished_changes,publication_requested,base_version)=tx.query_row("SELECT revision,dirty,publication_requested,base_version FROM ink_documents WHERE page_uuid=?1",[self.document],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let heads = versions::head_ids(&tx, self.document)?
            .into_iter()
            .map(|id| {
                let v = versions::get(&tx, id)?
                    .ok_or_else(|| CommandError::not_found("Missing ink version"))?;
                let available = tx.query_row(
                    "SELECT root_id IS NOT NULL FROM ink_versions WHERE version_uuid=?1",
                    [id],
                    |r| r.get(0),
                )?;
                Ok(InkVersionInfo {
                    version_uuid: id,
                    parents: v.publication.parents,
                    root_hash: v.publication.root_hash.to_string(),
                    device_name: v.publication.device_name,
                    device_id: v.device_id,
                    modified_at_ms: v.modified_hlc.wall_ms() as f64,
                    available,
                })
            })
            .collect::<CommandResult<_>>()?;
        tx.commit()?;
        Ok(InkNoteStatus {
            page_uuid: self.document,
            revision,
            unpublished_changes,
            publication_requested,
            base_version,
            heads,
        })
    }
    pub fn preview(&self, version: uuid::Uuid) -> CommandResult<InkDraft> {
        let mut conn = storage::open(self)?;
        let tx = conn.transaction()?;
        let root: Option<Vec<u8>> = tx.query_row(
            "SELECT root_id FROM ink_versions WHERE version_uuid=?1 AND page_uuid=?2",
            params![version, self.document],
            |r| r.get(0),
        )?;
        let root =
            root.ok_or_else(|| CommandError::not_found("Download handwriting version first"))?;
        let snapshot = storage::load_root(
            &tx,
            storage::get_record(
                &tx,
                root.try_into()
                    .map_err(|_| CommandError::invalid("Invalid root ID"))?,
                None,
            )?,
        )?;
        let page = storage::to_draft(&snapshot)?;
        tx.commit()?;
        Ok(page)
    }
    pub fn request_publication(&self) -> CommandResult<()> {
        let conn = storage::open(self)?;
        conn.execute(
            "UPDATE ink_documents SET publication_requested=1 WHERE page_uuid=?1 AND dirty=1 AND publication_requested=0",
            [self.document],
        )?;
        Ok(())
    }
    pub fn publication_requested(&self) -> CommandResult<bool> {
        let conn = storage::open(self)?;
        Ok(conn.query_row(
            "SELECT publication_requested FROM ink_documents WHERE page_uuid=?1",
            [self.document],
            |r| r.get(0),
        )?)
    }
    pub fn page_uuid(&self) -> uuid::Uuid {
        self.document
    }
}
/// On process startup, interrupted sessions become candidates for completion.
/// No geometry is imported from the former prototype database.
pub async fn recoverable_notes(conn: &crate::Connection) -> anyhow::Result<Vec<uuid::Uuid>> {
    conn.call(|db| {
        db.prepare(
            "SELECT d.page_uuid FROM ink_documents d JOIN pages p ON p.uuid=d.page_uuid \
             WHERE dirty=1 OR publication_requested=1 OR editing=1",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
    })
    .await
}

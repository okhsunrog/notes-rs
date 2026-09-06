//! Causal publication metadata. Concurrent heads are retained until explicit resolution.
use super::*;
use crate::{Hlc, OpKind, PageKind, operation};
use notes_blob::BlobHash;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Publish {
    pub version_uuid: Uuid,
    pub page_uuid: Uuid,
    pub parents: Vec<Uuid>,
    pub root_hash: BlobHash,
    pub device_name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    #[serde(flatten)]
    pub publication: Publish,
    pub device_id: Uuid,
    pub modified_hlc: Hlc,
}
impl Publish {
    pub(crate) fn validate(&self) -> CommandResult<()> {
        let parents: std::collections::BTreeSet<_> = self.parents.iter().collect();
        if self.page_uuid.is_nil()
            || self.version_uuid.is_nil()
            || self.parents.len() > 128
            || parents.len() != self.parents.len()
            || self
                .parents
                .iter()
                .any(|p| p.is_nil() || *p == self.version_uuid)
            || self.device_name.trim().is_empty()
            || self.device_name.len() > 256
        {
            return Err(CommandError::invalid("Invalid handwriting publication"));
        }
        Ok(())
    }
}
pub(crate) fn get(conn: &Connection, id: Uuid) -> CommandResult<Option<Version>> {
    let row = conn.query_row("SELECT page_uuid,root_hash,device_name,device_id,modified_hlc FROM ink_versions WHERE version_uuid=?1",[id], |r|Ok((r.get::<_,Uuid>(0)?,crate::db::row_blob_hash(r,1)?,r.get::<_,String>(2)?,r.get::<_,Uuid>(3)?,r.get::<_,String>(4)?))).optional()?;
    let Some((page_uuid, root_hash, device_name, device_id, hlc)) = row else {
        return Ok(None);
    };
    let parents=conn.prepare("SELECT parent_uuid FROM ink_version_parents WHERE version_uuid=?1 ORDER BY parent_uuid")?.query_map([id],|r|r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(Version {
        publication: Publish {
            version_uuid: id,
            page_uuid,
            parents,
            root_hash,
            device_name,
        },
        device_id,
        modified_hlc: hlc.parse().map_err(err)?,
    }))
}
pub(crate) fn all(conn: &Connection) -> CommandResult<Vec<Version>> {
    let ids = conn
        .prepare("SELECT version_uuid FROM ink_versions ORDER BY version_uuid")?
        .query_map([], |r| r.get::<_, Uuid>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| {
            get(conn, id)?.ok_or_else(|| CommandError::not_found("Missing handwriting version"))
        })
        .collect()
}
pub(crate) fn apply(conn: &rusqlite::Transaction<'_>, version: &Version) -> CommandResult<()> {
    let p = &version.publication;
    p.validate()?;
    operation::ensure_page_identity(conn, p.page_uuid, &PageKind::Handwriting)?;
    if let Some(existing) = get(conn, p.version_uuid)? {
        let mut normalized = version.clone();
        normalized.publication.parents.sort();
        if existing != normalized {
            return Err(CommandError::conflict("Handwriting version ID reused"));
        }
        return Ok(());
    }
    // Parents may arrive out of order, but can never cross notes or form a cycle.
    for parent in &p.parents {
        let wrong: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM ink_versions WHERE version_uuid=?1 AND page_uuid!=?2)",
            params![parent, p.page_uuid],
            |r| r.get(0),
        )?;
        if wrong {
            return Err(CommandError::invalid(
                "Handwriting parent belongs to another note",
            ));
        }
    }
    let wrong_child: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM ink_version_parents p JOIN ink_versions v ON v.version_uuid=p.version_uuid WHERE p.parent_uuid=?1 AND v.page_uuid!=?2)", params![p.version_uuid,p.page_uuid], |r|r.get(0))?;
    if wrong_child {
        return Err(CommandError::invalid(
            "Handwriting child belongs to another note",
        ));
    }
    conn.execute("INSERT INTO ink_versions(version_uuid,page_uuid,root_hash,device_name,device_id,modified_hlc,root_id) VALUES(?1,?2,?3,?4,?5,?6,(SELECT root_id FROM ink_staged_roots WHERE root_hash=?3 UNION SELECT root_id FROM ink_versions WHERE root_hash=?3 AND root_id IS NOT NULL LIMIT 1))",
        params![p.version_uuid,p.page_uuid,p.root_hash.as_bytes().as_slice(),p.device_name,version.device_id,version.modified_hlc.to_string()])?;
    for parent in &p.parents {
        conn.execute(
            "INSERT INTO ink_version_parents VALUES(?1,?2)",
            params![p.version_uuid, parent],
        )?;
    }
    let cyclic: bool = conn.query_row("WITH RECURSIVE ancestors(id) AS (SELECT parent_uuid FROM ink_version_parents WHERE version_uuid=?1 UNION SELECT p.parent_uuid FROM ink_version_parents p JOIN ancestors a ON p.version_uuid=a.id) SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=?1)", [p.version_uuid], |r|r.get(0))?;
    if cyclic {
        return Err(CommandError::invalid("Cyclic handwriting history"));
    }
    conn.execute(
        "DELETE FROM ink_staged_roots WHERE root_hash=?1",
        [p.root_hash.as_bytes().as_slice()],
    )?;
    conn.execute(
        "UPDATE pages SET updated_at=max(updated_at,?2) WHERE uuid=?1",
        params![p.page_uuid, (version.modified_hlc.wall_ms() / 1000) as i64],
    )?;
    adopt_single_head(conn, p.page_uuid)?;
    Ok(())
}
pub(crate) fn head_ids(conn: &Connection, page: Uuid) -> CommandResult<Vec<Uuid>> {
    Ok(conn.prepare("SELECT v.version_uuid FROM ink_versions v WHERE v.page_uuid=?1 AND NOT EXISTS(SELECT 1 FROM ink_version_parents p WHERE p.parent_uuid=v.version_uuid) ORDER BY v.version_uuid")?
        .query_map([page], |r|r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub(crate) fn adopt_single_head(conn: &Connection, page: Uuid) -> CommandResult<()> {
    let heads = head_ids(conn, page)?;
    if heads.len() != 1 {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO ink_documents(page_uuid) VALUES(?1)",
        [page],
    )?;
    let (dirty, base): (bool, Option<Uuid>) = conn.query_row(
        "SELECT dirty,base_version FROM ink_documents WHERE page_uuid=?1",
        [page],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if dirty || base == Some(heads[0]) {
        return Ok(());
    }
    let root: Option<Vec<u8>> = conn.query_row(
        "SELECT root_id FROM ink_versions WHERE version_uuid=?1",
        [heads[0]],
        |r| r.get(0),
    )?;
    if let Some(root) = root {
        conn.execute("DELETE FROM ink_history WHERE page_uuid=?1", [page])?;
        conn.execute(
            "INSERT INTO ink_history VALUES(?1,0,?2)",
            params![page, root],
        )?;
        conn.execute("UPDATE ink_documents SET root_id=?2,revision=?3,cursor=0,base_version=?4 WHERE page_uuid=?1",params![page,root,Uuid::now_v7().to_string(),heads[0]])?;
    }
    Ok(())
}

impl Store {
    /// Publish after the host has drained its write queue and completed compaction.
    /// The chosen root and outbox envelope commit together, even without a network.
    pub fn publish(&self, device_name: String) -> CommandResult<Option<Publish>> {
        let mut conn = storage::open(self)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        storage::ensure_document(&tx, self.document)?;
        let (dirty, base): (bool, Option<Uuid>) = tx.query_row(
            "SELECT dirty,base_version FROM ink_documents WHERE page_uuid=?1",
            [self.document],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if !dirty {
            return Ok(None);
        }
        let Some((snapshot, _)) = storage::load(&tx, self.document)? else {
            return Ok(None);
        };
        let publication = Publish {
            version_uuid: Uuid::now_v7(),
            page_uuid: self.document,
            parents: base.into_iter().collect(),
            root_hash: BlobHash::digest(&snapshot.root.encode().map_err(err)?),
            device_name,
        };
        operation::apply_local_kinds_in_transaction(
            &tx,
            vec![OpKind::InkPublish(publication.clone())],
        )?;
        tx.execute(
            "UPDATE ink_versions SET root_id=?2 WHERE version_uuid=?1",
            params![publication.version_uuid, snapshot.root.id.as_slice()],
        )?;
        tx.execute(
            "UPDATE ink_documents SET dirty=0,base_version=?2 WHERE page_uuid=?1",
            params![self.document, publication.version_uuid],
        )?;
        for id in snapshot.chunks.keys() {
            tx.execute(
                "INSERT OR IGNORE INTO ink_sealed_chunks VALUES(?1)",
                [id.as_slice()],
            )?;
        }
        tx.commit()?;
        Ok(Some(publication))
    }
    pub fn versions(&self) -> CommandResult<Vec<Version>> {
        let conn = storage::open(self)?;
        let heads = head_ids(&conn, self.document)?;
        heads
            .into_iter()
            .map(|id| {
                get(&conn, id)?.ok_or_else(|| CommandError::not_found("Missing handwriting head"))
            })
            .collect()
    }
}

impl Store {
    /// The first selected version stays in this note; further selections become
    /// independent notes. An empty selection or stale conflict dialog is rejected.
    pub fn resolve(
        &self,
        expected_heads: Vec<Uuid>,
        keep: Vec<Uuid>,
        device_name: String,
    ) -> CommandResult<Vec<Uuid>> {
        let mut conn = storage::open(self)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        storage::ensure_document(&tx, self.document)?;
        let mut expected = expected_heads;
        expected.sort();
        let heads = head_ids(&tx, self.document)?;
        let dirty: bool = tx.query_row(
            "SELECT dirty FROM ink_documents WHERE page_uuid=?1",
            [self.document],
            |r| r.get(0),
        )?;
        if heads != expected || heads.len() < 2 || dirty {
            return Err(CommandError::conflict(
                "Save local edits and reload handwriting conflict before resolving",
            ));
        }
        let unique: std::collections::BTreeSet<_> = keep.iter().collect();
        if keep.is_empty()
            || unique.len() != keep.len()
            || keep.iter().any(|id| !heads.contains(id))
        {
            return Err(CommandError::invalid(
                "Invalid handwriting conflict selection",
            ));
        }
        let title: Option<String> = tx.query_row(
            "SELECT title FROM pages WHERE uuid=?1",
            [self.document],
            |r| r.get(0),
        )?;
        let mut notes = Vec::new();
        for (index, id) in keep.into_iter().enumerate() {
            let selected = get(&tx, id)?
                .ok_or_else(|| CommandError::not_found("Missing handwriting selection"))?;
            let root: Option<Vec<u8>> = tx.query_row(
                "SELECT root_id FROM ink_versions WHERE version_uuid=?1",
                [id],
                |r| r.get(0),
            )?;
            if root.is_none() {
                return Err(CommandError::not_found(
                    "Download selected handwriting version first",
                ));
            }
            let page_uuid = if index == 0 {
                self.document
            } else {
                Uuid::now_v7()
            };
            let mut ops = Vec::new();
            if index != 0 {
                ops.push(OpKind::PageCreate(crate::PageCreate {
                    uuid: page_uuid,
                    kind: PageKind::Handwriting,
                    title: Some(format!(
                        "{} — {}",
                        title.as_deref().unwrap_or("Handwriting"),
                        selected.publication.device_name
                    )),
                    layout: crate::PageLayout::Outline,
                    created_at: chrono::Utc::now().timestamp(),
                }));
            }
            ops.push(OpKind::InkPublish(Publish {
                version_uuid: Uuid::now_v7(),
                page_uuid,
                parents: if index == 0 { heads.clone() } else { vec![] },
                root_hash: selected.publication.root_hash,
                device_name: device_name.clone(),
            }));
            operation::apply_local_kinds_in_transaction(&tx, ops)?;
            notes.push(page_uuid);
        }
        tx.commit()?;
        Ok(notes)
    }
}

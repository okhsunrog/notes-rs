//! Binary graph transfer independent of SQLite layout and HTTP transport.
use super::*;
use ink_format::{
    Id,
    model::{Body, Document},
    record::Record,
    snapshot::Snapshot,
};
use notes_blob::BlobHash;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;

pub const MAX_GRAPH_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ROOT_BYTES: u64 = 16 * 1024 * 1024;
pub type Blobs = BTreeMap<BlobHash, Vec<u8>>;

/// Parse a root without decoding point columns. The closure excludes the root itself.
pub fn required_blobs(root_hash: BlobHash, root: &[u8]) -> CommandResult<BTreeMap<BlobHash, u64>> {
    if root.len() as u64 > MAX_ROOT_BYTES || BlobHash::digest(root) != root_hash {
        return Err(CommandError::invalid(
            "Invalid handwriting root hash or size",
        ));
    }
    let record = Record::decode(root).map_err(err)?;
    record.check_features(&[1, 2, 3, 4]).map_err(err)?;
    let doc = Document::read(&record).map_err(err)?;
    if !doc.resources.is_empty() || doc.records.len() > 450_016 || doc.chunks.len() > 150_000 {
        return Err(CommandError::invalid(
            "Unsupported or oversized handwriting graph",
        ));
    }
    let mut blobs = BTreeMap::new();
    let mut total = root.len() as u64;
    for b in doc.records.values().chain(doc.chunks.values()) {
        let hash = BlobHash::from_bytes(b.hash);
        total = total
            .checked_add(b.length)
            .ok_or_else(|| CommandError::invalid("Handwriting size overflow"))?;
        if total > MAX_GRAPH_BYTES
            || b.length > MAX_ROOT_BYTES
            || blobs.insert(hash, b.length).is_some_and(|n| n != b.length)
        {
            return Err(CommandError::invalid("Invalid handwriting blob catalog"));
        }
    }
    Ok(blobs)
}
fn snapshot(root_hash: BlobHash, blobs: &Blobs) -> CommandResult<Snapshot> {
    let root = blobs
        .get(&root_hash)
        .ok_or_else(|| CommandError::invalid("Missing handwriting root"))?;
    let required = required_blobs(root_hash, root)?;
    for (hash, len) in required {
        let bytes = blobs
            .get(&hash)
            .ok_or_else(|| CommandError::invalid("Incomplete handwriting graph"))?;
        if bytes.len() as u64 != len || BlobHash::digest(bytes) != hash {
            return Err(CommandError::invalid("Corrupt handwriting blob"));
        }
    }
    let root = Record::decode(root).map_err(err)?;
    let doc = Document::read(&root).map_err(err)?;
    let records = doc
        .records
        .into_iter()
        .map(|(id, b)| {
            Ok((
                id,
                Record::decode(&blobs[&BlobHash::from_bytes(b.hash)]).map_err(err)?,
            ))
        })
        .collect::<CommandResult<_>>()?;
    let chunks = doc
        .chunks
        .into_iter()
        .map(|(id, b)| (id, blobs[&BlobHash::from_bytes(b.hash)].clone()))
        .collect();
    let snapshot = Snapshot {
        root,
        records,
        chunks,
        resources: BTreeMap::new(),
    };
    let decoded = snapshot.chunks.values().try_fold(0u64, |total, bytes| {
        let size = ink_format::chunk::Chunk::decoded_value_bytes(bytes).map_err(err)?;
        total
            .checked_add(size as u64)
            .ok_or_else(|| CommandError::invalid("Decoded handwriting size overflow"))
    })?;
    if decoded > 128 * 1024 * 1024 {
        return Err(CommandError::invalid(
            "Decoded handwriting graph is too large",
        ));
    }
    snapshot.validate(&[1, 2, 3, 4]).map_err(err)?;
    storage::validate_adapter(&snapshot)?;
    Ok(snapshot)
}
/// Complete validation before server publication or local installation.
pub fn validate_graph(root_hash: BlobHash, blobs: &Blobs) -> CommandResult<()> {
    snapshot(root_hash, blobs).map(|_| ())
}
fn stage(conn: &Connection, root_hash: BlobHash, snapshot: &Snapshot) -> CommandResult<()> {
    storage::history::persist(conn, &snapshot)?;
    conn.execute(
        "INSERT OR IGNORE INTO ink_staged_roots VALUES(?1,?2)",
        params![root_hash.as_bytes().as_slice(), snapshot.root.id.as_slice()],
    )?;
    // A snapshot may already carry the publication while its blobs download later.
    conn.execute(
        "UPDATE ink_versions SET root_id=?2 WHERE root_hash=?1",
        params![root_hash.as_bytes().as_slice(), snapshot.root.id.as_slice()],
    )?;
    let pages = conn
        .prepare("SELECT DISTINCT page_uuid FROM ink_versions WHERE root_hash=?1")?
        .query_map([root_hash.as_bytes().as_slice()], |r| {
            r.get::<_, uuid::Uuid>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !pages.is_empty() {
        conn.execute(
            "DELETE FROM ink_staged_roots WHERE root_hash=?1",
            [root_hash.as_bytes().as_slice()],
        )?;
        for page in pages {
            versions::adopt_single_head(conn, page)?;
        }
    }
    // Downloaded/published chunks are immutable across future local compaction.
    for id in snapshot.chunks.keys() {
        conn.execute(
            "INSERT OR IGNORE INTO ink_sealed_chunks VALUES(?1)",
            [id.as_slice()],
        )?;
    }
    Ok(())
}
pub async fn stage_graph(
    conn: &crate::Connection,
    root_hash: BlobHash,
    blobs: Blobs,
) -> anyhow::Result<()> {
    let snapshot = tokio::task::spawn_blocking(move || snapshot(root_hash, &blobs)).await??;
    conn.call_domain(move |database| -> CommandResult<_> {
        let tx = database.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        stage(&tx, root_hash, &snapshot)?;
        tx.commit()?;
        Ok(())
    })
    .await
}
pub(crate) fn export_root(conn: &Connection, root: Id) -> CommandResult<Blobs> {
    let bytes: Vec<u8> = conn.query_row(
        "SELECT data FROM ink_records WHERE id=?1",
        [root.as_slice()],
        |r| r.get(0),
    )?;
    let snapshot = storage::load_root(conn, Record::decode(&bytes).map_err(err)?)?;
    let mut blobs = Blobs::new();
    blobs.insert(BlobHash::digest(&bytes), bytes);
    for record in snapshot.records.values() {
        let bytes = record.encode().map_err(err)?;
        blobs.insert(BlobHash::digest(&bytes), bytes);
    }
    for bytes in snapshot.chunks.into_values() {
        blobs.insert(BlobHash::digest(&bytes), bytes);
    }
    Ok(blobs)
}
pub async fn export_graph(conn: &crate::Connection, root_hash: BlobHash) -> anyhow::Result<Blobs> {
    conn.call_domain(move |database| -> CommandResult<_> {
        let tx=database.transaction()?;
        let root:Option<Vec<u8>>=tx.query_row("SELECT root_id FROM ink_versions WHERE root_hash=?1 AND root_id IS NOT NULL LIMIT 1",[root_hash.as_bytes().as_slice()],|r|r.get(0)).optional()?;
        let root=root.ok_or_else(||CommandError::not_found("Handwriting version is not available locally"))?;
        let blobs=export_root(&tx,root.try_into().map_err(|_|CommandError::invalid("Invalid root ID"))?)?;
        if !blobs.contains_key(&root_hash) { return Err(CommandError::invalid("Published root hash mismatch")); }
        tx.commit()?;
        Ok(blobs)
    }).await
}

/// Reuse blocks already present in any local note or retained history.
pub async fn cached_blobs(
    conn: &crate::Connection,
    hashes: Vec<BlobHash>,
) -> anyhow::Result<Blobs> {
    conn.call_domain(move |database| -> CommandResult<_> {
        let tx=database.transaction()?;
        let mut result=Blobs::new();
        {
            let mut query=tx.prepare("SELECT data FROM ink_records WHERE hash=?1 UNION ALL SELECT data FROM ink_chunks WHERE hash=?1 LIMIT 1")?;
            for hash in hashes {
                if let Some(bytes)=query.query_row([hash.as_bytes().as_slice()],|r|r.get::<_,Vec<u8>>(0)).optional()? {
                    if BlobHash::digest(&bytes)!=hash { return Err(CommandError::invalid("Corrupt local handwriting block")); }
                    result.insert(hash,bytes);
                }
            }
        }
        tx.commit()?;
        Ok(result)
    }).await
}

/// A root is marked available only after full validation and atomic installation.
pub async fn has_graph(conn: &crate::Connection, root_hash: BlobHash) -> anyhow::Result<bool> {
    conn.call(move |database| database.query_row("SELECT EXISTS(SELECT 1 FROM ink_versions WHERE root_hash=?1 AND root_id IS NOT NULL UNION ALL SELECT 1 FROM ink_staged_roots WHERE root_hash=?1)",[root_hash.as_bytes().as_slice()],|r|r.get(0))).await
}

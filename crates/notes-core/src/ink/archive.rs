//! Ink entries in the existing workspace archive, not a single-note file format.
use super::*;
use crate::{OpKind, PageKind, operation};
use ink_format::{Id, snapshot::Snapshot};
use notes_blob::BlobHash;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use transfer::Blobs;
/// This in-memory workspace-archive adapter is bounded separately from streamed attachments.
pub const MAX_ARCHIVE_INK_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub documents: Vec<Document>,
    pub blobs: BTreeMap<BlobHash, u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub page_uuid: uuid::Uuid,
    /// First variant is the working copy on restore. Other variants remain conflicts.
    pub variants: Vec<Variant>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    pub root_hash: BlobHash,
    pub device_name: String,
}

fn insert_snapshot(blobs: &mut Blobs, snapshot: Snapshot) -> CommandResult<BlobHash> {
    let root = snapshot.root.encode().map_err(err)?;
    let hash = BlobHash::digest(&root);
    blobs.insert(hash, root);
    for record in snapshot.records.values() {
        let bytes = record.encode().map_err(err)?;
        blobs.insert(BlobHash::digest(&bytes), bytes);
    }
    for bytes in snapshot.chunks.into_values() {
        blobs.insert(BlobHash::digest(&bytes), bytes);
    }
    Ok(hash)
}
pub(crate) fn capture(conn: &Connection) -> CommandResult<(Manifest, Blobs)> {
    let pages = conn.prepare("SELECT p.uuid FROM pages p JOIN page_identities i ON i.page_uuid=p.uuid WHERE i.content_type='ink' ORDER BY p.uuid")?
        .query_map([], |r| r.get::<_, uuid::Uuid>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut manifest = Manifest::default();
    let mut blobs = Blobs::new();
    for page_uuid in pages {
        let state: Option<(Option<Vec<u8>>, bool, Option<uuid::Uuid>)> = conn
            .query_row(
                "SELECT root_id,dirty,base_version FROM ink_documents WHERE page_uuid=?1",
                [page_uuid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let heads = versions::head_ids(conn, page_uuid)?;
        let mut variants = Vec::new();
        let mut seen = BTreeSet::new();
        // Include unpublished work even when other devices have published concurrent heads.
        if let Some((Some(root), dirty, _)) = state.as_ref() {
            if *dirty || heads.is_empty() {
                let id: Id = root
                    .clone()
                    .try_into()
                    .map_err(|_| CommandError::invalid("Invalid ink root ID"))?;
                let graph = transfer::export_root(conn, id)?;
                let bytes =
                    conn.query_row("SELECT data FROM ink_records WHERE id=?1", [root], |r| {
                        r.get::<_, Vec<u8>>(0)
                    })?;
                let hash = BlobHash::digest(&bytes);
                blobs.extend(graph);
                seen.insert(hash);
                variants.push(Variant {
                    root_hash: hash,
                    device_name: "Local working copy".into(),
                });
            }
        }
        for head in heads {
            if state
                .as_ref()
                .is_some_and(|(_, dirty, base)| *dirty && *base == Some(head))
            {
                continue;
            }
            let version = versions::get(conn, head)?
                .ok_or_else(|| CommandError::not_found("Missing ink version"))?;
            let hash = version.publication.root_hash;
            if !seen.insert(hash) {
                continue;
            }
            let root: Option<Vec<u8>> = conn.query_row(
                "SELECT root_id FROM ink_versions WHERE version_uuid=?1",
                [head],
                |r| r.get(0),
            )?;
            let root = root.ok_or_else(|| {
                CommandError::not_found(
                    "Download all handwriting conflict versions before making a backup",
                )
            })?;
            blobs.extend(transfer::export_root(
                conn,
                root.try_into()
                    .map_err(|_| CommandError::invalid("Invalid ink root ID"))?,
            )?);
            variants.push(Variant {
                root_hash: hash,
                device_name: version.publication.device_name,
            });
        }
        if variants.is_empty() {
            let hash = insert_snapshot(
                &mut blobs,
                storage::build(
                    None,
                    InkDraftPatch {
                        order: vec![],
                        upserts: vec![],
                        background: InkBackground::Plain,
                    },
                )?,
            )?;
            variants.push(Variant {
                root_hash: hash,
                device_name: "Empty note".into(),
            });
        }
        if let Some((Some(root), false, _)) = state {
            let bytes: Vec<u8> =
                conn.query_row("SELECT data FROM ink_records WHERE id=?1", [root], |r| {
                    r.get(0)
                })?;
            let hash = BlobHash::digest(&bytes);
            if let Some(index) = variants.iter().position(|v| v.root_hash == hash) {
                variants.swap(0, index);
            }
        }
        manifest.documents.push(Document {
            page_uuid,
            variants,
        });
        if blobs.values().map(|b| b.len() as u64).sum::<u64>() > MAX_ARCHIVE_INK_BYTES {
            return Err(CommandError::invalid(
                "Handwriting archive exceeds the in-memory archive limit",
            ));
        }
    }
    manifest.blobs = blobs.iter().map(|(h, b)| (*h, b.len() as u64)).collect();
    Ok((manifest, blobs))
}

/// Validate/decode before taking the write lock. No partially staged graphs survive failure.
pub(crate) fn prepare(
    archive: &crate::db::DataArchive,
) -> CommandResult<(Manifest, BTreeMap<BlobHash, Snapshot>)> {
    let expected: BTreeSet<_> = archive
        .pages
        .iter()
        .filter(|p| p.kind == PageKind::Handwriting)
        .map(|p| p.uuid)
        .collect();
    let mut pages = BTreeSet::new();
    let mut graphs = BTreeMap::new();
    let mut required = BTreeMap::new();
    let mut graph_bytes = 0u64;
    if archive
        .ink_blobs
        .values()
        .map(|b| b.len() as u64)
        .sum::<u64>()
        > MAX_ARCHIVE_INK_BYTES
    {
        return Err(CommandError::invalid(
            "Handwriting archive exceeds the in-memory archive limit",
        ));
    }
    for document in &archive.ink.documents {
        if !pages.insert(document.page_uuid)
            || document.variants.is_empty()
            || document.variants.len() > 128
        {
            return Err(CommandError::invalid("Invalid archive ink document"));
        }
        let mut roots = BTreeSet::new();
        for variant in &document.variants {
            if !roots.insert(variant.root_hash)
                || variant.device_name.is_empty()
                || variant.device_name.len() > 256
            {
                return Err(CommandError::invalid("Invalid archive ink variant"));
            }
            if graphs.contains_key(&variant.root_hash) {
                continue;
            }
            let bytes = archive
                .ink_blobs
                .get(&variant.root_hash)
                .ok_or_else(|| CommandError::invalid("Missing binary handwriting archive data"))?;
            let closure = transfer::required_blobs(variant.root_hash, bytes)?;
            graph_bytes += bytes.len() as u64 + closure.values().sum::<u64>();
            if graph_bytes > MAX_ARCHIVE_INK_BYTES * 2 {
                return Err(CommandError::invalid(
                    "Too many overlapping handwriting archive variants",
                ));
            }
            required.insert(variant.root_hash, bytes.len() as u64);
            required.extend(closure);
            graphs.insert(
                variant.root_hash,
                transfer::snapshot(variant.root_hash, &archive.ink_blobs)?,
            );
        }
    }
    if pages != expected
        || required != archive.ink.blobs
        || archive.ink_blobs.len() != required.len()
    {
        return Err(CommandError::invalid(
            "Archive handwriting catalog does not match its notes and blobs",
        ));
    }
    let mut manifest = archive.ink.clone();
    let mut packed = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    manifest.blobs.clear();
    for (old, snapshot) in graphs {
        let snapshot = storage::compact_snapshot(snapshot)?;
        let mut blobs = Blobs::new();
        let hash = insert_snapshot(&mut blobs, snapshot.clone())?;
        manifest
            .blobs
            .extend(blobs.iter().map(|(h, b)| (*h, b.len() as u64)));
        hashes.insert(old, hash);
        packed.insert(hash, snapshot);
    }
    for document in &mut manifest.documents {
        for variant in &mut document.variants {
            variant.root_hash = hashes[&variant.root_hash];
        }
    }
    Ok((manifest, packed))
}

pub(crate) fn restore(
    conn: &rusqlite::Transaction<'_>,
    manifest: Manifest,
    graphs: BTreeMap<BlobHash, Snapshot>,
) -> CommandResult<usize> {
    for (hash, snapshot) in &graphs {
        transfer::stage(conn, *hash, snapshot)?;
    }
    let mut count = 0;
    for document in manifest.documents {
        let parents = versions::head_ids(conn, document.page_uuid)?;
        conn.execute(
            "INSERT OR IGNORE INTO ink_documents(page_uuid) VALUES(?1)",
            [document.page_uuid],
        )?;
        conn.execute(
            "DELETE FROM ink_history WHERE page_uuid=?1",
            [document.page_uuid],
        )?;
        conn.execute("UPDATE ink_documents SET root_id=NULL,revision=NULL,cursor=0,dirty=0,editing=0,publication_requested=0,base_version=NULL WHERE page_uuid=?1",[document.page_uuid])?;
        let mut first = None;
        for variant in document.variants {
            let version_uuid = uuid::Uuid::now_v7();
            first.get_or_insert((version_uuid, graphs[&variant.root_hash].root.id));
            let publication = Publish {
                version_uuid,
                page_uuid: document.page_uuid,
                parents: parents.clone(),
                root_hash: variant.root_hash,
                device_name: variant.device_name,
            };
            operation::apply_local_kinds_in_transaction(
                conn,
                vec![OpKind::InkPublish(publication)],
            )?;
            count += 1;
        }
        let (base, root) = first.expect("validated nonempty archive variants");
        conn.execute(
            "DELETE FROM ink_history WHERE page_uuid=?1",
            [document.page_uuid],
        )?;
        conn.execute(
            "INSERT INTO ink_history VALUES(?1,0,?2)",
            params![document.page_uuid, root.as_slice()],
        )?;
        conn.execute("UPDATE ink_documents SET root_id=?2,revision=?3,cursor=0,base_version=?4 WHERE page_uuid=?1",params![document.page_uuid,root.as_slice(),uuid::Uuid::now_v7().to_string(),base])?;
    }
    Ok(count)
}

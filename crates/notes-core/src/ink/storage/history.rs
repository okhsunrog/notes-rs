//! History roots pin complete immutable snapshots, including redo states.
use super::*;
pub(super) fn cursor(conn: &Connection, document: uuid::Uuid) -> CommandResult<i64> {
    conn.query_row(
        "SELECT cursor FROM ink_documents WHERE page_uuid=?1",
        [document],
        |r| r.get(0),
    )
    .map_err(err)
}
pub(in crate::ink) fn persist(conn: &Connection, snapshot: &Snapshot) -> CommandResult<()> {
    for (id, bytes) in &snapshot.chunks {
        put(conn, "ink_chunks", *id, bytes)?;
    }
    for (id, r) in &snapshot.records {
        put(conn, "ink_records", *id, &r.encode().map_err(err)?)?;
    }
    put(
        conn,
        "ink_records",
        snapshot.root.id,
        &snapshot.root.encode().map_err(err)?,
    )?;
    index_root(
        conn,
        snapshot.root.id,
        &model::Document::read(&snapshot.root).map_err(err)?,
    )
}
pub(super) fn set_head(
    conn: &Connection,
    document: uuid::Uuid,
    seq: i64,
    root: Id,
) -> CommandResult<String> {
    // A unique transition token prevents ABA after Undo/Redo. Compaction leaves it alone.
    let revision = uuid::Uuid::now_v7().to_string();
    conn.execute(
        "UPDATE ink_documents SET root_id=?1,revision=?2,cursor=?3,dirty=1 WHERE page_uuid=?4",
        params![root.as_slice(), revision, seq, document],
    )
    .map_err(err)?;
    Ok(revision)
}
pub(super) fn roots(conn: &Connection, document: uuid::Uuid) -> CommandResult<Vec<(i64, Id)>> {
    let mut stmt = conn
        .prepare("SELECT seq,root_id FROM ink_history WHERE page_uuid=?1 ORDER BY seq")
        .map_err(err)?;
    stmt.query_map([document], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
    })
    .map_err(err)?
    .map(|row| {
        let (seq, id) = row.map_err(err)?;
        Ok((
            seq,
            id.try_into()
                .map_err(|_| CommandError::invalid("Invalid history root"))?,
        ))
    })
    .collect()
}
pub(super) fn index_root(conn: &Connection, root: Id, doc: &model::Document) -> CommandResult<()> {
    let mut statement = conn
        .prepare("INSERT OR IGNORE INTO ink_root_refs(root_id,kind,object_id) VALUES(?1,?2,?3)")
        .map_err(err)?;
    for id in doc.records.keys() {
        statement
            .execute(params![root.as_slice(), 0, id.as_slice()])
            .map_err(err)?;
    }
    for id in doc.chunks.keys() {
        statement
            .execute(params![root.as_slice(), 1, id.as_slice()])
            .map_err(err)?;
    }
    Ok(())
}
pub(super) fn collect(conn: &Connection) -> CommandResult<()> {
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS keep_ink_roots(id BLOB PRIMARY KEY) WITHOUT ROWID;
      DELETE FROM keep_ink_roots;
      INSERT OR IGNORE INTO keep_ink_roots SELECT root_id FROM ink_history;
      INSERT OR IGNORE INTO keep_ink_roots SELECT root_id FROM ink_staged_roots;
      INSERT OR IGNORE INTO keep_ink_roots SELECT root_id FROM ink_versions WHERE root_id IS NOT NULL;
      INSERT OR IGNORE INTO keep_ink_roots SELECT root_id FROM ink_documents WHERE root_id IS NOT NULL;
      DELETE FROM ink_root_refs WHERE root_id NOT IN (SELECT id FROM keep_ink_roots);
      DELETE FROM ink_records WHERE id NOT IN (SELECT id FROM keep_ink_roots UNION SELECT object_id FROM ink_root_refs WHERE kind=0);
      DELETE FROM ink_chunks WHERE id NOT IN (SELECT object_id FROM ink_root_refs WHERE kind=1);").map_err(err)?;
    Ok(())
}

pub(in crate::ink) fn result(
    conn: &Connection,
    document: uuid::Uuid,
) -> CommandResult<(InkHistorySnapshot, BTreeMap<Id, Id>)> {
    let mut objects = BTreeMap::new();
    let snapshot = match load(conn, document)? {
        Some((s, revision)) => {
            objects = page_of(&s)?
                .objects
                .into_iter()
                .map(|r| (r.object_id, r.record_id))
                .collect();
            InkDraftSnapshot {
                draft: to_draft(&s)?,
                revision: Some(revision),
            }
        }
        None => InkDraftSnapshot {
            draft: InkDraft::default(),
            revision: None,
        },
    };
    let seq = cursor(conn, document)?;
    let (can_undo,can_redo)=conn.query_row("SELECT EXISTS(SELECT 1 FROM ink_history WHERE page_uuid=?2 AND seq<?1),EXISTS(SELECT 1 FROM ink_history WHERE page_uuid=?2 AND seq>?1)",params![seq,document],|r| Ok((r.get(0)?,r.get(1)?))).map_err(err)?;
    Ok((
        InkHistorySnapshot {
            snapshot,
            can_undo,
            can_redo,
        },
        objects,
    ))
}
pub(in super::super) fn navigate_update(
    path: &Store,
    redo: Option<bool>,
    expected: Option<String>,
) -> CommandResult<InkHistoryUpdate> {
    navigate_impl(path, redo, expected, true)
}

fn navigate_impl(
    path: &Store,
    redo: Option<bool>,
    expected: Option<String>,
    incremental: bool,
) -> CommandResult<InkHistoryUpdate> {
    let mut conn = open(path)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    ensure_document(&tx, path.document)?;
    let mut previous = BTreeMap::new();
    if let Some(redo) = redo {
        let current = load(&tx, path.document)?;
        if current.as_ref().map(|(_, r)| r) != expected.as_ref() {
            return Err(CommandError::conflict(
                "The handwriting draft changed. Reopen it before changing history.",
            ));
        }
        if incremental && let Some((snapshot, _)) = &current {
            previous = page_of(snapshot)?
                .objects
                .into_iter()
                .map(|r| (r.object_id, r.record_id))
                .collect();
        }
        let target = cursor(&tx, path.document)? + if redo { 1 } else { -1 };
        let id: Option<Vec<u8>> = tx
            .query_row(
                "SELECT root_id FROM ink_history WHERE page_uuid=?2 AND seq=?1",
                params![target, path.document],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let id = id.ok_or_else(|| CommandError::invalid("No further handwriting history"))?;
        set_head(
            &tx,
            path.document,
            target,
            id.try_into()
                .map_err(|_| CommandError::invalid("Invalid history root"))?,
        )?;
    }
    let (result, objects) = result(&tx, path.document)?; // Validate before committing a navigation.
    let result = if incremental && redo.is_some() {
        let unchanged: BTreeSet<_> = objects
            .into_iter()
            .filter(|(id, record)| previous.get(id) == Some(record))
            .map(|(id, _)| uuid::Uuid::from_bytes(id))
            .collect();
        let draft = result.snapshot.draft;
        InkHistoryUpdate::Patch {
            patch: InkDraftPatch {
                order: draft.strokes.iter().map(|s| s.id).collect(),
                upserts: draft
                    .strokes
                    .into_iter()
                    .filter(|s| !unchanged.contains(&s.id))
                    .collect(),
                background: draft.background,
            },
            base_revision: expected,
            revision: result.snapshot.revision,
            can_undo: result.can_undo,
            can_redo: result.can_redo,
        }
    } else {
        InkHistoryUpdate::Snapshot { history: result }
    };
    tx.commit().map_err(err)?;
    Ok(result)
}

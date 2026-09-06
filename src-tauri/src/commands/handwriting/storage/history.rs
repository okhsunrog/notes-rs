//! History roots pin complete immutable snapshots, including redo states.
use super::*;
pub(super) fn cursor(conn: &Connection) -> CommandResult<i64> {
    conn.query_row("SELECT seq FROM ink_cursor WHERE singleton=1", [], |r| {
        r.get(0)
    })
    .map_err(err)
}
pub(super) fn persist(conn: &Connection, snapshot: &Snapshot) -> CommandResult<()> {
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
    )
}
pub(super) fn set_head(conn: &Connection, seq: i64, root: Id) -> CommandResult<String> {
    // A unique transition token prevents ABA after Undo/Redo. Compaction leaves it alone.
    let revision = uuid::Uuid::now_v7().to_string();
    conn.execute("INSERT INTO ink_head VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET root_id=excluded.root_id,revision=excluded.revision",params![root.as_slice(),revision]).map_err(err)?;
    conn.execute("UPDATE ink_cursor SET seq=?1 WHERE singleton=1", [seq])
        .map_err(err)?;
    Ok(revision)
}
pub(super) fn roots(conn: &Connection) -> CommandResult<Vec<(i64, Id)>> {
    let mut stmt = conn
        .prepare("SELECT seq,root_id FROM ink_history ORDER BY seq")
        .map_err(err)?;
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
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
/// A complete set of live references for the transaction's current history roots.
/// Publication may reuse already validated documents instead of decoding them again.
#[derive(Default)]
pub(super) struct Retained {
    records: BTreeSet<Id>,
    chunks: BTreeSet<Id>,
}
impl Retained {
    pub(super) fn pin(&mut self, root: Id, doc: &model::Document) {
        self.records.insert(root);
        self.records.extend(doc.records.keys().copied());
        self.chunks.extend(doc.chunks.keys().copied());
    }
    pub(super) fn collect(self, conn: &Connection) -> CommandResult<()> {
        conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS keep_records(id BLOB PRIMARY KEY) WITHOUT ROWID; CREATE TEMP TABLE IF NOT EXISTS keep_chunks(id BLOB PRIMARY KEY) WITHOUT ROWID; DELETE FROM keep_records; DELETE FROM keep_chunks;").map_err(err)?;
        {
            #[cfg(test)]
            let _span = profile::span("gc.mark_sql");
            let mut records = conn
                .prepare("INSERT INTO keep_records VALUES(?1)")
                .map_err(err)?;
            for id in self.records {
                records.execute([id.as_slice()]).map_err(err)?;
            }
            let mut chunks = conn
                .prepare("INSERT INTO keep_chunks VALUES(?1)")
                .map_err(err)?;
            for id in self.chunks {
                chunks.execute([id.as_slice()]).map_err(err)?;
            }
        }
        #[cfg(test)]
        let _span = profile::span("gc.delete_sql");
        conn.execute_batch("DELETE FROM ink_records WHERE id NOT IN (SELECT id FROM keep_records); DELETE FROM ink_chunks WHERE id NOT IN (SELECT id FROM keep_chunks);").map_err(err)?;
        Ok(())
    }
}
pub(super) fn collect(conn: &Connection) -> CommandResult<()> {
    let mut retained = Retained::default();
    for (_, id) in roots(conn)? {
        #[cfg(test)]
        let _span = profile::span("gc.read_root");
        let doc = model::Document::read(&get_record(conn, id, None)?).map_err(err)?;
        retained.pin(id, &doc);
    }
    retained.collect(conn)
}

fn result(conn: &Connection) -> CommandResult<(InkHistorySnapshot, BTreeMap<Id, Id>)> {
    let mut objects = BTreeMap::new();
    let snapshot = match load(conn)? {
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
    let seq = cursor(conn)?;
    let (can_undo,can_redo)=conn.query_row("SELECT EXISTS(SELECT 1 FROM ink_history WHERE seq<?1),EXISTS(SELECT 1 FROM ink_history WHERE seq>?1)",[seq],|r| Ok((r.get(0)?,r.get(1)?))).map_err(err)?;
    Ok((
        InkHistorySnapshot {
            snapshot,
            can_undo,
            can_redo,
        },
        objects,
    ))
}
#[cfg(test)]
pub(in super::super) fn navigate(
    path: &Path,
    redo: Option<bool>,
    expected: Option<String>,
) -> CommandResult<InkHistorySnapshot> {
    match navigate_impl(path, redo, expected, false)? {
        InkHistoryUpdate::Snapshot { history } => Ok(history),
        _ => unreachable!(),
    }
}

pub(in super::super) fn navigate_update(
    path: &Path,
    redo: Option<bool>,
    expected: Option<String>,
) -> CommandResult<InkHistoryUpdate> {
    navigate_impl(path, redo, expected, true)
}

fn navigate_impl(
    path: &Path,
    redo: Option<bool>,
    expected: Option<String>,
    incremental: bool,
) -> CommandResult<InkHistoryUpdate> {
    let mut conn = open(path)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    let mut previous = BTreeMap::new();
    if let Some(redo) = redo {
        let current = load(&tx)?;
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
        let target = cursor(&tx)? + if redo { 1 } else { -1 };
        let id: Option<Vec<u8>> = tx
            .query_row(
                "SELECT root_id FROM ink_history WHERE seq=?1",
                [target],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let id = id.ok_or_else(|| CommandError::invalid("No further handwriting history"))?;
        set_head(
            &tx,
            target,
            id.try_into()
                .map_err(|_| CommandError::invalid("Invalid history root"))?,
        )?;
    }
    let (result, objects) = result(&tx)?; // Validate before committing a navigation.
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

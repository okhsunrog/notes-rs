//! Repack immutable segments across every retained history root.
//! Compression runs outside the publication transaction and never holds the input/store mutex.
use super::*;
const TARGET_POINTS: usize = 250_000;
const MAX_JOB_BYTES: u64 = 128 * 1024 * 1024;
// Each fresh block is one normalized stroke, at most 150k points * 49 bytes.
const MAX_FRESH_CHUNKS: usize = 16;
const MAX_FRESH_BYTES: u64 = 8 * 1024 * 1024;
type Packed = (BTreeMap<Id, Vec<u8>>, BTreeMap<Id, model::SegmentRef>);
struct Plan {
    roots: Vec<(i64, Id)>,
    revision: Option<String>,
    documents: Vec<(i64, model::Document)>,
    chunks: BTreeMap<Id, Vec<u8>>,
    locations: BTreeMap<Id, model::SegmentRef>,
}
fn prepare(path: &Path) -> CommandResult<Option<Plan>> {
    let mut conn = open(path)?;
    let tx = conn.transaction().map_err(err)?;
    let roots = history::roots(&tx)?;
    let revision = tx
        .query_row("SELECT revision FROM ink_head WHERE singleton=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    let mut documents = Vec::new();
    let mut catalog = BTreeMap::new();
    let mut locations = BTreeMap::new();
    for (seq, id) in &roots {
        let root = get_record(&tx, *id, None)?;
        if root.extensions != cbor::map([]) || !root.resources.is_empty() {
            return Err(CommandError::invalid("Unsupported compaction root"));
        }
        let doc = model::Document::read(&root).map_err(err)?;
        for (id, entry) in &doc.chunks {
            if let Some(prior) = catalog.insert(*id, entry.clone()) {
                if prior != *entry {
                    return Err(CommandError::invalid("Conflicting chunk catalog"));
                }
            }
        }
        for (id, location) in &doc.segments {
            if let Some(prior) = locations.insert(*id, location.clone()) {
                if prior != *location {
                    return Err(CommandError::invalid("Conflicting segment location"));
                }
            }
        }
        documents.push((*seq, doc));
    }
    let sealed: BTreeSet<Vec<u8>> = tx
        .prepare("SELECT id FROM ink_sealed_chunks")
        .map_err(err)?
        .query_map([], |r| r.get(0))
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    catalog.retain(|id, _| !sealed.contains(id.as_slice()));
    let mut selected_bytes = 0;
    let mut selected_count = 0;
    catalog.retain(|_, entry| {
        if selected_count >= MAX_FRESH_CHUNKS || selected_bytes + entry.length > MAX_FRESH_BYTES {
            return false;
        }
        selected_count += 1;
        selected_bytes += entry.length;
        true
    });
    locations.retain(|_, location| catalog.contains_key(&location.chunk));
    let size = catalog
        .values()
        .try_fold(0u64, |n, b| n.checked_add(b.length))
        .ok_or_else(|| CommandError::invalid("Compaction size overflow"))?;
    if catalog.len() < 2 || size > MAX_JOB_BYTES {
        return Ok(None);
    }
    let mut chunks = BTreeMap::new();
    for (id, entry) in catalog {
        let bytes: Vec<u8> = tx
            .query_row(
                "SELECT data FROM ink_chunks WHERE id=?1 AND length(data)=?2",
                params![id.as_slice(), entry.length as i64],
                |r| r.get(0),
            )
            .map_err(err)?;
        if blob_ref(&bytes) != entry {
            return Err(CommandError::invalid("Compaction chunk checksum"));
        }
        chunks.insert(id, bytes);
    }
    tx.commit().map_err(err)?;
    Ok(Some(Plan {
        roots,
        revision,
        documents,
        chunks,
        locations,
    }))
}
fn finish(
    builder: &mut Option<Chunk>,
    chunks: &mut BTreeMap<Id, Vec<u8>>,
    locations: &mut BTreeMap<Id, model::SegmentRef>,
) -> CommandResult<()> {
    if let Some(chunk) = builder.take() {
        for (index, (id, _)) in chunk.segments.iter().enumerate() {
            locations.insert(
                *id,
                model::SegmentRef {
                    chunk: chunk.id,
                    index: index as u32,
                },
            );
        }
        chunks.insert(chunk.id, chunk.encode(Encoding::Pco8).map_err(err)?);
    }
    Ok(())
}
fn repack(plan: &Plan) -> CommandResult<Option<Packed>> {
    repack_with_target(plan, TARGET_POINTS)
}
fn repack_with_target(plan: &Plan, target: usize) -> CommandResult<Option<Packed>> {
    let mut chunks = BTreeMap::new();
    let mut locations = BTreeMap::new();
    let mut builder: Option<Chunk> = None;
    let mut decoded_size = 0u64;
    for (id, bytes) in &plan.chunks {
        let chunk = Chunk::decode(bytes).map_err(err)?;
        if chunk.id != *id || chunk.columns.iter().any(|c| c.validity.is_some()) {
            return Err(CommandError::invalid("Unsupported compaction layout"));
        }
        decoded_size += chunk
            .columns
            .iter()
            .map(|c| c.values.len() as u64)
            .sum::<u64>();
        if decoded_size > MAX_JOB_BYTES {
            return Ok(None);
        }
        let mut first = 0;
        for (index, (segment, count)) in chunk.segments.iter().enumerate() {
            let count = *count as usize;
            if let Some(source) = plan.locations.get(segment) {
                if source.chunk != chunk.id || source.index as usize != index {
                    return Err(CommandError::invalid("Compaction segment identity"));
                }
                let compatible = builder.as_ref().is_none_or(|b| {
                    b.segments.len() < 65_536
                        && b.profile == chunk.profile
                        && b.count().is_ok_and(|n| n + count <= target)
                        && b.columns.len() == chunk.columns.len()
                        && b.columns.iter().zip(&chunk.columns).all(|(a, b)| {
                            a.semantic == b.semantic
                                && a.dtype == b.dtype
                                && a.required == b.required
                        })
                });
                if !compatible {
                    finish(&mut builder, &mut chunks, &mut locations)?;
                }
                let b = builder.get_or_insert_with(|| Chunk {
                    id: *uuid::Uuid::now_v7().as_bytes(),
                    profile: chunk.profile,
                    segments: vec![],
                    columns: chunk
                        .columns
                        .iter()
                        .map(|c| Column {
                            semantic: c.semantic,
                            dtype: c.dtype,
                            required: c.required,
                            validity: None,
                            values: vec![],
                        })
                        .collect(),
                });
                b.segments.push((*segment, count as u32));
                for (out, input) in b.columns.iter_mut().zip(&chunk.columns) {
                    let width = input.dtype.width();
                    out.values
                        .extend_from_slice(&input.values[first * width..(first + count) * width]);
                }
            }
            first += count;
        }
    }
    finish(&mut builder, &mut chunks, &mut locations)?;
    if locations.len() != plan.locations.len() {
        return Err(CommandError::invalid("Compaction lost a segment"));
    }
    let before: usize = plan.chunks.values().map(Vec::len).sum();
    let after: usize = chunks.values().map(Vec::len).sum();
    if after >= before {
        return Ok(None);
    }
    Ok(Some((chunks, locations)))
}
fn publish(
    path: &Path,
    plan: Plan,
    chunks: BTreeMap<Id, Vec<u8>>,
    locations: BTreeMap<Id, model::SegmentRef>,
) -> CommandResult<bool> {
    let mut replacements = Vec::new();
    for (seq, mut doc) in plan.documents {
        for (id, location) in &mut doc.segments {
            if let Some(replacement) = locations.get(id) {
                *location = replacement.clone();
            }
        }
        doc.chunks = doc
            .segments
            .values()
            .map(|r| {
                (
                    r.chunk,
                    chunks
                        .get(&r.chunk)
                        .map(|bytes| blob_ref(bytes))
                        .unwrap_or_else(|| doc.chunks[&r.chunk].clone()),
                )
            })
            .collect();
        let size = doc
            .chunks
            .values()
            .chain(doc.records.values())
            .try_fold(0u64, |n, b| n.checked_add(b.length))
            .ok_or_else(|| CommandError::invalid("Compaction snapshot size overflow"))?;
        if size > MAX_SNAPSHOT_BYTES as u64 {
            return Ok(false);
        }
        replacements.push((seq, seal(&doc)?));
    }
    let mut conn = open(path)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    let revision: Option<String> = tx
        .query_row("SELECT revision FROM ink_head WHERE singleton=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    if history::roots(&tx)? != plan.roots || revision != plan.revision {
        return Ok(false);
    }
    for (id, bytes) in &chunks {
        put(&tx, "ink_chunks", *id, bytes)?;
        tx.execute("INSERT INTO ink_sealed_chunks VALUES(?1)", [id.as_slice()])
            .map_err(err)?;
    }
    let cursor = history::cursor(&tx)?;
    for (seq, root) in replacements {
        put(&tx, "ink_records", root.id, &root.encode().map_err(err)?)?;
        tx.execute(
            "UPDATE ink_history SET root_id=?1 WHERE seq=?2",
            params![root.id.as_slice(), seq],
        )
        .map_err(err)?;
        if seq == cursor {
            tx.execute(
                "UPDATE ink_head SET root_id=?1 WHERE singleton=1",
                [root.id.as_slice()],
            )
            .map_err(err)?;
        }
    }
    history::collect(&tx)?;
    tx.commit().map_err(err)?;
    Ok(true)
}
pub(in super::super) fn compact(path: &Path) -> CommandResult<bool> {
    let Some(plan) = prepare(path)? else {
        return Ok(false);
    };
    let Some((chunks, locations)) = repack(&plan)? else {
        return Ok(false);
    };
    publish(path, plan, chunks, locations)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sheet() -> InkDraft {
        InkDraft {
            strokes: (0..3)
                .map(|n| InkStroke {
                    id: uuid::Uuid::now_v7(),
                    width: 3.125,
                    points: (0..20)
                        .map(|i| InkPoint {
                            x: if i == 0 {
                                -0.0
                            } else {
                                i as f64 + 0.1234567890123
                            },
                            y: 20. + n as f64,
                            pressure: 0.71234567890123,
                            tilt_x: -12.5,
                            tilt_y: 3.25,
                            time: 1788660500000.125 + i as f64,
                        })
                        .collect(),
                })
                .collect(),
            ..InkDraft::default()
        }
    }
    fn equal(a: &InkDraft, b: &InkDraft) {
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap()
        );
        for (a, b) in a.strokes.iter().zip(&b.strokes) {
            for (a, b) in a.points.iter().zip(&b.points) {
                for (a, b) in [a.x, a.y, a.pressure, a.tilt_x, a.tilt_y, a.time]
                    .into_iter()
                    .zip([b.x, b.y, b.pressure, b.tilt_x, b.tilt_y, b.time])
                {
                    assert_eq!(a.to_bits(), b.to_bits());
                }
            }
        }
    }
    fn rows(conn: &Connection, table: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
        conn.prepare(&format!("SELECT id,data FROM {table} ORDER BY id"))
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
    #[test]
    fn compaction_preserves_undo_redo_bits_geometry_and_revision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let a = sheet();
        let rev = write_draft(&path, a.clone(), None).unwrap();
        let mut b = a.clone();
        b.background = InkBackground::Grid;
        b.strokes[0].points[1].x = 123.456789012345;
        let rev = write_patch(
            &path,
            InkDraftPatch {
                order: b.strokes.iter().map(|s| s.id).collect(),
                upserts: vec![b.strokes[0].clone()],
                background: InkBackground::Grid,
            },
            Some(rev),
        )
        .unwrap();
        let mut c = b.clone();
        c.strokes.remove(0);
        let rev = write_patch(
            &path,
            InkDraftPatch {
                order: c.strokes.iter().map(|s| s.id).collect(),
                upserts: vec![],
                background: InkBackground::Grid,
            },
            Some(rev),
        )
        .unwrap();
        let undo = history::navigate(&path, Some(false), Some(rev)).unwrap();
        let conn = open(&path).unwrap();
        let before = rows(&conn, "ink_chunks");
        let geometries: Vec<_> = rows(&conn, "ink_records")
            .into_iter()
            .filter(|(_, b)| Record::decode(b).unwrap().kind == ink_format::record::kind::GEOMETRY)
            .collect();
        drop(conn);
        assert!(compact(&path).unwrap());
        let after = read(&path).unwrap();
        assert_eq!(after.revision, undo.snapshot.revision);
        equal(&after.draft, &b);
        let conn = open(&path).unwrap();
        let packed = rows(&conn, "ink_chunks");
        assert_eq!(packed.len(), 1);
        assert!(packed[0].1.len() < before.iter().map(|(_, b)| b.len()).sum());
        assert!(
            geometries
                .iter()
                .all(|r| rows(&conn, "ink_records").contains(r))
        );
        drop(conn);
        let old = history::navigate(&path, Some(false), after.revision).unwrap();
        equal(&old.snapshot.draft, &a);
        let empty = history::navigate(&path, Some(false), old.snapshot.revision).unwrap();
        assert!(empty.snapshot.draft.strokes.is_empty());
        let a2 = history::navigate(&path, Some(true), empty.snapshot.revision).unwrap();
        equal(&a2.snapshot.draft, &a);
        let b2 = history::navigate(&path, Some(true), a2.snapshot.revision).unwrap();
        equal(&b2.snapshot.draft, &b);
        let c2 = history::navigate(&path, Some(true), b2.snapshot.revision).unwrap();
        equal(&c2.snapshot.draft, &c);
        assert!(!compact(&path).unwrap());
    }
    #[test]
    fn stale_preparation_and_failed_publication_never_replace_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let rev = write_draft(&path, sheet(), None).unwrap();
        let plan = prepare(&path).unwrap().unwrap();
        let (chunks, locations) = repack(&plan).unwrap().unwrap();
        let new = write_draft(&path, sheet(), Some(rev)).unwrap();
        let conn = open(&path).unwrap();
        let roots = history::roots(&conn).unwrap();
        drop(conn);
        assert!(!publish(&path, plan, chunks, locations).unwrap());
        assert_eq!(read(&path).unwrap().revision, Some(new.clone()));
        let conn = open(&path).unwrap();
        assert_eq!(history::roots(&conn).unwrap(), roots);
        let records = rows(&conn, "ink_records");
        let chunks = rows(&conn, "ink_chunks");
        conn.execute_batch("CREATE TRIGGER fail_compaction BEFORE UPDATE ON ink_head BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        drop(conn);
        assert!(compact(&path).is_err());
        let conn = open(&path).unwrap();
        assert_eq!(history::roots(&conn).unwrap(), roots);
        assert_eq!(rows(&conn, "ink_records"), records);
        assert_eq!(rows(&conn, "ink_chunks"), chunks);
        assert_eq!(read(&path).unwrap().revision, Some(new));
    }
    #[test]
    fn subsequent_packs_leave_sealed_blocks_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let mut draft = sheet();
        let revision = write_draft(&path, draft.clone(), None).unwrap();
        assert!(compact(&path).unwrap());
        let sealed = rows(&open(&path).unwrap(), "ink_chunks");
        let fresh = sheet().strokes;
        draft.strokes.extend(fresh.clone());
        write_patch(
            &path,
            InkDraftPatch {
                order: draft.strokes.iter().map(|s| s.id).collect(),
                upserts: fresh,
                background: InkBackground::Plain,
            },
            Some(revision),
        )
        .unwrap();
        let plan = prepare(&path).unwrap().unwrap();
        assert_eq!(plan.chunks.len(), 3);
        assert!(compact(&path).unwrap());
        let after = rows(&open(&path).unwrap(), "ink_chunks");
        assert_eq!(after.len(), 2);
        assert!(sealed.iter().all(|row| after.contains(row)));
        equal(&read(&path).unwrap().draft, &draft);
        assert!(prepare(&path).unwrap().is_none());
    }

    #[test]
    fn bounded_packs_keep_whole_segments_and_restore_the_same_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let original = sheet();
        write_draft(&path, original.clone(), None).unwrap();
        let plan = prepare(&path).unwrap().unwrap();
        let (chunks, locations) = repack_with_target(&plan, 40).unwrap().unwrap();
        assert_eq!(chunks.len(), 2);
        for bytes in chunks.values() {
            let chunk = Chunk::decode(bytes).unwrap();
            assert!(chunk.count().unwrap() <= 40);
            assert!(chunk.segments.iter().all(|(_, n)| *n == 20));
        }
        assert!(publish(&path, plan, chunks, locations).unwrap());
        equal(&read(&path).unwrap().draft, &original);
    }
}

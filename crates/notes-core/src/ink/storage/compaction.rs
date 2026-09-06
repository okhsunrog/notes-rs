//! Repack immutable segments across every retained history root.
//! Compression runs outside the publication transaction and never holds the input/store mutex.
use super::*;
const TARGET_POINTS: usize = 250_000;
const MAX_JOB_BYTES: u64 = 128 * 1024 * 1024;
// Each fresh block is one normalized stroke, at most 150k points * 49 bytes.
const MAX_FRESH_BYTES: u64 = 8 * 1024 * 1024;
// Bounded, job-local reuse of validated typed metadata (not decoded points).
const MAX_CACHED_ROOT_ENTRIES: usize = 65_536;
type Packed = (BTreeMap<Id, Vec<u8>>, BTreeMap<Id, model::SegmentRef>);
struct Plan {
    roots: BTreeMap<Id, model::Document>,
    chunks: BTreeMap<Id, Vec<u8>>,
    locations: BTreeMap<Id, model::SegmentRef>,
}
fn prepare_with_limits(
    path: &Store,
    max_chunks: usize,
    max_decoded_bytes: u64,
) -> CommandResult<Option<Plan>> {
    if !(2..=512).contains(&max_chunks) {
        return Err(CommandError::invalid("Unsupported compaction batch limit"));
    }
    if max_decoded_bytes == 0 || max_decoded_bytes > MAX_JOB_BYTES {
        return Err(CommandError::invalid(
            "Unsupported compaction decoded budget",
        ));
    }
    let mut conn = open(path)?;
    let tx = conn.transaction().map_err(err)?;
    ensure_document(&tx, path.document)?;
    // No roots need decoding if fewer than two fresh blocks remain. This also
    // makes the worker's final idle check cheap; no corruption is hidden on reads.
    let fresh: i64 = tx.query_row("SELECT count(*) FROM (SELECT DISTINCT r.object_id FROM ink_history h JOIN ink_root_refs r ON r.root_id=h.root_id AND r.kind=1 WHERE h.page_uuid=?1 AND NOT EXISTS(SELECT 1 FROM ink_sealed_chunks s WHERE s.id=r.object_id) LIMIT 2)", [path.document], |r| r.get(0)).map_err(err)?;
    if fresh < 2 {
        return Ok(None);
    }
    let roots = history::roots(&tx, path.document)?;
    let mut cached_roots = BTreeMap::new();
    let mut cached_entries = 0;
    let mut catalog = BTreeMap::new();
    let mut locations = BTreeMap::new();
    for (_, id) in &roots {
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
        let entries = doc.records.len() + doc.chunks.len() + doc.segments.len() + doc.pages.len();
        // Unknown extension/resource payloads are not included in this entry budget.
        if doc.extra.is_empty()
            && doc.resources.is_empty()
            && cached_entries + entries <= MAX_CACHED_ROOT_ENTRIES
            && !cached_roots.contains_key(id)
        {
            cached_entries += entries;
            cached_roots.insert(*id, doc);
        }
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
    let mut decoded_bytes = 0;
    let mut chunks = BTreeMap::new();
    for (id, entry) in catalog {
        if chunks.len() >= max_chunks {
            break;
        }
        if selected_bytes + entry.length > MAX_FRESH_BYTES {
            continue;
        }
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
        // Inspect validated framing before decompression. Highly compressible
        // history may otherwise fit the encoded budget but exceed the work budget.
        let decoded = Chunk::decoded_value_bytes(&bytes).map_err(err)? as u64;
        if decoded_bytes + decoded > max_decoded_bytes {
            continue;
        }
        selected_bytes += entry.length;
        decoded_bytes += decoded;
        chunks.insert(id, bytes);
    }
    if chunks.len() < 2 {
        return Ok(None);
    }
    locations.retain(|_, location| chunks.contains_key(&location.chunk));
    tx.commit().map_err(err)?;
    Ok(Some(Plan {
        roots: cached_roots,
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
    path: &Store,
    plan: Plan,
    chunks: BTreeMap<Id, Vec<u8>>,
    locations: BTreeMap<Id, model::SegmentRef>,
) -> CommandResult<bool> {
    let mut conn = open(path)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    let mut replacements = Vec::new();
    ensure_document(&tx, path.document)?;
    // Hash each output block once, rather than once per segment per history root.
    let chunk_refs: BTreeMap<_, _> = chunks
        .iter()
        .map(|(id, bytes)| (*id, blob_ref(bytes)))
        .collect();
    for (seq, root_id) in history::roots(&tx, path.document)? {
        let mut doc = {
            // The transaction rereads the current history root IDs. Reuse only
            // exact immutable roots validated during preparation; concurrent edits
            // and competing compactions create different roots and take the fallback.
            match plan.roots.get(&root_id) {
                Some(doc) => doc.clone(),
                None => model::Document::read(&get_record(&tx, root_id, None)?).map_err(err)?,
            }
        };
        let mut changed = false;
        for (id, location) in &mut doc.segments {
            if let Some(replacement) = locations.get(id) {
                let source = &plan.locations[id];
                if source.chunk != location.chunk || source.index != location.index {
                    return Ok(false);
                }
                changed = true;
                *location = replacement.clone();
            }
        }
        if !changed {
            continue;
        }
        doc.chunks = doc
            .segments
            .values()
            .map(|r| r.chunk)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| {
                let entry = chunk_refs.get(&id).unwrap_or_else(|| &doc.chunks[&id]);
                (id, entry.clone())
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
        let root = seal(&doc)?;

        replacements.push((seq, root));
    }
    if replacements.is_empty() {
        return Ok(false);
    }
    for (id, bytes) in &chunks {
        put(&tx, "ink_chunks", *id, bytes)?;
        tx.execute("INSERT INTO ink_sealed_chunks VALUES(?1)", [id.as_slice()])
            .map_err(err)?;
    }
    let cursor = history::cursor(&tx, path.document)?;
    for (seq, root) in replacements {
        put(&tx, "ink_records", root.id, &root.encode().map_err(err)?)?;
        history::index_root(&tx, root.id, &model::Document::read(&root).map_err(err)?)?;
        tx.execute(
            "UPDATE ink_history SET root_id=?1 WHERE seq=?2 AND page_uuid=?3",
            params![root.id.as_slice(), seq, path.document],
        )
        .map_err(err)?;
        if seq == cursor {
            tx.execute(
                "UPDATE ink_documents SET root_id=?1 WHERE page_uuid=?2",
                params![root.id.as_slice(), path.document],
            )
            .map_err(err)?;
        }
    }
    {
        history::collect(&tx)?;
    }
    tx.commit().map_err(err)?;
    drop(conn);
    Ok(true)
}
pub(in super::super) fn compact_with_limits(
    path: &Store,
    max_chunks: usize,
    max_decoded_bytes: u64,
) -> CommandResult<bool> {
    if max_decoded_bytes < (2 * MAX_POINTS * 49) as u64 {
        return Err(CommandError::invalid(
            "Compaction budget cannot fit two fresh strokes",
        ));
    }
    let plan = prepare_with_limits(path, max_chunks, max_decoded_bytes)?;
    let Some(plan) = plan else {
        return Ok(false);
    };
    let Some((chunks, locations)) = repack(&plan)? else {
        return Ok(false);
    };
    publish(path, plan, chunks, locations)
}

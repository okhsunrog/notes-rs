//! Repack immutable segments across every retained history root.
//! Compression runs outside the publication transaction and never holds the input/store mutex.
use super::*;
const TARGET_POINTS: usize = 250_000;
const MAX_JOB_BYTES: u64 = 128 * 1024 * 1024;
// Each fresh block is one normalized stroke, at most 150k points * 49 bytes.
const MAX_FRESH_CHUNKS: usize = 16;
const MAX_FRESH_BYTES: u64 = 8 * 1024 * 1024;
// Bounded, job-local reuse of validated typed metadata (not decoded points).
const MAX_CACHED_ROOT_ENTRIES: usize = 65_536;
type Packed = (BTreeMap<Id, Vec<u8>>, BTreeMap<Id, model::SegmentRef>);
struct Plan {
    roots: BTreeMap<Id, model::Document>,
    chunks: BTreeMap<Id, Vec<u8>>,
    locations: BTreeMap<Id, model::SegmentRef>,
}
fn prepare(path: &Path) -> CommandResult<Option<Plan>> {
    prepare_with_limit(path, MAX_FRESH_CHUNKS)
}
fn prepare_with_limit(path: &Path, max_chunks: usize) -> CommandResult<Option<Plan>> {
    prepare_with_limits(path, max_chunks, MAX_JOB_BYTES)
}
fn prepare_with_limits(
    path: &Path,
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
    // No roots need decoding if fewer than two fresh blocks remain. This also
    // makes the worker's final idle check cheap; no corruption is hidden on reads.
    let fresh: i64 = tx.query_row("SELECT count(*) FROM (SELECT 1 FROM ink_chunks c WHERE NOT EXISTS(SELECT 1 FROM ink_sealed_chunks s WHERE s.id=c.id) LIMIT 2)", [], |r| r.get(0)).map_err(err)?;
    if fresh < 2 {
        return Ok(None);
    }
    let roots = history::roots(&tx)?;
    let mut cached_roots = BTreeMap::new();
    let mut cached_entries = 0;
    let mut catalog = BTreeMap::new();
    let mut locations = BTreeMap::new();
    #[cfg(test)]
    let roots_span = profile::span("prepare.roots");
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
    #[cfg(test)]
    drop(roots_span);
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
    path: &Path,
    plan: Plan,
    chunks: BTreeMap<Id, Vec<u8>>,
    locations: BTreeMap<Id, model::SegmentRef>,
) -> CommandResult<bool> {
    let mut conn = open(path)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    let mut replacements = Vec::new();
    let mut retained = history::Retained::default();
    // Hash each output block once, rather than once per segment per history root.
    let chunk_refs: BTreeMap<_, _> = chunks
        .iter()
        .map(|(id, bytes)| (*id, blob_ref(bytes)))
        .collect();
    for (seq, root_id) in history::roots(&tx)? {
        let mut doc = {
            #[cfg(test)]
            let _span = profile::span("publish.read_root");
            // The transaction rereads the current history root IDs. Reuse only
            // exact immutable roots validated during preparation; concurrent edits
            // and competing compactions create different roots and take the fallback.
            match plan.roots.get(&root_id) {
                Some(doc) => doc.clone(),
                None => model::Document::read(&get_record(&tx, root_id, None)?).map_err(err)?,
            }
        };
        #[cfg(test)]
        let remap_span = profile::span("publish.remap");
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
            retained.pin(root_id, &doc);
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
        #[cfg(test)]
        drop(remap_span);
        #[cfg(test)]
        let _span = profile::span("publish.seal");
        let root = seal(&doc)?;
        retained.pin(root.id, &doc);
        replacements.push((seq, root));
    }
    if replacements.is_empty() {
        return Ok(false);
    }
    #[cfg(test)]
    let persist_span = profile::span("publish.persist");
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
    #[cfg(test)]
    drop(persist_span);
    {
        #[cfg(test)]
        let _span = profile::span("publish.gc");
        retained.collect(&tx)?;
    }
    #[cfg(test)]
    let _span = profile::span("publish.commit_close");
    #[cfg(test)]
    profile::kill_point("before_commit");
    tx.commit().map_err(err)?;
    #[cfg(test)]
    profile::kill_point("after_commit");
    drop(conn);
    Ok(true)
}
#[cfg(test)]
pub(in super::super) fn compact(path: &Path) -> CommandResult<bool> {
    compact_with_limits(path, MAX_FRESH_CHUNKS, MAX_JOB_BYTES)
}
/// Internal policy seam shared with the benchmark. The installed scheduler uses
/// `compact` defaults. The minimum fits two maximum-sized normalized fresh strokes.
pub(in super::super) fn compact_with_limits(
    path: &Path,
    max_chunks: usize,
    max_decoded_bytes: u64,
) -> CommandResult<bool> {
    if max_decoded_bytes < (2 * MAX_POINTS * 49) as u64 {
        return Err(CommandError::invalid(
            "Compaction budget cannot fit two fresh strokes",
        ));
    }
    let plan = if max_chunks == MAX_FRESH_CHUNKS && max_decoded_bytes == MAX_JOB_BYTES {
        prepare(path)?
    } else {
        prepare_with_limits(path, max_chunks, max_decoded_bytes)?
    };
    let Some(plan) = plan else {
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
    #[test]
    fn decoded_budget_selects_partial_jobs_and_keeps_making_progress() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let mut original = sheet();
        let mut fourth = original.strokes[0].clone();
        fourth.id = uuid::Uuid::now_v7();
        original.strokes.push(fourth);
        let revision = write_draft(&path, original.clone(), None).unwrap();
        // Each stroke is 20 points * 49 value bytes. Encoded size is deliberately
        // irrelevant: two jobs must fit even though all four blocks compress well.
        for _ in 0..2 {
            let plan = prepare_with_limits(&path, 512, 1960).unwrap().unwrap();
            assert_eq!(plan.chunks.len(), 2);
            assert_eq!(
                plan.chunks
                    .values()
                    .map(|b| Chunk::decoded_value_bytes(b).unwrap())
                    .sum::<usize>(),
                1960
            );
            let (chunks, locations) = repack(&plan).unwrap().unwrap();
            assert!(publish(&path, plan, chunks, locations).unwrap());
        }
        assert!(prepare_with_limits(&path, 512, 1960).unwrap().is_none());
        equal(&read(&path).unwrap().draft, &original);
        assert_eq!(read(&path).unwrap().revision, Some(revision.clone()));
        assert!(compact_with_limits(&path, 512, 1960).is_err());
        let undo = history::navigate(&path, Some(false), Some(revision)).unwrap();
        assert!(undo.snapshot.draft.strokes.is_empty());
        equal(
            &history::navigate(&path, Some(true), undo.snapshot.revision)
                .unwrap()
                .snapshot
                .draft,
            &original,
        );
    }

    #[test]
    #[ignore = "large isolated history stress; requires INK_STRESS_DEST"]
    fn large_compressible_history_respects_decoded_budget() {
        let path = std::path::PathBuf::from(std::env::var("INK_STRESS_DEST").unwrap());
        assert!(path.is_absolute() && !path.exists());
        assert!(
            !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        );
        assert!(
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("ink-compaction-")
        );
        for suffix in ["-wal", "-shm"] {
            assert!(!std::path::PathBuf::from(format!("{}{suffix}", path.display())).exists());
        }
        let mut draft = sheet();
        draft.strokes.truncate(1);
        draft.strokes[0].points = (0..75_000)
            .map(|i| InkPoint {
                x: (i % 1000) as f64,
                y: 20.,
                pressure: 0.7,
                tilt_x: -12.5,
                tilt_y: 3.25,
                time: 1788660500000.125 + i as f64,
            })
            .collect();
        let mut revision = None;
        for version in 0..40 {
            for p in &mut draft.strokes[0].points {
                p.y = version as f64;
            }
            revision = Some(write_draft(&path, draft.clone(), revision).unwrap());
        }
        let conn = open(&path).unwrap();
        let before_roots = history::roots(&conn).unwrap();
        let encoded_bytes: usize = rows(&conn, "ink_chunks").iter().map(|(_, b)| b.len()).sum();
        drop(conn);
        let budget = 16 * 1024 * 1024;
        let timer = std::time::Instant::now();
        let mut jobs = 0;
        let mut max_decoded = 0;
        while let Some(plan) = prepare_with_limits(&path, 512, budget).unwrap() {
            let bytes: usize = plan
                .chunks
                .values()
                .map(|b| Chunk::decoded_value_bytes(b).unwrap())
                .sum();
            assert!(bytes <= budget as usize);
            max_decoded = max_decoded.max(bytes);
            let (chunks, locations) = repack(&plan)
                .unwrap()
                .expect("compressible batch must make progress");
            assert!(publish(&path, plan, chunks, locations).unwrap());
            jobs += 1;
            assert!(jobs <= 20);
        }
        let elapsed = timer.elapsed().as_secs_f64();
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let peak = status.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
        assert_eq!(jobs, 10);
        assert_eq!(read(&path).unwrap().revision, revision);
        for version in (0..40).rev() {
            for p in &mut draft.strokes[0].points {
                p.y = version as f64;
            }
            let snapshot = read(&path).unwrap();
            equal(&snapshot.draft, &draft);
            history::navigate(&path, Some(false), snapshot.revision).unwrap();
        }
        assert!(read(&path).unwrap().draft.strokes.is_empty());
        let conn = open(&path).unwrap();
        assert_eq!(history::roots(&conn).unwrap().len(), before_roots.len());
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        println!(
            "INK_STRESS {}",
            serde_json::json!({"history_states":41,"points_across_history":3_000_000,"decoded_bytes_across_history":147_000_000,"encoded_bytes_before":encoded_bytes,"budget":budget,"max_job_decoded_bytes":max_decoded,"jobs":jobs,"compaction_seconds":elapsed,"peak_before_verification":peak,"verified":true})
        );
    }
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
    /// Run only against a closed, disposable benchmark DB. Production paths are
    /// not accepted. The source is copied and never mutated.
    #[test]
    #[ignore = "requires INK_PROFILE_SOURCE and INK_PROFILE_DEST in a benchmark directory"]
    fn profile_recorded_exit() {
        use std::time::Instant;
        let source = std::path::PathBuf::from(std::env::var("INK_PROFILE_SOURCE").unwrap());
        let path = std::path::PathBuf::from(std::env::var("INK_PROFILE_DEST").unwrap());
        for p in [&source, &path] {
            assert!(p.is_absolute());
            assert!(
                !p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            );
            assert!(
                p.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("ink-compaction-")
            );
        }
        for p in [&source, &path] {
            for suffix in ["-wal", "-shm"] {
                assert!(!std::path::PathBuf::from(format!("{}{suffix}", p.display())).exists());
            }
        }
        let mut input = std::fs::File::open(&source).unwrap();
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        std::io::copy(&mut input, &mut output).unwrap();
        output.sync_all().unwrap();
        drop(output);
        // Suite verification left the cursor at the oldest retained state.
        loop {
            let state = history::navigate(&path, None, None).unwrap();
            if !state.can_redo {
                break;
            }
            history::navigate(&path, Some(true), state.snapshot.revision).unwrap();
        }
        let before = read(&path).unwrap();
        // Optional experiment only; the production scheduler retains its default.
        let max_chunks = std::env::var("INK_PROFILE_CHUNKS")
            .ok()
            .map(|v| v.parse::<usize>().unwrap())
            .unwrap_or(MAX_FRESH_CHUNKS);
        println!(
            "INK_PROFILE_BASE {}",
            serde_json::json!({"revision":before.revision})
        );
        profile::start();
        let mut prepare_ms = 0.;
        let mut repack_ms = 0.;
        let mut publish_ms = 0.;
        let mut batches = 0;
        loop {
            let t = Instant::now();
            let plan = prepare_with_limit(&path, max_chunks).unwrap();
            prepare_ms += t.elapsed().as_secs_f64() * 1000.;
            let Some(plan) = plan else {
                break;
            };
            let t = Instant::now();
            let packed = repack(&plan).unwrap();
            repack_ms += t.elapsed().as_secs_f64() * 1000.;
            let Some((chunks, locations)) = packed else {
                break;
            };
            let t = Instant::now();
            assert!(publish(&path, plan, chunks, locations).unwrap());
            publish_ms += t.elapsed().as_secs_f64() * 1000.;
            batches += 1;
        }
        let phases = profile::finish();
        let after = read(&path).unwrap();
        assert_eq!(before.revision, after.revision);
        equal(&before.draft, &after.draft);
        println!(
            "INK_PROFILE {}",
            serde_json::json!({"max_chunks":max_chunks,"batches":batches,"strokes":after.draft.strokes.len(),"prepare_ms":prepare_ms,"decode_merge_encode_ms":repack_ms,"publish_ms":publish_ms,"phases":phases,"verified":true})
        );
    }

    #[test]
    #[ignore = "requires isolated crash fixture paths and expected revision"]
    fn verify_recorded_recovery() {
        let source = std::path::PathBuf::from(std::env::var("INK_PROFILE_SOURCE").unwrap());
        let path = std::path::PathBuf::from(std::env::var("INK_PROFILE_DEST").unwrap());
        for p in [&source, &path] {
            assert!(p.is_absolute());
            assert!(
                !p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            );
            assert!(
                p.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("ink-compaction-")
            );
        }
        // The fixture is closed and immutable. Ordinary read-only WAL access
        // can still create sidecars, preventing a subsequent isolated copy.
        for suffix in ["-wal", "-shm"] {
            assert!(!std::path::PathBuf::from(format!("{}{suffix}", source.display())).exists());
        }
        let source = Connection::open_with_flags(
            format!("file:{}?immutable=1", source.display()),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .unwrap();
        let mut recovered = open(&path).unwrap(); // SQLite recovers the interrupted WAL.
        assert_eq!(
            recovered
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(
            !recovered
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
        let expected_roots = history::roots(&source).unwrap();
        let actual_roots = history::roots(&recovered).unwrap();
        assert_eq!(expected_roots.len(), actual_roots.len());
        for ((a_seq, a_id), (b_seq, b_id)) in expected_roots.iter().zip(&actual_roots) {
            assert_eq!(a_seq, b_seq);
            let a = load_root(&source, get_record(&source, *a_id, None).unwrap()).unwrap();
            let b = load_root(&recovered, get_record(&recovered, *b_id, None).unwrap()).unwrap();
            equal(&to_draft(&a).unwrap(), &to_draft(&b).unwrap());
        }
        let tx = recovered.transaction().unwrap();
        let (snapshot, revision) = load(&tx).unwrap().unwrap();
        assert_eq!(revision, std::env::var("INK_EXPECTED_REVISION").unwrap());
        assert_eq!(
            history::cursor(&tx).unwrap(),
            actual_roots.last().unwrap().0
        );
        assert_eq!(snapshot.root.id, actual_roots.last().unwrap().1);
        println!(
            "INK_RECOVERY {}",
            serde_json::json!({"history_states":actual_roots.len(),"strokes":to_draft(&snapshot).unwrap().strokes.len(),"verified":true})
        );
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
    fn new_edits_survive_prepared_packs_and_publication_failure_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let mut draft = sheet();
        let rev = write_draft(&path, draft.clone(), None).unwrap();
        let plan = prepare(&path).unwrap().unwrap();
        let (chunks, locations) = repack(&plan).unwrap().unwrap();
        let fresh = sheet().strokes;
        draft.strokes.extend(fresh.clone());
        let new = write_patch(
            &path,
            InkDraftPatch {
                order: draft.strokes.iter().map(|s| s.id).collect(),
                upserts: fresh,
                background: InkBackground::Plain,
            },
            Some(rev),
        )
        .unwrap();
        let conn = open(&path).unwrap();
        let roots = history::roots(&conn).unwrap();
        drop(conn);
        assert!(publish(&path, plan, chunks, locations).unwrap());
        assert_eq!(read(&path).unwrap().revision, Some(new.clone()));
        equal(&read(&path).unwrap().draft, &draft);
        let conn = open(&path).unwrap();
        let updated_roots = history::roots(&conn).unwrap();
        assert_eq!(
            updated_roots.iter().map(|r| r.0).collect::<Vec<_>>(),
            roots.iter().map(|r| r.0).collect::<Vec<_>>()
        );
        let roots = updated_roots;
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
    fn a_second_prepared_pack_cannot_replace_already_sealed_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        write_draft(&path, sheet(), None).unwrap();
        let stale = prepare(&path).unwrap().unwrap();
        let (chunks, locations) = repack(&stale).unwrap().unwrap();
        assert!(compact(&path).unwrap());
        let before = rows(&open(&path).unwrap(), "ink_chunks");
        assert!(!publish(&path, stale, chunks, locations).unwrap());
        assert_eq!(rows(&open(&path).unwrap(), "ink_chunks"), before);
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
    fn history_only_strokes_survive_and_late_gc_failure_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let original = sheet();
        let first = write_draft(&path, original.clone(), None).unwrap();
        let remaining = original.strokes.last().unwrap().clone();
        let revision = write_patch(
            &path,
            InkDraftPatch {
                order: vec![remaining.id],
                upserts: vec![],
                background: InkBackground::Grid,
            },
            Some(first),
        )
        .unwrap();
        let before = read(&path).unwrap();
        let conn = open(&path).unwrap();
        let records = rows(&conn, "ink_records");
        let chunks = rows(&conn, "ink_chunks");
        let roots = history::roots(&conn).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_gc BEFORE DELETE ON ink_chunks BEGIN SELECT RAISE(ABORT,'injected late GC failure'); END;").unwrap();
        drop(conn);
        assert!(compact(&path).is_err());
        let conn = open(&path).unwrap();
        assert_eq!(rows(&conn, "ink_records"), records);
        assert_eq!(rows(&conn, "ink_chunks"), chunks);
        assert_eq!(history::roots(&conn).unwrap(), roots);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM ink_sealed_chunks", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        conn.execute_batch("DROP TRIGGER fail_gc").unwrap();
        drop(conn);
        assert_eq!(read(&path).unwrap().revision, Some(revision.clone()));
        assert!(compact(&path).unwrap());
        equal(&read(&path).unwrap().draft, &before.draft);
        let undo = history::navigate(&path, Some(false), Some(revision)).unwrap();
        equal(&undo.snapshot.draft, &original);
        let redo = history::navigate(&path, Some(true), undo.snapshot.revision).unwrap();
        equal(&redo.snapshot.draft, &before.draft);
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

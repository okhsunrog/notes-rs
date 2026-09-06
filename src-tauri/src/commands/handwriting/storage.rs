//! Device-local SQLite adapter. The portable codec knows nothing about this database.
use super::*;
use ink_format::{
    DType, Encoding, Id, cbor,
    chunk::{Chunk, Column, f64_column, f64_values},
    model::{self, Body},
    record::{ObjectRef, Record},
    snapshot::{Snapshot, blob_ref},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};
const APP_ID: i64 = 0x494e4b31;
const MAX_SNAPSHOT_BYTES: i64 = 64 * 1024 * 1024;
fn open(path: &Path) -> CommandResult<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let conn = Connection::open(path).map_err(err)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(err)?;
    let app: i64 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(err)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(err)?;
    if app != 0 && app != APP_ID || version > 1 {
        return Err(CommandError::invalid("Unsupported handwriting database"));
    }
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS ink_records(id BLOB PRIMARY KEY CHECK(length(id)=16), data BLOB NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS ink_chunks(id BLOB PRIMARY KEY CHECK(length(id)=16), data BLOB NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS ink_head(singleton INTEGER PRIMARY KEY CHECK(singleton=1), root_id BLOB NOT NULL REFERENCES ink_records(id), revision TEXT NOT NULL);
        PRAGMA application_id=1229867825; PRAGMA user_version=1;").map_err(err)?;
    Ok(conn)
}
fn get_record(conn: &Connection, id: Id, expected_length: Option<u64>) -> CommandResult<Record> {
    let bytes: Vec<u8> = conn
        .query_row(
            "SELECT data FROM ink_records WHERE id=?1 AND length(data)<=16777216 AND (?2 IS NULL OR length(data)=?2)",
            params![id.as_slice(), expected_length.map(|n| n as i64)],
            |r| r.get(0),
        )
        .map_err(err)?;
    let r = Record::decode(&bytes).map_err(err)?;
    if r.id != id {
        return Err(CommandError::invalid("Ink record identity mismatch"));
    }
    Ok(r)
}
fn load(conn: &Connection) -> CommandResult<Option<(Snapshot, String)>> {
    let head: Option<(Vec<u8>, String)> = conn
        .query_row(
            "SELECT root_id,revision FROM ink_head WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(err)?;
    let Some((id, revision)) = head else {
        return Ok(None);
    };
    let root = get_record(
        conn,
        id.try_into()
            .map_err(|_| CommandError::invalid("Invalid root ID"))?,
        None,
    )?;
    if format!("{:x}", Sha256::digest(root.encode().map_err(err)?)) != revision {
        return Err(CommandError::invalid("Ink root checksum mismatch"));
    }
    let doc = model::Document::read(&root).map_err(err)?;
    let total = doc
        .records
        .values()
        .chain(doc.chunks.values())
        .try_fold(0u64, |n, b| n.checked_add(b.length))
        .ok_or_else(|| CommandError::invalid("Ink size overflow"))?;
    if total > MAX_SNAPSHOT_BYTES as u64
        || doc.records.len() > MAX_POINTS * 3 + 16
        || doc.chunks.len() > MAX_POINTS
    {
        return Err(CommandError::invalid("Ink document is too large"));
    }
    let mut records = BTreeMap::new();
    for (id, entry) in &doc.records {
        records.insert(*id, get_record(conn, *id, Some(entry.length))?);
    }
    let mut chunks = BTreeMap::new();
    for (id, entry) in &doc.chunks {
        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT data FROM ink_chunks WHERE id=?1 AND length(data)=?2",
                params![id.as_slice(), entry.length as i64],
                |r| r.get(0),
            )
            .map_err(err)?;
        chunks.insert(*id, bytes);
    }
    Ok(Some((
        Snapshot {
            root,
            records,
            chunks,
            resources: BTreeMap::new(),
        },
        revision,
    )))
}
fn seal<T: Body>(value: &T) -> CommandResult<Record> {
    let mut hash = Sha256::new();
    hash.update(T::KIND.to_le_bytes());
    hash.update(cbor::encode(&value.to_value().map_err(err)?).map_err(err)?);
    let digest = hash.finalize();
    let mut id = [0; 16];
    id.copy_from_slice(&digest[..16]);
    if id == [0; 16] {
        id[0] = 1;
    }
    let mut record = value.record(id).map_err(err)?;
    record.required_features = vec![1, 2, 3, 4];
    Ok(record)
}
fn profile() -> CommandResult<Record> {
    let axes = [
        (1, "document-unit", 0),
        (2, "document-unit", 0),
        (3, "normalized", 2),
        (5, "degree", 0),
        (6, "degree", 0),
        (11, "ms", 0),
        (12, "kind", 3),
    ]
    .into_iter()
    .map(|(id, unit, representation)| {
        (
            id,
            model::Axis {
                unit: unit.into(),
                representation,
                convention: "Tangleaf normalized input; per-sample origin unavailable".into(),
                min: None,
                max: None,
                extra: BTreeMap::new(),
            },
        )
    })
    .collect();
    seal(&model::SourceProfile {
        axes,
        provenance: cbor::map([
            (0, cbor::text("tangleaf-normalized-v1")),
            (2, cbor::text("portable InkPoint IPC")),
            (3, cbor::text("f64 after input normalization/editing")),
            (4, cbor::num(0)),
            (5, cbor::text("adapter-unspecified")),
        ]),
        tick_ns: None,
        extra: BTreeMap::new(),
    })
}
fn brush() -> CommandResult<Record> {
    seal(&model::Brush {
        contract: "tangleaf-segment-v1".into(),
        version: 1,
        parameters: cbor::map([]),
        extra: BTreeMap::new(),
    })
}
fn paper(background: &InkBackground) -> model::Background {
    let mut b = model::Background::plain([255; 4]);
    if matches!(background, InkBackground::Grid) {
        b.grid = Some(model::Grid {
            step: [25., 25.],
            origin: [0., 0.],
            line_width: 1.,
            line_rgba: [119, 119, 119, 255],
        });
    }
    b
}
fn new_stroke(
    stroke: &InkStroke,
    records: &mut BTreeMap<Id, Record>,
    chunks: &mut BTreeMap<Id, Vec<u8>>,
    segments: &mut BTreeMap<Id, model::SegmentRef>,
) -> CommandResult<ObjectRef> {
    let profile = profile()?;
    let brush = brush()?;
    let segment = *uuid::Uuid::now_v7().as_bytes();
    let chunk_id = *uuid::Uuid::now_v7().as_bytes();
    let mut columns = Vec::new();
    for (axis, read) in [(1, 0), (2, 1), (3, 2), (5, 3), (6, 4), (11, 5)] {
        columns.push(f64_column(
            axis,
            stroke
                .points
                .iter()
                .map(|p| [p.x, p.y, p.pressure, p.tilt_x, p.tilt_y, p.time][read]),
        ));
    }
    columns.push(Column {
        semantic: 12,
        dtype: DType::U8,
        required: true,
        validity: None,
        values: vec![2; stroke.points.len()],
    });
    let chunk = Chunk {
        id: chunk_id,
        profile: profile.id,
        segments: vec![(segment, stroke.points.len() as u32)],
        columns,
    };
    let bytes = chunk.encode(Encoding::Pco8).map_err(err)?;
    let geometry = seal(&model::Geometry {
        profile: profile.id,
        timing: None,
        point_count: stroke.points.len() as u32,
        parts: vec![model::Part {
            segment,
            first_sample: 0,
            count: stroke.points.len() as u32,
            first_t_tick: 0,
        }],
        bounds: None,
        edit_provenance: None,
        extra: BTreeMap::new(),
    })?;
    let version = seal(&model::Stroke {
        id: *stroke.id.as_bytes(),
        geometry: geometry.id,
        brush: brush.id,
        rgba: [17, 17, 17, 255],
        width: stroke.width,
        transform: [1., 0., 0., 1., 0., 0.],
        created_at: None,
        updated_at: None,
        extra: BTreeMap::new(),
    })?;
    let reference = ObjectRef {
        object_id: *stroke.id.as_bytes(),
        record_id: version.id,
    };
    for record in [profile, brush, geometry, version] {
        records.insert(record.id, record);
    }
    chunks.insert(chunk_id, bytes);
    segments.insert(
        segment,
        model::SegmentRef {
            chunk: chunk_id,
            index: 0,
        },
    );
    Ok(reference)
}
fn page_of(snapshot: &Snapshot) -> CommandResult<model::Page> {
    let doc = model::Document::read(&snapshot.root).map_err(err)?;
    if doc.pages.len() != 1 {
        return Err(CommandError::invalid("Expected one handwriting page"));
    }
    model::Page::read(
        snapshot
            .records
            .get(&doc.pages[0].record_id)
            .ok_or_else(|| CommandError::invalid("Missing page"))?,
    )
    .map_err(err)
}
// This scratch-sheet editor cannot losslessly edit arbitrary portable documents yet.
// Reject unsupported metadata instead of replacing it with our defaults on the next save.
fn validate_adapter(snapshot: &Snapshot) -> CommandResult<()> {
    let doc = model::Document::read(&snapshot.root).map_err(err)?;
    let page = page_of(snapshot)?;
    if !doc.extra.is_empty()
        || !page.extra.is_empty()
        || page.width != 1000.
        || page.height != 1400.
        || (page.background != paper(&InkBackground::Plain)
            && page.background != paper(&InkBackground::Grid))
    {
        return Err(CommandError::invalid("Unsupported sheet metadata"));
    }
    let expected_profile = profile()?.body;
    let expected_brush = brush()?.body;
    for r in snapshot
        .records
        .values()
        .chain(std::iter::once(&snapshot.root))
    {
        if r.extensions != cbor::map([]) || !r.resources.is_empty() {
            return Err(CommandError::invalid("Unsupported ink extensions"));
        }
        match r.kind {
            ink_format::record::kind::SOURCE_PROFILE if r.body != expected_profile => {
                return Err(CommandError::invalid("Unsupported source profile"));
            }
            ink_format::record::kind::BRUSH if r.body != expected_brush => {
                return Err(CommandError::invalid("Unsupported brush"));
            }
            ink_format::record::kind::STROKE => {
                let stroke = model::Stroke::read(r).map_err(err)?;
                if !stroke.extra.is_empty()
                    || stroke.created_at.is_some()
                    || stroke.updated_at.is_some()
                    || stroke.transform != [1., 0., 0., 1., 0., 0.]
                    || stroke.rgba != [17, 17, 17, 255]
                {
                    return Err(CommandError::invalid("Unsupported stroke metadata"));
                }
            }
            ink_format::record::kind::GEOMETRY => {
                let g = model::Geometry::read(r).map_err(err)?;
                if !g.extra.is_empty() || g.edit_provenance.is_some() || g.timing.is_some() {
                    return Err(CommandError::invalid("Unsupported geometry metadata"));
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn to_draft(snapshot: &Snapshot) -> CommandResult<InkDraft> {
    snapshot.validate(&[1, 2, 3, 4]).map_err(err)?;
    validate_adapter(snapshot)?;
    let doc = model::Document::read(&snapshot.root).map_err(err)?;
    let page = page_of(snapshot)?;
    if page.width != 1000. || page.height != 1400. {
        return Err(CommandError::invalid("Unsupported sheet dimensions"));
    }
    let background = if page.background == paper(&InkBackground::Plain) {
        InkBackground::Plain
    } else if page.background == paper(&InkBackground::Grid) {
        InkBackground::Grid
    } else {
        return Err(CommandError::invalid("Unsupported paper style"));
    };
    let decoded: BTreeMap<_, _> = snapshot
        .chunks
        .iter()
        .map(|(id, bytes)| Ok((*id, Chunk::decode(bytes).map_err(err)?)))
        .collect::<CommandResult<_>>()?;
    let mut strokes = Vec::new();
    for reference in page.objects {
        let stroke = model::Stroke::read(&snapshot.records[&reference.record_id]).map_err(err)?;
        let b = model::Brush::read(&snapshot.records[&stroke.brush]).map_err(err)?;
        if b.contract != "tangleaf-segment-v1"
            || b.version != 1
            || stroke.transform != [1., 0., 0., 1., 0., 0.]
            || stroke.rgba != [17, 17, 17, 255]
        {
            return Err(CommandError::invalid("Unsupported ink rendering profile"));
        }
        let geometry = model::Geometry::read(&snapshot.records[&stroke.geometry]).map_err(err)?;
        let mut points = Vec::new();
        for part in geometry.parts {
            let location = &doc.segments[&part.segment];
            let chunk = &decoded[&location.chunk];
            let first = chunk.segments[..location.index as usize]
                .iter()
                .map(|(_, n)| *n as usize)
                .sum::<usize>();
            let values: Vec<_> = [1, 2, 3, 5, 6, 11]
                .into_iter()
                .map(|axis| {
                    f64_values(
                        chunk
                            .columns
                            .iter()
                            .find(|c| c.semantic == axis)
                            .ok_or_else(|| CommandError::invalid("Missing normalized axis"))?,
                    )
                    .map_err(err)
                })
                .collect::<CommandResult<_>>()?;
            for (i, x) in values[0]
                .iter()
                .enumerate()
                .skip(first)
                .take(part.count as usize)
            {
                points.push(InkPoint {
                    x: *x,
                    y: values[1][i],
                    pressure: values[2][i],
                    tilt_x: values[3][i],
                    tilt_y: values[4][i],
                    time: values[5][i],
                });
            }
        }
        strokes.push(InkStroke {
            id: uuid::Uuid::from_bytes(stroke.id),
            width: stroke.width,
            points,
        });
    }
    let draft = InkDraft {
        strokes,
        background,
        ..InkDraft::default()
    };
    validate(&draft)?;
    Ok(draft)
}
fn build(current: Option<&Snapshot>, patch: InkDraftPatch) -> CommandResult<Snapshot> {
    let (document_id, page_id, mut records, mut chunks, mut segments, previous) =
        if let Some(s) = current {
            let doc = model::Document::read(&s.root).map_err(err)?;
            let page = page_of(s)?;
            (
                doc.id,
                page.id,
                s.records.clone(),
                s.chunks.clone(),
                doc.segments,
                page.objects,
            )
        } else {
            (
                *uuid::Uuid::now_v7().as_bytes(),
                *uuid::Uuid::now_v7().as_bytes(),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                vec![],
            )
        };
    let order: BTreeSet<_> = patch.order.iter().copied().collect();
    if order.len() != patch.order.len() || patch.order.len() > MAX_POINTS {
        return Err(CommandError::invalid("Invalid handwriting order"));
    }
    let mut versions: BTreeMap<_, _> = previous
        .into_iter()
        .map(|r| (uuid::Uuid::from_bytes(r.object_id), r))
        .collect();
    let mut upserted = BTreeSet::new();
    validate(&InkDraft {
        strokes: patch.upserts.clone(),
        ..InkDraft::default()
    })?;
    for stroke in patch.upserts {
        if !order.contains(&stroke.id) || !upserted.insert(stroke.id) {
            return Err(CommandError::invalid("Invalid handwriting upsert"));
        }
        versions.insert(
            stroke.id,
            new_stroke(&stroke, &mut records, &mut chunks, &mut segments)?,
        );
    }
    let objects = patch
        .order
        .into_iter()
        .map(|id| {
            versions
                .remove(&id)
                .ok_or_else(|| CommandError::invalid("Unknown handwriting stroke"))
        })
        .collect::<CommandResult<_>>()?;
    let page = seal(&model::Page {
        id: page_id,
        width: 1000.,
        height: 1400.,
        objects,
        background: paper(&patch.background),
        recognition: vec![],
        corrections: vec![],
        recognition_selection: BTreeMap::new(),
        extra: BTreeMap::new(),
    })?;
    let page_ref = ObjectRef {
        object_id: page_id,
        record_id: page.id,
    };
    records.insert(page.id, page);
    let mut reachable = BTreeSet::new();
    let mut todo = vec![page_ref.record_id];
    while let Some(id) = todo.pop() {
        if reachable.insert(id) {
            todo.extend(
                records
                    .get(&id)
                    .ok_or_else(|| CommandError::invalid("Missing ink dependency"))?
                    .dependencies
                    .iter()
                    .copied(),
            );
        }
    }
    records.retain(|id, _| reachable.contains(id));
    let mut used_segments = BTreeSet::new();
    let mut points = 0usize;
    for r in records
        .values()
        .filter(|r| r.kind == ink_format::record::kind::GEOMETRY)
    {
        let g = model::Geometry::read(r).map_err(err)?;
        points += g.point_count as usize;
        used_segments.extend(g.parts.iter().map(|p| p.segment));
    }
    if points > MAX_POINTS {
        return Err(CommandError::invalid("Handwriting point limit"));
    }
    segments.retain(|id, _| used_segments.contains(id));
    let used_chunks: BTreeSet<_> = segments.values().map(|r| r.chunk).collect();
    chunks.retain(|id, _| used_chunks.contains(id));
    let doc = model::Document {
        id: document_id,
        pages: vec![page_ref],
        chunks: chunks.iter().map(|(id, b)| (*id, blob_ref(b))).collect(),
        segments,
        records: records
            .iter()
            .map(|(id, r)| Ok((*id, blob_ref(&r.encode().map_err(err)?))))
            .collect::<CommandResult<_>>()?,
        resources: BTreeMap::new(),
        extra: BTreeMap::new(),
    };
    let total = doc
        .records
        .values()
        .chain(doc.chunks.values())
        .try_fold(0u64, |n, blob| n.checked_add(blob.length))
        .ok_or_else(|| CommandError::invalid("Ink size overflow"))?;
    if total > MAX_SNAPSHOT_BYTES as u64 {
        return Err(CommandError::invalid("Ink document is too large"));
    }
    let s = Snapshot {
        root: seal(&doc)?,
        records,
        chunks,
        resources: BTreeMap::new(),
    };
    s.validate(&[1, 2, 3, 4]).map_err(err)?;
    Ok(s)
}
fn put(conn: &Connection, table: &str, id: Id, bytes: &[u8]) -> CommandResult<()> {
    let existing: Option<Vec<u8>> = conn
        .query_row(
            &format!("SELECT data FROM {table} WHERE id=?1"),
            [id.as_slice()],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    if let Some(existing) = existing {
        if existing != bytes {
            return Err(CommandError::conflict("Ink ID collision"));
        }
    } else {
        conn.execute(
            &format!("INSERT INTO {table}(id,data) VALUES(?1,?2)"),
            params![id.as_slice(), bytes],
        )
        .map_err(err)?;
    }
    Ok(())
}
pub(super) fn read(path: &Path) -> CommandResult<InkDraftSnapshot> {
    let mut conn = open(path)?;
    let tx = conn.transaction().map_err(err)?;
    let result = match load(&tx)? {
        Some((s, revision)) => InkDraftSnapshot {
            draft: to_draft(&s)?,
            revision: Some(revision),
        },
        None => InkDraftSnapshot {
            draft: InkDraft::default(),
            revision: None,
        },
    };
    tx.commit().map_err(err)?;
    Ok(result)
}
pub(super) fn patch(
    path: &Path,
    patch: InkDraftPatch,
    expected: Option<String>,
) -> CommandResult<String> {
    let mut conn = open(path)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    let current = load(&tx)?;
    if current.as_ref().map(|(_, r)| r) != expected.as_ref() {
        return Err(CommandError::conflict(
            "The handwriting draft changed. Reopen it before saving.",
        ));
    }
    if let Some((s, _)) = &current {
        s.validate(&[1, 2, 3, 4]).map_err(err)?;
        validate_adapter(s)?;
    }
    let snapshot = build(current.as_ref().map(|(s, _)| s), patch)?;
    for (id, bytes) in &snapshot.chunks {
        put(&tx, "ink_chunks", *id, bytes)?;
    }
    for (id, r) in &snapshot.records {
        put(&tx, "ink_records", *id, &r.encode().map_err(err)?)?;
    }
    let bytes = snapshot.root.encode().map_err(err)?;
    put(&tx, "ink_records", snapshot.root.id, &bytes)?;
    let revision = format!("{:x}", Sha256::digest(&bytes));
    tx.execute("INSERT INTO ink_head VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET root_id=excluded.root_id,revision=excluded.revision",params![snapshot.root.id.as_slice(),revision]).map_err(err)?;
    // No durable history yet: keep current content only. Frontend Undo resubmits old strokes.
    // Future history/export pins must extend these keep sets before enabling those features.
    tx.execute_batch("CREATE TEMP TABLE keep_records(id BLOB PRIMARY KEY) WITHOUT ROWID; CREATE TEMP TABLE keep_chunks(id BLOB PRIMARY KEY) WITHOUT ROWID;").map_err(err)?;
    for id in snapshot
        .records
        .keys()
        .chain(std::iter::once(&snapshot.root.id))
    {
        tx.execute("INSERT INTO keep_records VALUES(?1)", [id.as_slice()])
            .map_err(err)?;
    }
    for id in snapshot.chunks.keys() {
        tx.execute("INSERT INTO keep_chunks VALUES(?1)", [id.as_slice()])
            .map_err(err)?;
    }
    tx.execute_batch("DELETE FROM ink_records WHERE id NOT IN (SELECT id FROM keep_records); DELETE FROM ink_chunks WHERE id NOT IN (SELECT id FROM keep_chunks);").map_err(err)?;
    tx.commit().map_err(err)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(err)?;
    }
    Ok(revision)
}

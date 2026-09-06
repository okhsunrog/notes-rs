//! Validation of a complete immutable core-ink snapshot before publication/use.
use crate::{
    Error, Id, Result,
    chunk::Chunk,
    ensure,
    model::{self, Body},
    record::{Record, kind},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Encoded metadata/chunks belonging to one root. Persistence is intentionally external.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub root: Record,
    pub records: BTreeMap<Id, Record>,
    pub chunks: BTreeMap<Id, Vec<u8>>,
    pub resources: BTreeMap<[u8; 32], Vec<u8>>,
}
impl Snapshot {
    /// Validate reference closure, IDs, hashes, body schemas and column/profile relationships.
    /// Recognition/image/text object interpretation is not yet supported by this core validator.
    pub fn validate(&self, supported_features: &[u64]) -> Result<()> {
        self.root.encode()?;
        self.root.check_features(supported_features)?;
        let doc = model::Document::read(&self.root)?;
        ensure(
            !self.records.contains_key(&self.root.id),
            "root self catalog",
        )?;
        ensure(
            doc.records.len() == self.records.len()
                && doc.chunks.len() == self.chunks.len()
                && doc.resources.len() == self.resources.len(),
            "catalog size mismatch",
        )?;
        for (id, r) in &self.records {
            ensure(*id == r.id, "record ID mismatch")?;
            r.check_features(supported_features)?;
            let entry = doc.records.get(id).ok_or("uncatalogued record")?;
            let bytes = r.encode()?;
            check_blob(&bytes, entry)?;
        }
        for (hash, bytes) in &self.resources {
            ensure(
                doc.resources.get(hash) == Some(&(bytes.len() as u64))
                    && Sha256::digest(bytes).as_slice() == hash,
                "resource integrity",
            )?;
        }
        let mut decoded = BTreeMap::new();
        for (id, bytes) in &self.chunks {
            check_blob(bytes, doc.chunks.get(id).ok_or("uncatalogued chunk")?)?;
            let chunk = Chunk::decode(bytes)?;
            ensure(chunk.id == *id, "chunk ID mismatch")?;
            decoded.insert(*id, chunk);
        }
        self.validate_metadata(&doc)?;
        let mut used_segments = BTreeSet::new();
        for r in self.records.values().filter(|r| r.kind == kind::GEOMETRY) {
            let geometry = model::Geometry::read(r)?;
            let profile = model::SourceProfile::read(self.get(geometry.profile)?)?;
            let mut prior_tick = None;
            for part in &geometry.parts {
                used_segments.insert(part.segment);
                let location = doc
                    .segments
                    .get(&part.segment)
                    .ok_or("missing segment mapping")?;
                let chunk = decoded
                    .get(&location.chunk)
                    .ok_or("missing segment chunk")?;
                ensure(
                    chunk.profile == geometry.profile,
                    "geometry/source profile mismatch",
                )?;
                ensure(
                    chunk.segments.get(location.index as usize)
                        == Some(&(part.segment, part.count)),
                    "segment identity/count mismatch",
                )?;
                ensure(
                    chunk.columns.len() == profile.axes.len()
                        && chunk
                            .columns
                            .iter()
                            .all(|c| profile.axes.contains_key(&c.semantic)),
                    "source channel mismatch",
                )?;
                for column in &chunk.columns {
                    use crate::DType::*;
                    let allowed = match column.semantic {
                        1 | 2 | 8 | 9 | 10 => matches!(column.dtype, F32 | F64),
                        3 => matches!(column.dtype, U16 | U32 | F32 | F64),
                        4 => matches!(column.dtype, U32 | U64),
                        5 | 6 => matches!(column.dtype, I8 | I16 | F32 | F64),
                        7 => matches!(column.dtype, U16 | F32 | F64),
                        11 => matches!(column.dtype, I64 | F64),
                        12 => column.dtype == U8,
                        _ => !column.required,
                    };
                    ensure(allowed, "unsupported channel dtype/semantic")?;
                }
                let start = chunk.segments[..location.index as usize]
                    .iter()
                    .map(|(_, n)| *n as usize)
                    .sum::<usize>();
                let dt = chunk.columns.iter().find(|c| c.semantic == 4);
                ensure(
                    dt.is_some() == geometry.timing.is_some(),
                    "geometry timing/DT mismatch",
                )?;
                if let Some(dt) = dt {
                    ensure(
                        matches!(dt.dtype, crate::DType::U32 | crate::DType::U64),
                        "DT dtype",
                    )?;
                    let mut tick = part.first_t_tick;
                    for i in start..start + part.count as usize {
                        let b = &dt.values[i * dt.dtype.width()..(i + 1) * dt.dtype.width()];
                        let delta = if dt.dtype == crate::DType::U32 {
                            u32::from_le_bytes(b.try_into()?).into()
                        } else {
                            u64::from_le_bytes(b.try_into()?)
                        };
                        if i == start {
                            ensure(delta == 0, "segment-first DT")?;
                        } else {
                            tick = tick.checked_add(delta).ok_or("tick overflow")?;
                        }
                        if let Some(prev) = prior_tick {
                            ensure(tick >= prev, "nonmonotonic segment anchors")?;
                        }
                        prior_tick = Some(tick);
                        geometry
                            .timing
                            .as_ref()
                            .unwrap()
                            .start_tick
                            .checked_add(tick)
                            .ok_or("session tick overflow")?;
                    }
                } else {
                    ensure(part.first_t_tick == 0, "timing-free anchor")?;
                }
            }
        }
        ensure(
            used_segments.len() == doc.segments.len(),
            "unreachable segment mappings",
        )?;
        let used_chunks: BTreeSet<_> = doc.segments.values().map(|r| r.chunk).collect();
        ensure(used_chunks.len() == doc.chunks.len(), "unreachable chunks")?;
        Ok(())
    }
    fn get(&self, id: Id) -> Result<&Record> {
        self.records
            .get(&id)
            .ok_or_else(|| "missing record reference".into())
    }
    fn validate_metadata(&self, doc: &model::Document) -> Result<()> {
        let mut visited = BTreeSet::new();
        let mut active = BTreeSet::new();
        fn visit(
            s: &Snapshot,
            id: Id,
            visited: &mut BTreeSet<Id>,
            active: &mut BTreeSet<Id>,
            depth: usize,
        ) -> Result<()> {
            if visited.contains(&id) {
                return Ok(());
            }
            ensure(depth <= 64 && active.insert(id), "record cycle/depth")?;
            let r = s.get(id)?;
            for hash in &r.resources {
                ensure(s.resources.contains_key(hash), "missing resource")?;
            }
            match r.kind {
                kind::PAGE => {
                    let page = model::Page::read(r)?;
                    ensure(
                        page.recognition.is_empty() && page.corrections.is_empty(),
                        "recognition validation not supported",
                    )?;
                    for reference in page.objects {
                        let stroke = model::Stroke::read(s.get(reference.record_id)?)?;
                        ensure(reference.object_id == stroke.id, "object identity mismatch")?;
                    }
                }
                kind::STROKE => {
                    let stroke = model::Stroke::read(r)?;
                    model::Geometry::read(s.get(stroke.geometry)?)?;
                    model::Brush::read(s.get(stroke.brush)?)?;
                }
                kind::GEOMETRY => {
                    let geometry = model::Geometry::read(r)?;
                    let profile = model::SourceProfile::read(s.get(geometry.profile)?)?;
                    if let Some(t) = geometry.timing {
                        let session = model::Session::read(s.get(t.session)?)?;
                        ensure(
                            profile.tick_ns == Some(session.tick_ns),
                            "session/profile tick mismatch",
                        )?;
                    }
                }
                kind::SOURCE_PROFILE => {
                    model::SourceProfile::read(r)?;
                }
                kind::BRUSH => {
                    model::Brush::read(r)?;
                }
                kind::SESSION => {
                    model::Session::read(r)?;
                }
                k => return Err(Error::Unsupported("snapshot record kind", k)),
            }
            for child in &r.dependencies {
                visit(s, *child, visited, active, depth + 1)?;
            }
            active.remove(&id);
            visited.insert(id);
            Ok(())
        }
        for reference in &doc.pages {
            let p = model::Page::read(self.get(reference.record_id)?)?;
            ensure(p.id == reference.object_id, "page identity mismatch")?;
        }
        for id in &self.root.dependencies {
            visit(self, *id, &mut visited, &mut active, 0)?;
        }
        ensure(
            visited.len() == self.records.len(),
            "unreachable record catalog",
        )?;
        let referenced_resources: BTreeSet<_> = self
            .records
            .values()
            .chain(std::iter::once(&self.root))
            .flat_map(|r| r.resources.iter().copied())
            .collect();
        ensure(
            referenced_resources.len() == self.resources.len()
                && referenced_resources
                    .iter()
                    .all(|id| self.resources.contains_key(id)),
            "resource closure",
        )?;
        Ok(())
    }
}
pub fn blob_ref(bytes: &[u8]) -> model::BlobRef {
    model::BlobRef {
        hash: Sha256::digest(bytes).into(),
        length: bytes.len() as u64,
    }
}
fn check_blob(bytes: &[u8], expected: &model::BlobRef) -> Result<()> {
    ensure(
        expected.length == bytes.len() as u64
            && expected.hash == <[u8; 32]>::from(Sha256::digest(bytes)),
        "catalog integrity",
    )
}

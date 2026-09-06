//! Typed core metadata. Storage, rendering and application policies remain external.
use crate::{
    Error, Id, Result,
    cbor::{self, Value},
    ensure,
    record::{ObjectRef, Record, kind},
};
use std::collections::{BTreeMap, BTreeSet};

pub type ExtraFields = BTreeMap<u64, Value>;
pub trait Body: Sized {
    const KIND: u64;
    fn to_value(&self) -> Result<Value>;
    fn from_value(value: &Value) -> Result<Self>;
    fn dependencies(&self) -> Vec<Id> {
        vec![]
    }
    fn record(&self, id: Id) -> Result<Record> {
        let mut r = Record::new(Self::KIND, id, self.to_value()?);
        r.dependencies = self.dependencies();
        r.dependencies.sort_unstable();
        r.dependencies.dedup();
        r.required_features = vec![1];
        r.encode()?;
        Ok(r)
    }
    fn read(record: &Record) -> Result<Self> {
        ensure(record.kind == Self::KIND, "record kind mismatch")?;
        let body = Self::from_value(&record.body)?;
        for id in body.dependencies() {
            ensure(
                record.dependencies.contains(&id),
                "undeclared strong reference",
            )?;
        }
        Ok(body)
    }
}
fn field(v: &Value, k: u64) -> Result<&Value> {
    cbor::field(v, k)
}
fn number(v: &Value, k: u64) -> Result<u64> {
    cbor::u64_value(field(v, k)?)
}
fn id(v: &Value, k: u64) -> Result<Id> {
    cbor::id(field(v, k)?)
}
fn float(v: &Value, k: u64) -> Result<f64> {
    let f = cbor::f64_value(field(v, k)?)?;
    ensure(f.is_finite(), "nonfinite metadata")?;
    Ok(f)
}
fn string(v: &Value, k: u64) -> Result<String> {
    Ok(field(v, k)?.as_text().ok_or("expected text")?.to_owned())
}
fn ids(v: &Value, k: u64) -> Result<Vec<Id>> {
    cbor::items(field(v, k)?)?.iter().map(cbor::id).collect()
}
fn refs(v: &Value, k: u64) -> Result<Vec<ObjectRef>> {
    cbor::items(field(v, k)?)?
        .iter()
        .map(ObjectRef::from_value)
        .collect()
}
fn bytes<const N: usize>(v: &Value) -> Result<[u8; N]> {
    Ok(v.as_bytes()
        .ok_or("expected bytes")?
        .as_slice()
        .try_into()?)
}
fn floats<const N: usize>(v: &Value) -> Result<[f64; N]> {
    let a = cbor::items(v)?;
    ensure(a.len() == N, "float array length")?;
    let mut out = [0.; N];
    for (i, v) in a.iter().enumerate() {
        out[i] = cbor::f64_value(v)?;
        ensure(out[i].is_finite(), "nonfinite metadata")?;
    }
    Ok(out)
}
fn optional(v: &Value, k: u64) -> Option<&Value> {
    v.as_map()?
        .iter()
        .find(|(key, _)| key == &cbor::num(k))
        .map(|(_, value)| value)
}
fn extras(v: &Value, known: &[u64]) -> Result<ExtraFields> {
    let mut out = BTreeMap::new();
    for (k, x) in v.as_map().ok_or("expected map")? {
        let k = cbor::u64_value(k)?;
        if !known.contains(&k) {
            if k < 0x8000 {
                return Err(Error::Unsupported("body field", k));
            }
            out.insert(k, x.clone());
        }
    }
    Ok(out)
}
fn body(fields: impl IntoIterator<Item = (u64, Value)>, extra: &ExtraFields) -> Result<Value> {
    let mut fields: BTreeMap<_, _> = fields.into_iter().collect();
    for (k, v) in extra {
        ensure(
            *k >= 0x8000 && !fields.contains_key(k),
            "invalid extra field",
        )?;
        fields.insert(*k, v.clone());
    }
    Ok(cbor::map(fields))
}
fn refs_value(refs: &[ObjectRef]) -> Result<Value> {
    Ok(cbor::array(
        refs.iter()
            .map(|r| r.to_value())
            .collect::<Result<Vec<_>>>()?,
    ))
}
fn ids_value(ids: &[Id]) -> Value {
    cbor::array(ids.iter().map(cbor::bytes))
}
fn farray(values: impl IntoIterator<Item = f64>) -> Value {
    cbor::array(values.into_iter().map(cbor::float))
}
fn positive(v: f64) -> Result<()> {
    ensure(v.is_finite() && v > 0., "expected positive finite value")
}
fn unique(ids: impl IntoIterator<Item = Id>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for id in ids {
        ensure(id != [0; 16] && seen.insert(id), "invalid/duplicate ID")?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub step: [f64; 2],
    pub origin: [f64; 2],
    pub line_width: f64,
    pub line_rgba: [u8; 4],
}
#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    pub rgba: [u8; 4],
    pub grid: Option<Grid>,
    pub extra: ExtraFields,
}
impl Background {
    pub fn plain(rgba: [u8; 4]) -> Self {
        Self {
            rgba,
            grid: None,
            extra: BTreeMap::new(),
        }
    }
    pub fn to_value(&self) -> Result<Value> {
        ensure(self.rgba[3] == 255, "background must be opaque")?;
        let mut f = vec![
            (0, cbor::num(u64::from(self.grid.is_some()))),
            (1, cbor::bytes(self.rgba)),
        ];
        if let Some(g) = &self.grid {
            positive(g.step[0])?;
            positive(g.step[1])?;
            positive(g.line_width)?;
            ensure(g.origin.iter().all(|v| v.is_finite()), "grid origin")?;
            f.extend([
                (2, cbor::float(g.step[0])),
                (3, cbor::float(g.step[1])),
                (4, farray(g.origin)),
                (5, cbor::float(g.line_width)),
                (6, cbor::bytes(g.line_rgba)),
            ]);
        }
        body(f, &self.extra)
    }
    pub fn from_value(v: &Value) -> Result<Self> {
        let grid = match number(v, 0)? {
            0 => {
                ensure(
                    (2..=6).all(|k| optional(v, k).is_none()),
                    "grid fields on plain background",
                )?;
                None
            }
            1 => Some(Grid {
                step: [float(v, 2)?, float(v, 3)?],
                origin: floats(field(v, 4)?)?,
                line_width: float(v, 5)?,
                line_rgba: bytes(field(v, 6)?)?,
            }),
            k => return Err(Error::Unsupported("background", k)),
        };
        let result = Self {
            rgba: bytes(field(v, 1)?)?,
            grid,
            extra: extras(v, &[0, 1, 2, 3, 4, 5, 6])?,
        };
        result.to_value()?;
        Ok(result)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub id: Id,
    pub width: f64,
    pub height: f64,
    pub objects: Vec<ObjectRef>,
    pub background: Background,
    pub recognition: Vec<Id>,
    pub corrections: Vec<Id>,
    pub recognition_selection: BTreeMap<[u8; 32], Id>,
    pub extra: ExtraFields,
}
impl Body for Page {
    const KIND: u64 = kind::PAGE;
    fn dependencies(&self) -> Vec<Id> {
        self.objects
            .iter()
            .map(|r| r.record_id)
            .chain(self.recognition.iter().copied())
            .chain(self.corrections.iter().copied())
            .collect()
    }
    fn to_value(&self) -> Result<Value> {
        positive(self.width)?;
        positive(self.height)?;
        unique([self.id])?;
        unique(self.objects.iter().map(|r| r.object_id))?;
        unique(self.recognition.iter().copied())?;
        unique(self.corrections.iter().copied())?;
        ensure(
            self.recognition_selection
                .values()
                .all(|id| self.recognition.contains(id)),
            "recognition selection reference",
        )?;
        body(
            [
                (0, cbor::bytes(self.id)),
                (1, cbor::float(self.width)),
                (2, cbor::float(self.height)),
                (3, refs_value(&self.objects)?),
                (4, self.background.to_value()?),
                (5, ids_value(&self.recognition)),
                (6, ids_value(&self.corrections)),
                (
                    7,
                    Value::Map(
                        self.recognition_selection
                            .iter()
                            .map(|(k, v)| (cbor::bytes(k), cbor::bytes(v)))
                            .collect(),
                    ),
                ),
            ],
            &self.extra,
        )
    }
    fn from_value(v: &Value) -> Result<Self> {
        let result = Self {
            id: id(v, 0)?,
            width: float(v, 1)?,
            height: float(v, 2)?,
            objects: refs(v, 3)?,
            background: Background::from_value(field(v, 4)?)?,
            recognition: ids(v, 5)?,
            corrections: ids(v, 6)?,
            recognition_selection: field(v, 7)?
                .as_map()
                .ok_or("selection map")?
                .iter()
                .map(|(k, v)| Ok((bytes(k)?, cbor::id(v)?)))
                .collect::<Result<_>>()?,
            extra: extras(v, &[0, 1, 2, 3, 4, 5, 6, 7])?,
        };
        result.to_value()?;
        Ok(result)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub id: Id,
    pub geometry: Id,
    pub brush: Id,
    pub rgba: [u8; 4],
    pub width: f64,
    pub transform: [f64; 6],
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub extra: ExtraFields,
}
impl Body for Stroke {
    const KIND: u64 = kind::STROKE;
    fn dependencies(&self) -> Vec<Id> {
        vec![self.geometry, self.brush]
    }
    fn to_value(&self) -> Result<Value> {
        positive(self.width)?;
        for id in [self.id, self.geometry, self.brush] {
            unique([id])?;
        }
        ensure(
            self.transform.iter().all(|v| v.is_finite()),
            "transform values",
        )?;
        let [a, b, c, d, _, _] = self.transform;
        let determinant = a * d - b * c;
        ensure(
            determinant.is_finite() && determinant != 0.,
            "singular/unstable transform",
        )?;
        let mut f = vec![
            (0, cbor::bytes(self.id)),
            (1, cbor::bytes(self.geometry)),
            (2, cbor::bytes(self.brush)),
            (3, cbor::bytes(self.rgba)),
            (4, cbor::float(self.width)),
            (5, farray(self.transform)),
        ];
        if let Some(t) = self.created_at {
            f.push((6, Value::Integer(t.into())));
        }
        if let Some(t) = self.updated_at {
            f.push((7, Value::Integer(t.into())));
        }
        body(f, &self.extra)
    }
    fn from_value(v: &Value) -> Result<Self> {
        let time = |k| -> Result<Option<i64>> {
            optional(v, k)
                .map(|v| Ok(v.as_integer().ok_or("timestamp integer")?.try_into()?))
                .transpose()
        };
        let result = Self {
            id: id(v, 0)?,
            geometry: id(v, 1)?,
            brush: id(v, 2)?,
            rgba: bytes(field(v, 3)?)?,
            width: float(v, 4)?,
            transform: floats(field(v, 5)?)?,
            created_at: time(6)?,
            updated_at: time(7)?,
            extra: extras(v, &[0, 1, 2, 3, 4, 5, 6, 7])?,
        };
        result.to_value()?;
        Ok(result)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub segment: Id,
    pub first_sample: u32,
    pub count: u32,
    pub first_t_tick: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    pub session: Id,
    pub start_tick: u64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub profile: Id,
    pub timing: Option<Timing>,
    pub point_count: u32,
    pub parts: Vec<Part>,
    pub bounds: Option<[f64; 4]>,
    pub edit_provenance: Option<Value>,
    pub extra: ExtraFields,
}
impl Body for Geometry {
    const KIND: u64 = kind::GEOMETRY;
    fn dependencies(&self) -> Vec<Id> {
        std::iter::once(self.profile)
            .chain(self.timing.iter().map(|t| t.session))
            .collect()
    }
    fn to_value(&self) -> Result<Value> {
        unique([self.profile])?;
        ensure(
            self.point_count > 0 && !self.parts.is_empty(),
            "empty geometry",
        )?;
        let mut n = 0u32;
        for p in &self.parts {
            unique([p.segment])?;
            ensure(p.count > 0 && p.first_sample == n, "geometry part coverage")?;
            n = n.checked_add(p.count).ok_or("geometry count overflow")?;
        }
        ensure(n == self.point_count, "geometry count")?;
        let timing = if let Some(t) = &self.timing {
            unique([t.session])?;
            cbor::map([(0, cbor::bytes(t.session)), (1, cbor::num(t.start_tick))])
        } else {
            Value::Null
        };
        let mut f = vec![
            (0, cbor::bytes(self.profile)),
            (1, timing),
            (2, cbor::num(self.point_count.into())),
            (
                3,
                cbor::array(self.parts.iter().map(|p| {
                    cbor::map([
                        (0, cbor::bytes(p.segment)),
                        (1, cbor::num(p.first_sample.into())),
                        (2, cbor::num(p.count.into())),
                        (3, cbor::num(p.first_t_tick)),
                    ])
                })),
            ),
            (4, cbor::num(0)),
        ];
        if let Some(b) = self.bounds {
            ensure(
                b.iter().all(|v| v.is_finite()) && b[0] <= b[2] && b[1] <= b[3],
                "geometry bounds",
            )?;
            f.push((5, farray(b)));
        }
        if let Some(v) = &self.edit_provenance {
            ids(v, 0)?;
            string(v, 1)?;
            f.push((6, v.clone()));
        }
        body(f, &self.extra)
    }
    fn from_value(v: &Value) -> Result<Self> {
        ensure(number(v, 4)? == 0, "unsupported geometry accuracy")?;
        let timing = match field(v, 1)? {
            Value::Null => None,
            t => {
                ensure(
                    extras(t, &[0, 1])?.is_empty(),
                    "unsupported timing extension",
                )?;
                Some(Timing {
                    session: id(t, 0)?,
                    start_tick: number(t, 1)?,
                })
            }
        };
        let result = Self {
            profile: id(v, 0)?,
            timing,
            point_count: number(v, 2)?.try_into()?,
            parts: cbor::items(field(v, 3)?)?
                .iter()
                .map(|p| {
                    ensure(
                        extras(p, &[0, 1, 2, 3])?.is_empty(),
                        "unsupported part extension",
                    )?;
                    Ok(Part {
                        segment: id(p, 0)?,
                        first_sample: number(p, 1)?.try_into()?,
                        count: number(p, 2)?.try_into()?,
                        first_t_tick: number(p, 3)?,
                    })
                })
                .collect::<Result<_>>()?,
            bounds: optional(v, 5).map(floats).transpose()?,
            edit_provenance: optional(v, 6).cloned(),
            extra: extras(v, &[0, 1, 2, 3, 4, 5, 6])?,
        };
        result.to_value()?;
        Ok(result)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub unit: String,
    pub representation: u64,
    pub convention: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub extra: ExtraFields,
}
impl Axis {
    fn to_value(&self) -> Result<Value> {
        ensure(self.representation <= 3, "axis representation")?;
        let mut f = vec![
            (0, cbor::text(&self.unit)),
            (1, cbor::num(self.representation)),
            (2, cbor::text(&self.convention)),
        ];
        if let Some(x) = self.min {
            ensure(x.is_finite(), "axis minimum")?;
            f.push((3, cbor::float(x)));
        }
        if let Some(x) = self.max {
            ensure(x.is_finite(), "axis maximum")?;
            f.push((4, cbor::float(x)));
        }
        if let (Some(a), Some(b)) = (self.min, self.max) {
            ensure(a <= b, "axis range")?;
        }
        body(f, &self.extra)
    }
    fn from_value(v: &Value) -> Result<Self> {
        let a = Self {
            unit: string(v, 0)?,
            representation: number(v, 1)?,
            convention: string(v, 2)?,
            min: optional(v, 3).map(cbor::f64_value).transpose()?,
            max: optional(v, 4).map(cbor::f64_value).transpose()?,
            extra: extras(v, &[0, 1, 2, 3, 4])?,
        };
        a.to_value()?;
        Ok(a)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct SourceProfile {
    pub axes: BTreeMap<u16, Axis>,
    pub provenance: Value,
    pub tick_ns: Option<u64>,
    pub extra: ExtraFields,
}
impl Body for SourceProfile {
    const KIND: u64 = kind::SOURCE_PROFILE;
    fn to_value(&self) -> Result<Value> {
        for id in [1, 2, 12] {
            ensure(self.axes.contains_key(&id), "missing source axis")?;
        }
        ensure(!self.axes.contains_key(&0), "zero axis")?;
        string(&self.provenance, 0)?;
        ensure(self.tick_ns != Some(0), "zero tick")?;
        ensure(
            self.axes.contains_key(&4) == self.tick_ns.is_some(),
            "DT/tick mismatch",
        )?;
        if let Some(p) = self.axes.get(&3) {
            match p.representation {
                0 | 1 => {
                    ensure(
                        matches!((p.min,p.max),(Some(a),Some(b)) if b>a),
                        "pressure range required",
                    )?;
                }
                2 => {}
                _ => return Err("pressure representation".into()),
            }
        }
        body(
            [
                (
                    0,
                    Value::Map(
                        self.axes
                            .iter()
                            .map(|(k, v)| Ok((cbor::num((*k).into()), v.to_value()?)))
                            .collect::<Result<_>>()?,
                    ),
                ),
                (1, self.provenance.clone()),
                (2, self.tick_ns.map(cbor::num).unwrap_or(Value::Null)),
            ],
            &self.extra,
        )
    }
    fn from_value(v: &Value) -> Result<Self> {
        let result = Self {
            axes: field(v, 0)?
                .as_map()
                .ok_or("axes map")?
                .iter()
                .map(|(k, v)| Ok((cbor::u64_value(k)?.try_into()?, Axis::from_value(v)?)))
                .collect::<Result<_>>()?,
            provenance: field(v, 1)?.clone(),
            tick_ns: match field(v, 2)? {
                Value::Null => None,
                v => Some(cbor::u64_value(v)?),
            },
            extra: extras(v, &[0, 1, 2])?,
        };
        result.to_value()?;
        Ok(result)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Brush {
    pub contract: String,
    pub version: u64,
    pub parameters: Value,
    pub extra: ExtraFields,
}
impl Body for Brush {
    const KIND: u64 = kind::BRUSH;
    fn to_value(&self) -> Result<Value> {
        ensure(
            !self.contract.is_empty() && self.version > 0,
            "brush identity",
        )?;
        ensure(self.parameters.as_map().is_some(), "brush parameters")?;
        body(
            [
                (0, cbor::text(&self.contract)),
                (1, cbor::num(self.version)),
                (2, self.parameters.clone()),
            ],
            &self.extra,
        )
    }
    fn from_value(v: &Value) -> Result<Self> {
        let r = Self {
            contract: string(v, 0)?,
            version: number(v, 1)?,
            parameters: field(v, 2)?.clone(),
            extra: extras(v, &[0, 1, 2])?,
        };
        r.to_value()?;
        Ok(r)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    pub hash: [u8; 32],
    pub length: u64,
}
impl BlobRef {
    fn to_value(&self) -> Value {
        cbor::array([cbor::bytes(self.hash), cbor::num(self.length)])
    }
    fn from_value(v: &Value) -> Result<Self> {
        let a = cbor::items(v)?;
        ensure(a.len() == 2, "blob reference")?;
        Ok(Self {
            hash: bytes(&a[0])?,
            length: cbor::u64_value(&a[1])?,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRef {
    pub chunk: Id,
    pub index: u32,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub id: Id,
    pub pages: Vec<ObjectRef>,
    pub chunks: BTreeMap<Id, BlobRef>,
    pub segments: BTreeMap<Id, SegmentRef>,
    pub records: BTreeMap<Id, BlobRef>,
    pub resources: BTreeMap<[u8; 32], u64>,
    pub extra: ExtraFields,
}
impl Body for Document {
    const KIND: u64 = kind::DOCUMENT;
    fn dependencies(&self) -> Vec<Id> {
        self.pages.iter().map(|p| p.record_id).collect()
    }
    fn to_value(&self) -> Result<Value> {
        unique([self.id])?;
        unique(self.pages.iter().map(|r| r.object_id))?;
        for id in self
            .chunks
            .keys()
            .chain(self.segments.keys())
            .chain(self.records.keys())
        {
            unique([*id])?;
        }
        let catalog = |m: &BTreeMap<Id, BlobRef>| {
            Value::Map(
                m.iter()
                    .map(|(k, v)| (cbor::bytes(k), v.to_value()))
                    .collect(),
            )
        };
        body(
            [
                (0, cbor::bytes(self.id)),
                (1, cbor::num(1)),
                (2, cbor::num(0)),
                (3, refs_value(&self.pages)?),
                (4, catalog(&self.chunks)),
                (
                    5,
                    Value::Map(
                        self.segments
                            .iter()
                            .map(|(k, v)| {
                                (
                                    cbor::bytes(k),
                                    cbor::array([cbor::bytes(v.chunk), cbor::num(v.index.into())]),
                                )
                            })
                            .collect(),
                    ),
                ),
                (6, catalog(&self.records)),
                (
                    7,
                    Value::Map(
                        self.resources
                            .iter()
                            .map(|(k, v)| (cbor::bytes(k), cbor::num(*v)))
                            .collect(),
                    ),
                ),
            ],
            &self.extra,
        )
    }
    fn from_value(v: &Value) -> Result<Self> {
        ensure(
            number(v, 1)? == 1 && number(v, 2)? == 0,
            "unsupported document version",
        )?;
        let catalog = |k| -> Result<BTreeMap<Id, BlobRef>> {
            field(v, k)?
                .as_map()
                .ok_or("catalog map")?
                .iter()
                .map(|(k, v)| Ok((cbor::id(k)?, BlobRef::from_value(v)?)))
                .collect()
        };
        let r = Self {
            id: id(v, 0)?,
            pages: refs(v, 3)?,
            chunks: catalog(4)?,
            segments: field(v, 5)?
                .as_map()
                .ok_or("segment map")?
                .iter()
                .map(|(k, v)| {
                    let a = cbor::items(v)?;
                    ensure(a.len() == 2, "segment ref")?;
                    Ok((
                        cbor::id(k)?,
                        SegmentRef {
                            chunk: cbor::id(&a[0])?,
                            index: cbor::u64_value(&a[1])?.try_into()?,
                        },
                    ))
                })
                .collect::<Result<_>>()?,
            records: catalog(6)?,
            resources: field(v, 7)?
                .as_map()
                .ok_or("resources map")?
                .iter()
                .map(|(k, v)| Ok((bytes(k)?, cbor::u64_value(v)?)))
                .collect::<Result<_>>()?,
            extra: extras(v, &[0, 1, 2, 3, 4, 5, 6, 7])?,
        };
        r.to_value()?;
        Ok(r)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub tick_ns: u64,
    pub wall_anchor: Option<Value>,
    pub adapter_contract: String,
    pub extra: ExtraFields,
}
impl Body for Session {
    const KIND: u64 = kind::SESSION;
    fn to_value(&self) -> Result<Value> {
        ensure(
            self.tick_ns > 0 && !self.adapter_contract.is_empty(),
            "session clock",
        )?;
        let mut f = vec![
            (0, cbor::num(self.tick_ns)),
            (1, cbor::num(1)),
            (3, cbor::text(&self.adapter_contract)),
        ];
        if let Some(v) = &self.wall_anchor {
            number(v, 0)?;
            let _: i64 = field(v, 1)?
                .as_integer()
                .ok_or("wall timestamp")?
                .try_into()?;
            if let Some(u) = optional(v, 2) {
                cbor::u64_value(u)?;
            }
            f.push((2, v.clone()));
        }
        body(f, &self.extra)
    }
    fn from_value(v: &Value) -> Result<Self> {
        ensure(number(v, 1)? == 1, "unsupported session clock")?;
        let s = Self {
            tick_ns: number(v, 0)?,
            wall_anchor: optional(v, 2).cloned(),
            adapter_contract: string(v, 3)?,
            extra: extras(v, &[0, 1, 2, 3])?,
        };
        s.to_value()?;
        Ok(s)
    }
}

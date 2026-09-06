//! Immutable metadata envelopes. Body contracts and required features are explicit.
//!
//! Decoding an envelope is not permission to render or edit an unfamiliar body.
//! Applications must check required features and understand the record kind first.
use crate::{
    Error, Id, Result,
    cbor::{self, Value},
    ensure,
};
use sha2::{Digest, Sha256};

/// Identity of an object and identity of one immutable version of that object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectRef {
    pub object_id: Id,
    pub record_id: Id,
}
impl ObjectRef {
    pub fn to_value(self) -> Result<Value> {
        ensure(
            self.object_id != [0; 16] && self.record_id != [0; 16],
            "zero object reference",
        )?;
        Ok(cbor::array([
            cbor::bytes(self.object_id),
            cbor::bytes(self.record_id),
        ]))
    }
    pub fn from_value(value: &Value) -> Result<Self> {
        let items = cbor::items(value)?;
        ensure(items.len() == 2, "object reference length")?;
        let result = Self {
            object_id: cbor::id(&items[0])?,
            record_id: cbor::id(&items[1])?,
        };
        result.to_value()?;
        Ok(result)
    }
}
/// Registered record kinds. Unknown values are retained by [`Record`].
pub mod kind {
    pub const DOCUMENT: u64 = 1;
    pub const PAGE: u64 = 2;
    pub const STROKE: u64 = 3;
    pub const GEOMETRY: u64 = 4;
    pub const SOURCE_PROFILE: u64 = 5;
    pub const SESSION: u64 = 6;
    pub const BRUSH: u64 = 7;
    pub const RECOGNITION: u64 = 8;
    pub const TEXT_CORRECTION: u64 = 9;
}
/// A canonical metadata record. Unknown body/extension values round-trip unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub kind: u64,
    pub id: Id,
    pub body: Value,
    pub required_features: Vec<u64>,
    pub extensions: Value,
    pub dependencies: Vec<Id>,
    pub resources: Vec<[u8; 32]>,
}
impl Record {
    /// Construct an envelope without interpreting its body's application-independent schema.
    pub fn new(kind: u64, id: Id, body: Value) -> Self {
        Self {
            kind,
            id,
            body,
            required_features: vec![],
            extensions: Value::Map(vec![]),
            dependencies: vec![],
            resources: vec![],
        }
    }
    fn validate(&self) -> Result<()> {
        ensure(self.kind > 0 && self.id != [0; 16], "record identity")?;
        for value in [&self.body, &self.extensions] {
            let map = value
                .as_map()
                .ok_or("record body/extensions must be maps")?;
            for (key, _) in map {
                cbor::u64_value(key)?;
            }
        }
        ensure(
            self.required_features.windows(2).all(|w| w[0] < w[1]),
            "feature order/duplicates",
        )?;
        ensure(
            self.dependencies.windows(2).all(|w| w[0] < w[1]),
            "dependency order/duplicates",
        )?;
        ensure(
            self.dependencies
                .iter()
                .all(|id| *id != [0; 16] && *id != self.id),
            "invalid dependency",
        )?;
        ensure(
            self.resources.windows(2).all(|w| w[0] < w[1]),
            "resource order/duplicates",
        )?;
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        cbor::encode(&cbor::map([
            (0, cbor::num(self.kind)),
            (1, cbor::bytes(self.id)),
            (2, self.body.clone()),
            (
                3,
                cbor::array(self.required_features.iter().copied().map(cbor::num)),
            ),
            (4, self.extensions.clone()),
            (5, cbor::array(self.dependencies.iter().map(cbor::bytes))),
            (6, cbor::array(self.resources.iter().map(cbor::bytes))),
        ]))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value = cbor::decode(bytes)?;
        let fields = value.as_map().ok_or("record must be map")?;
        ensure(fields.len() == 7, "unsupported record envelope fields")?;
        let record = Self {
            kind: cbor::u64_value(cbor::field(&value, 0)?)?,
            id: cbor::id(cbor::field(&value, 1)?)?,
            body: cbor::field(&value, 2)?.clone(),
            required_features: cbor::items(cbor::field(&value, 3)?)?
                .iter()
                .map(cbor::u64_value)
                .collect::<Result<_>>()?,
            extensions: cbor::field(&value, 4)?.clone(),
            dependencies: cbor::items(cbor::field(&value, 5)?)?
                .iter()
                .map(cbor::id)
                .collect::<Result<_>>()?,
            resources: cbor::items(cbor::field(&value, 6)?)?
                .iter()
                .map(|v| {
                    Ok(v.as_bytes()
                        .ok_or("resource hash bytes")?
                        .as_slice()
                        .try_into()?)
                })
                .collect::<Result<_>>()?,
        };
        record.validate()?;
        Ok(record)
    }
    pub fn check_features(&self, supported: &[u64]) -> Result<()> {
        for feature in &self.required_features {
            if !supported.contains(feature) {
                return Err(Error::Unsupported("required feature", *feature));
            }
        }
        Ok(())
    }
    /// Hash the canonical encoded record. Persistence and hash catalog management are external.
    pub fn digest(&self) -> Result<[u8; 32]> {
        Ok(Sha256::digest(self.encode()?).into())
    }
}

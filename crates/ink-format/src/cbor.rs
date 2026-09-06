use crate::{Error, Id, Result, ensure};
pub use ciborium::Value;

pub fn map(fields: impl IntoIterator<Item = (u64, Value)>) -> Value {
    Value::Map(fields.into_iter().map(|(k, v)| (num(k), v)).collect())
}
pub fn array(values: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(values.into_iter().collect())
}
pub fn num(v: u64) -> Value {
    Value::Integer(v.into())
}
pub fn bytes(v: impl AsRef<[u8]>) -> Value {
    Value::Bytes(v.as_ref().to_vec())
}
pub fn text(v: &str) -> Value {
    Value::Text(v.to_owned())
}
pub fn float(v: f64) -> Value {
    Value::Float(v)
}

pub fn encode(value: &Value) -> Result<Vec<u8>> {
    fn canonical(value: &Value, depth: usize) -> Result<Value> {
        ensure(depth <= 32, "CBOR nesting")?;
        Ok(match value {
            Value::Map(fields) => {
                let mut sorted = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    let k = canonical(k, depth + 1)?;
                    let mut key = Vec::new();
                    ciborium::ser::into_writer(&k, &mut key)
                        .map_err(|e| Error::Cbor(e.to_string()))?;
                    sorted.push((key, k, canonical(v, depth + 1)?));
                }
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                ensure(
                    sorted.windows(2).all(|w| w[0].0 != w[1].0),
                    "duplicate CBOR key",
                )?;
                Value::Map(sorted.into_iter().map(|(_, k, v)| (k, v)).collect())
            }
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|v| canonical(v, depth + 1))
                    .collect::<Result<_>>()?,
            ),
            Value::Float(f) => {
                ensure(f.is_finite(), "non-finite CBOR float")?;
                value.clone()
            }
            Value::Integer(_) | Value::Bytes(_) | Value::Text(_) | Value::Bool(_) | Value::Null => {
                value.clone()
            }
            _ => return Err("unsupported CBOR value".into()),
        })
    }
    let mut out = Vec::new();
    ciborium::ser::into_writer(&canonical(value, 0)?, &mut out)
        .map_err(|e| Error::Cbor(e.to_string()))?;
    ensure(out.len() <= 16 * 1024 * 1024, "CBOR record limit")?;
    preflight(&out)?;
    Ok(out)
}

pub fn decode(input: &[u8]) -> Result<Value> {
    ensure(input.len() <= 16 * 1024 * 1024, "CBOR record limit")?;
    preflight(input)?;
    let value: Value = ciborium::de::from_reader_with_recursion_limit(input, 32)
        .map_err(|e| Error::Cbor(e.to_string()))?;
    ensure(
        encode(&value)? == input,
        "non-deterministic CBOR/trailing data",
    )?;
    Ok(value)
}

pub fn field(value: &Value, key: u64) -> Result<&Value> {
    value
        .as_map()
        .ok_or("expected map")?
        .iter()
        .find(|(k, _)| k == &num(key))
        .map(|(_, v)| v)
        .ok_or_else(|| "missing CBOR field".into())
}
pub fn items(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "expected array".into())
}
pub fn id(value: &Value) -> Result<Id> {
    let id: Id = value
        .as_bytes()
        .ok_or("expected bytes")?
        .as_slice()
        .try_into()?;
    ensure(id != [0; 16], "zero id")?;
    Ok(id)
}
pub fn u64_value(value: &Value) -> Result<u64> {
    Ok(value.as_integer().ok_or("expected uint")?.try_into()?)
}
pub fn f64_value(value: &Value) -> Result<f64> {
    value.as_float().ok_or_else(|| "expected float".into())
}

// Check lengths and item budget before serde can allocate from untrusted size hints.
fn preflight(input: &[u8]) -> Result<()> {
    fn item(input: &[u8], at: &mut usize, depth: usize, budget: &mut usize) -> Result<()> {
        if depth > 32 || *budget == 0 {
            return Err(Error::ResourceLimit("CBOR depth/items"));
        }
        *budget -= 1;
        let head = *input.get(*at).ok_or("truncated CBOR")?;
        *at += 1;
        let major = head >> 5;
        let add = head & 31;
        let argument = match add {
            0..=23 => u64::from(add),
            24..=27 => {
                let n = 1usize << (add - 24);
                let end = at.checked_add(n).ok_or("CBOR length overflow")?;
                let data = input.get(*at..end).ok_or("truncated CBOR argument")?;
                *at = end;
                let mut b = [0u8; 8];
                b[8 - n..].copy_from_slice(data);
                u64::from_be_bytes(b)
            }
            _ => return Err("indefinite/reserved CBOR".into()),
        };
        match major {
            0 | 1 => {}
            2 | 3 => {
                let n = usize::try_from(argument)?;
                let end = at.checked_add(n).ok_or("CBOR length overflow")?;
                ensure(end <= input.len(), "truncated CBOR string")?;
                *at = end;
            }
            4 | 5 => {
                let count = usize::try_from(argument)?
                    .checked_mul(if major == 5 { 2 } else { 1 })
                    .ok_or("CBOR count overflow")?;
                if count > *budget {
                    return Err(Error::ResourceLimit("CBOR items"));
                }
                ensure(count <= input.len() - *at, "truncated CBOR container")?;
                for _ in 0..count {
                    item(input, at, depth + 1, budget)?;
                }
            }
            7 if matches!(add,20..=22|25..=27) => {}
            _ => return Err("unsupported CBOR value".into()),
        }
        Ok(())
    }
    let mut at = 0;
    item(input, &mut at, 0, &mut 1_000_000)?;
    ensure(at == input.len(), "CBOR trailing bytes")
}

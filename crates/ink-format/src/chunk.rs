//! Immutable, independently encoded column chunks.
use crate::{DType, Encoding, Error, Id, Result, codec, ensure};

pub const MAX_CHUNK: usize = 64 * 1024 * 1024;
const HEADER: usize = 80;

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub semantic: u16,
    pub dtype: DType,
    pub required: bool,
    /// None means all valid; otherwise LSB-first bitmap, including all-null.
    pub validity: Option<Vec<u8>>,
    /// Only valid values, in row order, as little-endian scalars.
    pub values: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub id: Id,
    pub profile: Id,
    pub segments: Vec<(Id, u32)>,
    pub columns: Vec<Column>,
}

fn uint(bytes: &[u8], at: usize, size: usize) -> Result<u64> {
    let end = at.checked_add(size).ok_or("offset overflow")?;
    let src = bytes.get(at..end).ok_or("truncated integer")?;
    let mut le = [0; 8];
    le[..size].copy_from_slice(src);
    Ok(u64::from_le_bytes(le))
}

fn put(bytes: &mut [u8], at: usize, size: usize, value: u64) {
    bytes[at..at + size].copy_from_slice(&value.to_le_bytes()[..size]);
}

fn id(bytes: &[u8], at: usize) -> Result<Id> {
    let value: Id = bytes.get(at..at + 16).ok_or("truncated id")?.try_into()?;
    ensure(value != [0; 16], "zero id")?;
    Ok(value)
}

impl Chunk {
    pub fn count(&self) -> Result<usize> {
        let n = self.segments.iter().try_fold(0_usize, |n, (_, count)| {
            n.checked_add(*count as usize).ok_or("count overflow")
        })?;
        ensure(n > 0 && n <= 1_000_000, "point limit")?;
        Ok(n)
    }

    pub fn encode(&self, encoding: Encoding) -> Result<Vec<u8>> {
        let decoded = self.columns.iter().try_fold(0usize, |n, c| {
            n.checked_add(c.values.len()).ok_or("decoded overflow")
        })?;
        if decoded > MAX_CHUNK {
            return Err(Error::ResourceLimit("decoded column bytes"));
        }
        let n = self.count()?;
        ensure(
            !self.segments.is_empty() && self.segments.len() <= 65536,
            "segment limit",
        )?;
        ensure(
            !self.columns.is_empty() && self.columns.len() <= 64,
            "column limit",
        )?;
        let tables = HEADER + self.segments.len() * 32 + self.columns.len() * 64;
        let mut bytes = vec![0_u8; tables];
        let mut payloads = Vec::with_capacity(self.columns.len());
        bytes[..8].copy_from_slice(b"INKCHNK\0");
        put(&mut bytes, 8, 2, 1);
        bytes[16..32].copy_from_slice(&self.id);
        bytes[32..48].copy_from_slice(&self.profile);
        put(&mut bytes, 48, 4, n as u64);
        put(&mut bytes, 52, 4, self.segments.len() as u64);
        put(&mut bytes, 56, 2, self.columns.len() as u64);
        for (i, (segment, count)) in self.segments.iter().enumerate() {
            let p = HEADER + i * 32;
            bytes[p..p + 16].copy_from_slice(segment);
            put(&mut bytes, p + 16, 4, u64::from(*count));
        }
        for (i, column) in self.columns.iter().enumerate() {
            let p = HEADER + self.segments.len() * 32 + i * 64;
            let (state, valid) = if let Some(bitmap) = &column.validity {
                ensure(bitmap.len() == n.div_ceil(8), "bitmap length")?;
                if !n.is_multiple_of(8) {
                    ensure(bitmap[n / 8] >> (n % 8) == 0, "bitmap tail")?;
                }
                let valid = bitmap
                    .iter()
                    .map(|b| b.count_ones() as usize)
                    .sum::<usize>();
                ensure(valid < n, "all-valid bitmap must be omitted")?;
                if valid == 0 { (2, 0) } else { (1, valid) }
            } else {
                (0, n)
            };
            ensure(
                column.values.len() == valid * column.dtype.width(),
                "column length",
            )?;
            put(&mut bytes, p, 2, u64::from(column.semantic));
            bytes[p + 2] = column.dtype as u8;
            bytes[p + 3] = column.dtype as u8;
            let selected = if valid == 0 { Encoding::Raw } else { encoding };
            put(&mut bytes, p + 4, 2, selected.id().into());
            validate_values(column)?;
            payloads.push(codec::compress(column.dtype, &column.values, selected)?);
            put(&mut bytes, p + 6, 2, 1);
            put(
                &mut bytes,
                p + 10,
                2,
                state | if column.required { 16 } else { 0 },
            );
            put(&mut bytes, p + 12, 4, n as u64);
            put(&mut bytes, p + 28, 4, column.values.len() as u64);
            let offset = bytes.len();
            bytes.extend_from_slice(selected.params());
            put(&mut bytes, p + 32, 4, offset as u64);
            put(&mut bytes, p + 36, 4, selected.params().len() as u64);
            if state == 1 {
                let offset = bytes.len();
                let bitmap = column.validity.as_ref().unwrap();
                bytes.extend_from_slice(bitmap);
                put(&mut bytes, p + 40, 4, offset as u64);
                put(&mut bytes, p + 44, 4, bitmap.len() as u64);
            }
        }
        let payload_start = bytes.len();
        for (i, payload) in payloads.iter().enumerate() {
            let p = HEADER + self.segments.len() * 32 + i * 64;
            if !payload.is_empty() {
                let offset = bytes.len();
                put(&mut bytes, p + 16, 8, offset as u64);
                put(&mut bytes, p + 24, 4, payload.len() as u64);
                put(&mut bytes, p + 48, 4, u64::from(crc32fast::hash(payload)));
                put(&mut bytes, p + 52, 4, payload.len() as u64);
                bytes.extend_from_slice(payload);
            }
        }
        let payload_len = bytes.len() - payload_start;
        put(&mut bytes, 60, 4, (payload_start - HEADER) as u64);
        put(&mut bytes, 64, 8, payload_len as u64);
        let header_crc = crc32fast::hash(&bytes[..72]);
        let directory_crc = crc32fast::hash(&bytes[HEADER..payload_start]);
        put(&mut bytes, 72, 4, header_crc.into());
        put(&mut bytes, 76, 4, directory_crc.into());
        // The decoder checks cross-field invariants, including mandatory columns.
        Self::decode(&bytes)?;
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Self::read_layout(bytes, true).map(|(chunk, _)| chunk)
    }

    /// Inspect the decoded column-value byte count without decompressing values.
    /// Validates framing, directory invariants and payload CRCs. This is a budget
    /// estimate, not full codec/sample validation or a bound on process memory:
    /// decoding also needs metadata, validity bitmaps and codec scratch space.
    pub fn decoded_value_bytes(bytes: &[u8]) -> Result<usize> {
        Self::read_layout(bytes, false).map(|(_, bytes)| bytes)
    }

    fn read_layout(bytes: &[u8], decode_values: bool) -> Result<(Self, usize)> {
        ensure(
            bytes.len() >= HEADER && bytes.len() <= MAX_CHUNK,
            "chunk size",
        )?;
        ensure(&bytes[..8] == b"INKCHNK\0", "chunk magic")?;
        ensure(
            uint(bytes, 8, 2)? == 1 && uint(bytes, 10, 2)? == 0,
            "unsupported chunk version",
        )?;
        ensure(
            uint(bytes, 12, 4)? == 0 && uint(bytes, 58, 2)? == 0,
            "unsupported flags",
        )?;
        ensure(
            uint(bytes, 72, 4)? == u64::from(crc32fast::hash(&bytes[..72])),
            "header CRC",
        )?;
        let n = uint(bytes, 48, 4)? as usize;
        let s = uint(bytes, 52, 4)? as usize;
        let c = uint(bytes, 56, 2)? as usize;
        ensure(
            n > 0 && n <= 1_000_000 && s > 0 && s <= 65536 && c > 0 && c <= 64,
            "chunk counts",
        )?;
        let directory = uint(bytes, 60, 4)? as usize;
        ensure(directory <= 16 * 1024 * 1024, "directory limit")?;
        let start = HEADER + directory;
        let payload = usize::try_from(uint(bytes, 64, 8)?)?;
        ensure(
            start.checked_add(payload) == Some(bytes.len()),
            "chunk length",
        )?;
        let table_end = HEADER + s * 32 + c * 64;
        ensure(table_end <= start, "directory tables")?;
        ensure(
            uint(bytes, 76, 4)? == u64::from(crc32fast::hash(&bytes[HEADER..start])),
            "directory CRC",
        )?;
        let mut segments = Vec::with_capacity(s);
        let mut ids = std::collections::HashSet::new();
        let mut total = 0_usize;
        for i in 0..s {
            let p = HEADER + i * 32;
            let segment = id(bytes, p)?;
            let count = uint(bytes, p + 16, 4)? as u32;
            ensure(count > 0 && ids.insert(segment), "invalid segment")?;
            ensure(
                uint(bytes, p + 20, 4)? == 0 && uint(bytes, p + 24, 8)? == 0,
                "segment flags",
            )?;
            total = total
                .checked_add(count as usize)
                .ok_or("segment count overflow")?;
            segments.push((segment, count));
        }
        ensure(total == n, "segment count sum")?;
        let mut ranges = Vec::new();
        let mut range = |offset: usize, length: usize, low: usize, high: usize| -> Result<&[u8]> {
            if length == 0 {
                ensure(offset == 0, "empty offset")?;
                return Ok(&[]);
            }
            let end = offset.checked_add(length).ok_or("range overflow")?;
            ensure(offset >= low && end <= high, "range bounds")?;
            ranges.push((offset, end));
            Ok(&bytes[offset..end])
        };
        let mut columns = Vec::with_capacity(c);
        let mut previous = 0;
        let mut decoded_total = 0_usize;
        for i in 0..c {
            let p = HEADER + s * 32 + i * 64;
            let semantic = uint(bytes, p, 2)? as u16;
            ensure(semantic > previous, "column order")?;
            previous = semantic;
            let dtype = DType::try_from(bytes[p + 2])?;
            let w = dtype.width();
            ensure(bytes[p + 3] == dtype as u8, "unsupported storage dtype")?;
            let codec_id = uint(bytes, p + 4, 2)?;
            let encoding = match codec_id {
                0 => Encoding::Raw,
                2 => Encoding::Pco8,
                _ => return Err(Error::Unsupported("codec", codec_id)),
            };
            if uint(bytes, p + 6, 2)? != 1 {
                return Err(Error::Unsupported(
                    "codec wire version",
                    uint(bytes, p + 6, 2)?,
                ));
            }
            ensure(uint(bytes, p + 8, 2)? == 0, "unsupported outer compression")?;
            let flags = uint(bytes, p + 10, 2)?;
            ensure(
                flags & !19 == 0 && flags & 3 != 3,
                "unsupported column flags",
            )?;
            ensure(
                uint(bytes, p + 12, 4)? == n as u64 && uint(bytes, p + 56, 8)? == 0,
                "column count/reserved",
            )?;
            let params = range(
                uint(bytes, p + 32, 4)? as usize,
                uint(bytes, p + 36, 4)? as usize,
                table_end,
                start,
            )?;
            ensure(params == encoding.params(), "unsupported codec parameters")?;
            let bitmap = range(
                uint(bytes, p + 40, 4)? as usize,
                uint(bytes, p + 44, 4)? as usize,
                table_end,
                start,
            )?;
            let (validity, valid) = match flags & 3 {
                0 => {
                    ensure(bitmap.is_empty(), "all-valid bitmap")?;
                    (None, n)
                }
                1 => {
                    ensure(bitmap.len() == n.div_ceil(8), "bitmap size")?;
                    if !n.is_multiple_of(8) {
                        ensure(bitmap[n / 8] >> (n % 8) == 0, "bitmap tail")?;
                    }
                    let valid = bitmap
                        .iter()
                        .map(|b| b.count_ones() as usize)
                        .sum::<usize>();
                    ensure(valid > 0 && valid < n, "mixed bitmap count")?;
                    (Some(bitmap.to_vec()), valid)
                }
                2 => {
                    ensure(bitmap.is_empty(), "all-null bitmap")?;
                    (Some(vec![0; n.div_ceil(8)]), 0)
                }
                _ => unreachable!(),
            };
            if [1, 2, 4, 12].contains(&semantic) {
                ensure(flags == 16, "required axis must be all-valid")?;
            }
            let expected = valid * w;
            decoded_total = decoded_total
                .checked_add(expected)
                .ok_or("decoded overflow")?;
            ensure(decoded_total <= MAX_CHUNK, "decoded limit")?;
            ensure(uint(bytes, p + 28, 4)? == expected as u64, "decoded length")?;
            let payload_len = uint(bytes, p + 24, 4)? as usize;
            ensure(
                uint(bytes, p + 52, 4)? == payload_len as u64,
                "codec length",
            )?;
            if valid == 0 {
                ensure(
                    encoding == Encoding::Raw && payload_len == 0,
                    "all-null encoding",
                )?;
            }
            let payload = range(
                usize::try_from(uint(bytes, p + 16, 8)?)?,
                payload_len,
                start,
                bytes.len(),
            )?;
            ensure(
                uint(bytes, p + 48, 4)? == u64::from(crc32fast::hash(payload)),
                "payload CRC",
            )?;
            if encoding == Encoding::Raw {
                ensure(payload_len == expected, "RAW length")?;
            }
            let values = match (decode_values, encoding) {
                (false, _) => Vec::new(),
                (true, Encoding::Raw) => payload.to_vec(),
                (true, Encoding::Pco8) => codec::decompress(dtype, payload, valid)?,
            };
            let column = Column {
                semantic,
                dtype,
                required: flags & 16 != 0,
                validity,
                values,
            };
            if semantic == 12 {
                ensure(dtype == DType::U8, "sample kind must be U8")?;
            }
            if decode_values {
                validate_values(&column)?;
            }
            columns.push(column);
        }
        for axis in [1, 2, 12] {
            ensure(
                columns.iter().any(|col| col.semantic == axis),
                "missing required axis",
            )?;
        }
        ranges.sort_unstable();
        let mut end = table_end;
        for (lo, hi) in ranges {
            ensure(lo >= end, "overlapping ranges")?;
            ensure(bytes[end..lo].iter().all(|b| *b == 0), "nonzero padding")?;
            end = hi;
        }
        ensure(end == bytes.len(), "trailing padding")?;
        Ok((
            Self {
                id: id(bytes, 16)?,
                profile: id(bytes, 32)?,
                segments,
                columns,
            },
            decoded_total,
        ))
    }
}

pub fn f64_column(semantic: u16, values: impl Iterator<Item = f64>) -> Column {
    Column {
        semantic,
        dtype: DType::F64,
        required: semantic == 1 || semantic == 2,
        validity: None,
        values: values.flat_map(f64::to_le_bytes).collect(),
    }
}

pub fn f64_values(column: &Column) -> Result<Vec<f64>> {
    ensure(
        column.dtype == DType::F64 && column.validity.is_none(),
        "expected all-valid F64",
    )?;
    ensure(column.values.len().is_multiple_of(8), "F64 alignment")?;
    column
        .values
        .as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| {
            let value = f64::from_le_bytes(*bytes);
            ensure(value.is_finite(), "non-finite sample")?;
            Ok(value)
        })
        .collect()
}

fn validate_values(column: &Column) -> Result<()> {
    if column.semantic == 12 {
        ensure(column.dtype == DType::U8, "sample kind must be U8")?;
        ensure(
            column.values.iter().all(|v| *v <= 2),
            "unsupported sample kind",
        )?;
    }
    match column.dtype {
        DType::F32 => ensure(
            column
                .values
                .as_chunks::<4>()
                .0
                .iter()
                .all(|b| f32::from_le_bytes(*b).is_finite()),
            "non-finite F32 sample",
        )?,
        DType::F64 => ensure(
            column
                .values
                .as_chunks::<8>()
                .0
                .iter()
                .all(|b| f64::from_le_bytes(*b).is_finite()),
            "non-finite F64 sample",
        )?,
        _ => {}
    }
    Ok(())
}

impl Chunk {
    /// Read exactly one chunk from a stream, bounded before decoding.
    pub fn read_from(reader: impl std::io::Read) -> Result<Self> {
        use std::io::Read;
        let mut bytes = Vec::new();
        reader.take(MAX_CHUNK as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CHUNK {
            return Err(Error::ResourceLimit("encoded chunk bytes"));
        }
        Self::decode(&bytes)
    }
    /// Encode and validate before writing. Atomic publication is the caller's responsibility.
    pub fn write_to(&self, mut writer: impl std::io::Write, encoding: Encoding) -> Result<()> {
        writer.write_all(&self.encode(encoding)?)?;
        Ok(())
    }
}

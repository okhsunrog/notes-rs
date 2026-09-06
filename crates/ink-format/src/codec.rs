//! Lossless codecs for typed scalar columns.
use crate::{Error, Result, ensure};
use pco::{
    ChunkConfig, PagingSpec,
    data_types::Number,
    standalone::{self, DecompressorItem, FileDecompressor},
};

/// The wire representation of a scalar; byte order is always little endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DType {
    U8 = 1,
    I8 = 2,
    U16 = 3,
    I16 = 4,
    U32 = 5,
    I32 = 6,
    U64 = 7,
    I64 = 8,
    F32 = 9,
    F64 = 10,
}
impl DType {
    pub const fn width(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            _ => 8,
        }
    }
}
impl TryFrom<u8> for DType {
    type Error = Error;
    fn try_from(v: u8) -> Result<Self> {
        Ok(match v {
            1 => Self::U8,
            2 => Self::I8,
            3 => Self::U16,
            4 => Self::I16,
            5 => Self::U32,
            6 => Self::I32,
            7 => Self::U64,
            8 => Self::I64,
            9 => Self::F32,
            10 => Self::F64,
            _ => return Err(Error::Unsupported("dtype", v.into())),
        })
    }
}
/// Encoder policy. Decoder compatibility is determined by the wire codec, not this level.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Raw,
    #[default]
    Pco8,
}
impl Encoding {
    pub(crate) fn id(self) -> u16 {
        match self {
            Self::Raw => 0,
            Self::Pco8 => 2,
        }
    }
    pub(crate) fn params(self) -> &'static [u8] {
        match self {
            Self::Raw => &[0xa1, 0, 0xf4],
            Self::Pco8 => &[0xa0],
        }
    }
}
fn compression(e: pco::errors::PcoError) -> Error {
    Error::Compression(e.to_string())
}

pub(crate) fn compress(dtype: DType, values: &[u8], encoding: Encoding) -> Result<Vec<u8>> {
    ensure(
        values.len().is_multiple_of(dtype.width()),
        "scalar alignment",
    )?;
    if encoding == Encoding::Raw {
        return Ok(values.to_vec());
    }
    macro_rules! encode {
        ($t:ty) => {{
            let nums: Vec<$t> = values
                .chunks_exact(size_of::<$t>())
                .map(|b| <$t>::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let config = ChunkConfig::default()
                .with_compression_level(8)
                .with_enable_8_bit(true)
                .with_paging_spec(PagingSpec::EqualPagesUpTo(1 << 18));
            standalone::simple_compress(&nums, &config).map_err(compression)
        }};
    }
    match dtype {
        DType::U8 => encode!(u8),
        DType::I8 => encode!(i8),
        DType::U16 => encode!(u16),
        DType::I16 => encode!(i16),
        DType::U32 => encode!(u32),
        DType::I32 => encode!(i32),
        DType::U64 => encode!(u64),
        DType::I64 => encode!(i64),
        DType::F32 => encode!(f32),
        DType::F64 => encode!(f64),
    }
}

// Do not use simple_decompress: a forged stream count must not allocate an unbounded Vec.
fn decode_numbers<T: Number + Default>(payload: &[u8], count: usize) -> Result<Vec<T>> {
    let (file, mut src) = FileDecompressor::new(payload).map_err(compression)?;
    let mut values = Vec::with_capacity(count);
    loop {
        match file.chunk_decompressor::<T, _>(src).map_err(compression)? {
            DecompressorItem::EndOfData(rest) => {
                ensure(rest.is_empty(), "PCO trailing bytes")?;
                ensure(values.len() == count, "PCO sample count")?;
                return Ok(values);
            }
            DecompressorItem::Chunk(mut chunk) => {
                let end = values
                    .len()
                    .checked_add(chunk.n())
                    .ok_or("PCO count overflow")?;
                ensure(end <= count, "PCO sample count exceeds column")?;
                let start = values.len();
                values.resize_with(end, T::default);
                let progress = chunk.read(&mut values[start..]).map_err(compression)?;
                ensure(
                    progress.finished && progress.n_processed == end - start,
                    "incomplete PCO chunk",
                )?;
                src = chunk.into_src();
            }
        }
    }
}

pub(crate) fn decompress(dtype: DType, payload: &[u8], count: usize) -> Result<Vec<u8>> {
    macro_rules! decode {
        ($t:ty) => {{
            Ok(decode_numbers::<$t>(payload, count)?
                .into_iter()
                .flat_map(<$t>::to_le_bytes)
                .collect())
        }};
    }
    match dtype {
        DType::U8 => decode!(u8),
        DType::I8 => decode!(i8),
        DType::U16 => decode!(u16),
        DType::I16 => decode!(i16),
        DType::U32 => decode!(u32),
        DType::I32 => decode!(i32),
        DType::U64 => decode!(u64),
        DType::I64 => decode!(i64),
        DType::F32 => decode!(f32),
        DType::F64 => decode!(f64),
    }
}

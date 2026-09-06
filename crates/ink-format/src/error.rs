use std::{array::TryFromSliceError, num::TryFromIntError};
/// A format failure that callers can inspect without parsing error text.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid ink data: {0}")]
    InvalidData(&'static str),
    #[error("unsupported {0}: {1}")]
    Unsupported(&'static str, u64),
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(&'static str),
    #[error("CBOR: {0}")]
    Cbor(String),
    #[error("PCO: {0}")]
    Compression(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<&'static str> for Error {
    fn from(value: &'static str) -> Self {
        Self::InvalidData(value)
    }
}
impl From<TryFromSliceError> for Error {
    fn from(_: TryFromSliceError) -> Self {
        Self::InvalidData("scalar or identifier length")
    }
}
impl From<TryFromIntError> for Error {
    fn from(_: TryFromIntError) -> Self {
        Self::InvalidData("integer range")
    }
}

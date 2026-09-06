#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
pub mod cbor;
pub mod chunk;
mod codec;
pub mod record;
pub use codec::{DType, Encoding};
mod error;
pub use error::{Error, Result};
/// An opaque identifier. Applications choose how to generate IDs; zero is invalid.
pub type Id = [u8; 16];
fn ensure(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::InvalidData(message))
    }
}

pub mod model;

pub mod snapshot;

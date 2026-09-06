//! Bounded binary batches for content-addressed ink blobs. JSON carries hashes only.
use notes_core::{BlobHash, CoreError, CoreResult, ink::transfer::Blobs};
use serde::{Deserialize, Serialize};
pub const MAX_HASHES: usize = 1024;
pub const MAX_BATCH_BYTES: usize = 32 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"INKBLOB1";
#[derive(Debug, Serialize, Deserialize)]
pub struct Hashes {
    pub hashes: Vec<BlobHash>,
}
impl Hashes {
    pub fn validate(&self) -> CoreResult<()> {
        if self.hashes.len() > MAX_HASHES {
            return Err(CoreError::invalid("Too many ink hashes"));
        }
        Ok(())
    }
}
pub fn encode(blobs: &Blobs) -> CoreResult<Vec<u8>> {
    let size = blobs
        .values()
        .try_fold(12usize, |n, b| n.checked_add(36)?.checked_add(b.len()))
        .ok_or_else(|| CoreError::invalid("Ink batch overflow"))?;
    if blobs.len() > MAX_HASHES || size > MAX_BATCH_BYTES {
        return Err(CoreError::invalid("Ink batch too large"));
    }
    let mut out = Vec::with_capacity(size);
    out.extend(MAGIC);
    out.extend((blobs.len() as u32).to_le_bytes());
    for (hash, bytes) in blobs {
        out.extend(hash.as_bytes());
        out.extend((bytes.len() as u32).to_le_bytes());
        out.extend(bytes);
    }
    Ok(out)
}
pub fn decode(mut input: &[u8]) -> CoreResult<Blobs> {
    fn take<'a>(input: &mut &'a [u8], n: usize) -> CoreResult<&'a [u8]> {
        if n > input.len() {
            return Err(CoreError::invalid("Truncated ink batch"));
        }
        let (value, rest) = input.split_at(n);
        *input = rest;
        Ok(value)
    }
    if input.len() > MAX_BATCH_BYTES || take(&mut input, 8)? != MAGIC {
        return Err(CoreError::invalid("Invalid ink batch"));
    }
    let count = u32::from_le_bytes(take(&mut input, 4)?.try_into().unwrap()) as usize;
    if count > MAX_HASHES {
        return Err(CoreError::invalid("Too many ink blobs"));
    }
    let mut blobs = Blobs::new();
    for _ in 0..count {
        let hash = BlobHash::from_bytes(take(&mut input, 32)?.try_into().unwrap());
        let size = u32::from_le_bytes(take(&mut input, 4)?.try_into().unwrap()) as usize;
        let bytes = take(&mut input, size)?;
        if BlobHash::digest(bytes) != hash || blobs.insert(hash, bytes.to_vec()).is_some() {
            return Err(CoreError::invalid("Corrupt or duplicate ink blob"));
        }
    }
    if !input.is_empty() {
        return Err(CoreError::invalid("Trailing ink batch data"));
    }
    Ok(blobs)
}
/// Bound both count and byte size; a blob is never split across requests.
pub fn batches(blobs: Blobs) -> CoreResult<Vec<Blobs>> {
    let mut result = vec![];
    let mut batch = Blobs::new();
    let mut size = 12;
    for (hash, bytes) in blobs {
        let added = 36 + bytes.len();
        if added + 12 > MAX_BATCH_BYTES {
            return Err(CoreError::invalid("Ink blob exceeds batch limit"));
        }
        if batch.len() == MAX_HASHES || size + added > MAX_BATCH_BYTES {
            result.push(std::mem::take(&mut batch));
            size = 12;
        }
        size += added;
        batch.insert(hash, bytes);
    }
    if !batch.is_empty() {
        result.push(batch);
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_roundtrip_rejects_corruption_truncation_and_extra_data() {
        let bytes = vec![1, 2, 3];
        let mut blobs = Blobs::new();
        blobs.insert(BlobHash::digest(&bytes), bytes);
        let encoded = encode(&blobs).unwrap();
        assert_eq!(decode(&encoded).unwrap(), blobs);
        for n in 0..encoded.len() {
            assert!(decode(&encoded[..n]).is_err());
        }
        let mut corrupt = encoded.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(decode(&corrupt).is_err());
        let mut extra = encoded;
        extra.push(0);
        assert!(decode(&extra).is_err());
    }
}

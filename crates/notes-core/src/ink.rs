//! Handwriting stored in the workspace database; acquisition and rendering are host concerns.
use crate::{CoreError as CommandError, CoreResult as CommandResult};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
pub mod archive;
mod input;
mod status;
pub use status::{InkNoteStatus, InkVersionInfo, recoverable_notes};
pub mod runtime;
mod storage;
pub mod transfer;
pub(crate) mod versions;
pub use input::*;
use input::{MAX_POINTS, validate};
pub use versions::{Publish, Version};
fn err(error: impl std::fmt::Display) -> CommandError {
    CommandError::InkStorage(error.to_string())
}

/// A document-scoped connection target. `db::open` owns schema migration; opening
/// an ink connection never creates a database or changes its application/version IDs.
#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
    document: uuid::Uuid,
}
impl Store {
    pub fn new(path: impl AsRef<Path>, document: uuid::Uuid) -> Self {
        Self {
            path: path.as_ref().to_owned(),
            document,
        }
    }
    pub fn read(&self) -> CommandResult<InkDraftSnapshot> {
        storage::read(self)
    }
    pub fn patch(&self, patch: InkDraftPatch, expected: Option<String>) -> CommandResult<String> {
        storage::patch(self, patch, expected)
    }
    pub fn history(
        &self,
        redo: Option<bool>,
        expected: Option<String>,
    ) -> CommandResult<InkHistoryUpdate> {
        storage::navigate_update(self, redo, expected)
    }
    /// Benchmark/policy control; byte and chunk limits are validated by the packer.
    pub fn compact_with_limits(
        &self,
        max_chunks: usize,
        max_decoded_bytes: u64,
    ) -> CommandResult<bool> {
        storage::compact_with_limits(self, max_chunks, max_decoded_bytes)
    }
    pub fn compact(&self) -> CommandResult<bool> {
        self.compact_with_limits(512, 16 * 1024 * 1024)
    }
}

#[cfg(test)]
mod adapter_tests;

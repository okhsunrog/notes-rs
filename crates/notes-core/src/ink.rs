//! Handwriting stored in the workspace database; acquisition and rendering are host concerns.
use crate::{CoreError as CommandError, CoreResult as CommandResult};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
mod input;
mod storage;
pub use input::*;
use input::{MAX_POINTS, validate};
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
    pub fn compact(&self) -> CommandResult<bool> {
        storage::compact_with_limits(self, 512, 16 * 1024 * 1024)
    }
}

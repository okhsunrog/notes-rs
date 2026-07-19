#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    SyncConflict(String),
    #[error("sync sequence gap: expected {expected}, received {received}")]
    SyncSequenceGap { expected: u64, received: u64 },
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
}

impl CoreError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn sync_conflict(message: impl Into<String>) -> Self {
        Self::SyncConflict(message.into())
    }

    pub fn sync_sequence_gap(expected: u64, received: u64) -> Self {
        Self::SyncSequenceGap { expected, received }
    }
}

pub type CoreResult<T> = std::result::Result<T, CoreError>;

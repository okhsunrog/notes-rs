use anyhow::{Context, Result, bail};
use notes_core::{BlobHash, Connection};
use rusqlite::OptionalExtension;
use std::path::Path;

#[derive(Clone)]
pub struct BlobOwnership {
    connection: Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed,
    AlreadyOwned,
}

#[derive(Debug)]
pub struct QuotaExceeded {
    pub used: u64,
    pub requested: u64,
    pub limit: u64,
}

impl std::fmt::Display for QuotaExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "blob quota exceeded: {} used + {} requested > {} limit",
            self.used, self.requested, self.limit
        )
    }
}

impl std::error::Error for QuotaExceeded {}

impl BlobOwnership {
    pub async fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).await?;
        connection
            .call(|database| {
                database.pragma_update(None, "journal_mode", "WAL")?;
                database.pragma_update(None, "synchronous", "FULL")?;
                database.execute_batch(
                    "CREATE TABLE IF NOT EXISTS blob_ownership (
                       user_id TEXT NOT NULL,
                       blob_hash BLOB NOT NULL CHECK (length(blob_hash) = 32),
                       size INTEGER NOT NULL CHECK (size >= 0),
                       PRIMARY KEY (user_id, blob_hash)
                     );",
                )?;
                Ok(())
            })
            .await?;
        Ok(Self { connection })
    }

    pub async fn claim(
        &self,
        user_id: &str,
        hash: BlobHash,
        size: u64,
        quota: u64,
    ) -> Result<ClaimOutcome> {
        let user_id = user_id.to_owned();
        self.connection
            .call_domain(move |database| -> Result<ClaimOutcome> {
                let transaction = database.transaction()?;
                let existing = transaction
                    .query_row(
                        "SELECT size FROM blob_ownership
                          WHERE user_id = ?1 AND blob_hash = ?2",
                        rusqlite::params![user_id, hash.as_bytes().as_slice()],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if let Some(existing) = existing {
                    let existing =
                        u64::try_from(existing).context("negative blob ownership size")?;
                    if existing != size {
                        bail!("blob ownership size does not match the uploaded content");
                    }
                    transaction.commit()?;
                    return Ok(ClaimOutcome::AlreadyOwned);
                }
                let used = transaction.query_row(
                    "SELECT COALESCE(SUM(size), 0) FROM blob_ownership WHERE user_id = ?1",
                    [&user_id],
                    |row| row.get::<_, i64>(0),
                )?;
                let used = u64::try_from(used).context("negative blob ownership usage")?;
                if used.checked_add(size).is_none_or(|next| next > quota) {
                    return Err(QuotaExceeded {
                        used,
                        requested: size,
                        limit: quota,
                    }
                    .into());
                }
                let size = i64::try_from(size).context("blob size exceeds SQLite integer")?;
                transaction.execute(
                    "INSERT INTO blob_ownership(user_id, blob_hash, size) VALUES (?1, ?2, ?3)",
                    rusqlite::params![user_id, hash.as_bytes().as_slice(), size],
                )?;
                transaction.commit()?;
                Ok(ClaimOutcome::Claimed)
            })
            .await
    }

    pub async fn release(&self, user_id: &str, hash: BlobHash) -> Result<()> {
        let user_id = user_id.to_owned();
        self.connection
            .call(move |database| {
                database.execute(
                    "DELETE FROM blob_ownership WHERE user_id = ?1 AND blob_hash = ?2",
                    rusqlite::params![user_id, hash.as_bytes().as_slice()],
                )?;
                Ok(())
            })
            .await
    }
}

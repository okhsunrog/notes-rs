use anyhow::{Context, Result, bail};
use notes_core::Connection;
use notes_protocol::SequencedOp;
use rusqlite::OptionalExtension;
use std::path::Path;

#[derive(Clone)]
pub struct Oplog {
    connection: Connection,
}

pub struct AppendOutcome {
    pub operation: SequencedOp,
    pub inserted: bool,
}

impl Oplog {
    pub async fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).await?;
        connection
            .call(|database| {
                database.pragma_update(None, "journal_mode", "WAL")?;
                database.pragma_update(None, "synchronous", "FULL")?;
                database.execute_batch(
                    "CREATE TABLE IF NOT EXISTS oplog (
                       seq INTEGER PRIMARY KEY CHECK (seq > 0),
                       op_id BLOB NOT NULL UNIQUE CHECK (length(op_id) = 16),
                       envelope TEXT NOT NULL,
                       created_at INTEGER NOT NULL
                     );",
                )?;
                Ok(())
            })
            .await?;
        Ok(Self { connection })
    }

    pub async fn append(&self, operation: notes_core::Op) -> Result<AppendOutcome> {
        let envelope = serde_json::to_string(&operation)?;
        self.connection
            .call(move |database| {
                if let Some((seq, existing)) = database
                    .query_row(
                        "SELECT seq, envelope FROM oplog WHERE op_id = ?1",
                        [&operation.op_id],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?
                {
                    let existing = serde_json::from_str(&existing).map_err(json_error)?;
                    return Ok(AppendOutcome {
                        operation: SequencedOp {
                            seq: seq as u64,
                            envelope: existing,
                        },
                        inserted: false,
                    });
                }
                let transaction = database.transaction()?;
                let seq = transaction.query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM oplog",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                transaction.execute(
                    "INSERT INTO oplog(seq, op_id, envelope, created_at)
                     VALUES (?1, ?2, ?3, unixepoch())",
                    rusqlite::params![seq, operation.op_id, envelope],
                )?;
                transaction.commit()?;
                Ok(AppendOutcome {
                    operation: SequencedOp {
                        seq: seq as u64,
                        envelope: operation,
                    },
                    inserted: true,
                })
            })
            .await
    }

    pub async fn remove_tail(&self, operation: &SequencedOp) -> Result<()> {
        let seq = i64::try_from(operation.seq).context("oplog sequence exceeds SQLite integer")?;
        let op_id = operation.envelope.op_id;
        self.connection
            .call(move |database| {
                let latest =
                    database.query_row("SELECT COALESCE(MAX(seq), 0) FROM oplog", [], |row| {
                        row.get::<_, i64>(0)
                    })?;
                if latest != seq {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                database.execute(
                    "DELETE FROM oplog WHERE seq = ?1 AND op_id = ?2",
                    rusqlite::params![seq, op_id],
                )?;
                Ok(())
            })
            .await
            .context("rolling back failed oplog tail")
    }

    pub async fn latest_seq(&self) -> Result<u64> {
        let seq = self
            .connection
            .call(|database| {
                database.query_row("SELECT COALESCE(MAX(seq), 0) FROM oplog", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .await?;
        u64::try_from(seq).context("oplog sequence is negative")
    }

    pub async fn ops_since(&self, since: u64, limit: usize) -> Result<Vec<SequencedOp>> {
        let since = i64::try_from(since).context("oplog cursor exceeds SQLite integer")?;
        let limit = i64::try_from(limit).context("oplog limit exceeds SQLite integer")?;
        self.connection
            .call(move |database| {
                let mut statement = database.prepare(
                    "SELECT seq, envelope FROM oplog
                     WHERE seq > ?1 ORDER BY seq LIMIT ?2",
                )?;
                statement
                    .query_map(rusqlite::params![since, limit], |row| {
                        let seq = row.get::<_, i64>(0)?;
                        let envelope = row.get::<_, String>(1)?;
                        let envelope = serde_json::from_str(&envelope).map_err(json_error)?;
                        Ok(SequencedOp {
                            seq: seq as u64,
                            envelope,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
    }

    pub async fn assert_gapless(&self) -> Result<()> {
        let (count, maximum) = self
            .connection
            .call(|database| {
                database.query_row(
                    "SELECT COUNT(*), COALESCE(MAX(seq), 0) FROM oplog",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
            })
            .await?;
        if count != maximum {
            bail!("oplog contains a sequence gap: {count} rows through seq {maximum}");
        }
        Ok(())
    }
}

fn json_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notes_core::{Hlc, NodeKind, Op, OpKind, operation::FORMAT_VERSION};
    use notes_sync::NodeCreate;

    fn operation(index: u128) -> Op {
        let device_id = uuid::Uuid::from_u128(1);
        Op {
            op_id: uuid::Uuid::from_u128(index + 10),
            device_id,
            hlc: Hlc::new(index as u64, 0, device_id),
            format_version: FORMAT_VERSION,
            kind: OpKind::NodeCreate(NodeCreate {
                uuid: uuid::Uuid::from_u128(index + 100),
                node_kind: NodeKind::Page,
                title: Some(format!("Page {index}")),
                content: String::new(),
                content_json: None,
                parent_uuid: None,
                position: None,
                created_at: index as i64,
            }),
        }
    }

    #[tokio::test]
    async fn persists_gapless_sequence_and_deduplicates() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("oplog.db");
        let log = Oplog::open(&path).await.expect("open oplog");
        let first = log.append(operation(1)).await.expect("append first");
        assert_eq!(first.operation.seq, 1);
        assert!(first.inserted);
        let duplicate = log.append(operation(1)).await.expect("append duplicate");
        assert_eq!(duplicate.operation.seq, 1);
        assert!(!duplicate.inserted);
        drop(log);

        let reopened = Oplog::open(&path).await.expect("reopen oplog");
        let second = reopened.append(operation(2)).await.expect("append second");
        assert_eq!(second.operation.seq, 2);
        reopened.assert_gapless().await.expect("gapless oplog");
    }
}

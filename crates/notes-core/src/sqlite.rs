use anyhow::{Context, Result, anyhow};
use std::path::Path;
use std::sync::{Arc, mpsc};

type Job = Box<dyn FnOnce(&mut rusqlite::Connection) + Send + 'static>;

enum WorkerMessage {
    Call(Job),
    Close,
}

struct Worker {
    sender: mpsc::Sender<WorkerMessage>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.send(WorkerMessage::Close);
    }
}

/// Cloneable asynchronous handle backed by one dedicated SQLite thread.
///
/// Calls wait in the connection's channel instead of occupying Tokio blocking
/// pool threads while another query owns the connection. Closures may return
/// either `rusqlite::Error` or a typed domain error; both remain available in
/// the returned `anyhow::Error` chain.
#[derive(Clone)]
pub struct Connection {
    worker: Arc<Worker>,
}

impl Connection {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let (sender, receiver) = mpsc::channel::<WorkerMessage>();
        let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("notes-rs-sqlite".into())
            .spawn(move || match rusqlite::Connection::open(path) {
                Ok(mut connection) => {
                    if opened_tx.send(Ok(())).is_err() {
                        return;
                    }
                    while let Ok(message) = receiver.recv() {
                        match message {
                            WorkerMessage::Call(job) => job(&mut connection),
                            WorkerMessage::Close => return,
                        }
                    }
                }
                Err(error) => {
                    let _ = opened_tx.send(Err(error));
                }
            })
            .context("spawning SQLite worker thread")?;
        opened_rx
            .await
            .context("SQLite worker stopped while opening")??;
        Ok(Self {
            worker: Arc::new(Worker { sender }),
        })
    }

    pub async fn call<F, R>(&self, function: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        self.call_domain(function).await
    }

    pub async fn call_domain<F, R, E>(&self, function: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> std::result::Result<R, E> + Send + 'static,
        R: Send + 'static,
        E: Into<anyhow::Error> + Send + 'static,
    {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        self.worker
            .sender
            .send(WorkerMessage::Call(Box::new(move |connection| {
                let _ = result_tx.send(function(connection).map_err(Into::into));
            })))
            .map_err(|_| anyhow!("SQLite connection worker has stopped"))?;
        result_rx
            .await
            .context("SQLite connection worker dropped a call")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("typed domain failure")]
    struct DomainFailure;

    #[tokio::test]
    async fn cloned_handles_serialize_database_calls() {
        let file = tempfile::NamedTempFile::new().expect("temporary database");
        let connection = Connection::open(file.path()).await.expect("open database");
        connection
            .call(|db| {
                db.execute("CREATE TABLE values_table (value INTEGER NOT NULL)", [])?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .expect("create table");

        let writes = (0..8).map(|value| {
            let connection = connection.clone();
            tokio::spawn(async move {
                connection
                    .call(move |db| {
                        db.execute("INSERT INTO values_table (value) VALUES (?1)", [value])?;
                        Ok::<_, rusqlite::Error>(())
                    })
                    .await
            })
        });
        for write in writes {
            write.await.expect("join write task").expect("insert value");
        }

        let count: i64 = connection
            .call(|db| db.query_row("SELECT COUNT(*) FROM values_table", [], |row| row.get(0)))
            .await
            .expect("count rows");
        assert_eq!(count, 8);
    }

    #[tokio::test]
    async fn preserves_typed_errors_from_database_calls() {
        let file = tempfile::NamedTempFile::new().expect("temporary database");
        let connection = Connection::open(file.path()).await.expect("open database");
        let error = connection
            .call_domain(|_| Err::<(), _>(DomainFailure))
            .await
            .expect_err("domain failure");
        assert!(error.downcast_ref::<DomainFailure>().is_some());
    }
}

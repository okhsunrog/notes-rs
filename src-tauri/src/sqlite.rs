use anyhow::{Context, Result, anyhow};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Cloneable asynchronous handle around one serialized SQLite connection.
///
/// rusqlite is synchronous by design, so database work runs on Tokio's
/// blocking pool. The mutex preserves SQLite connection serialization while
/// allowing the handle to be shared by Tauri commands and background workers.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Mutex<rusqlite::Connection>>,
}

impl Connection {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let connection = tokio::task::spawn_blocking(move || rusqlite::Connection::open(path))
            .await
            .context("SQLite open task failed")??;
        Ok(Self {
            inner: Arc::new(Mutex::new(connection)),
        })
    }

    pub async fn call<F, R>(&self, function: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .lock()
                .map_err(|_| anyhow!("SQLite connection mutex poisoned"))?;
            function(&mut connection).map_err(Into::into)
        })
        .await
        .context("SQLite worker task failed")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cloned_handles_serialize_database_calls() {
        let connection = Connection {
            inner: Arc::new(Mutex::new(
                rusqlite::Connection::open_in_memory().expect("open in-memory database"),
            )),
        };
        connection
            .call(|db| {
                db.execute("CREATE TABLE values_table (value INTEGER NOT NULL)", [])?;
                Ok(())
            })
            .await
            .expect("create table");

        let writes = (0..8).map(|value| {
            let connection = connection.clone();
            tokio::spawn(async move {
                connection
                    .call(move |db| {
                        db.execute("INSERT INTO values_table (value) VALUES (?1)", [value])?;
                        Ok(())
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
}

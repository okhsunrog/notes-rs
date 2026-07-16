use crate::sqlite::Connection;
use anyhow::{Context, Result};
use rusqlite_migration::{M, Migrations};

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(include_str!("migrations/V001__initial.sql"))])
}

pub(super) async fn migrate(connection: &Connection, ndims: usize) -> Result<()> {
    connection
        .call(|database| {
            migrations()
                .to_latest(database)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
        })
        .await
        .context("applying embedded database migrations")?;
    ensure_vector_table(connection, ndims).await
}

/// sqlite-vec requires the vector width in DDL, so this is deliberately the
/// only schema construction that remains dynamic Rust code.
async fn ensure_vector_table(connection: &Connection, ndims: usize) -> Result<()> {
    connection
        .call(move |database| {
            database.execute_batch(&format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS vec_nodes USING vec0(embedding float[{ndims}]);"
            ))
        })
        .await
        .context("creating sqlite-vec table")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_migration_is_valid() {
        migrations().validate().expect("valid migrations");
    }
}

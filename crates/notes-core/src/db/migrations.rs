use crate::sqlite::Connection;
use anyhow::{Context, Result};
use rusqlite_migration::{M, Migrations};

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(include_str!("migrations/V001__initial.sql"))])
}

pub(super) async fn migrate(connection: &Connection) -> Result<()> {
    connection
        .call(|database| {
            migrations()
                .to_latest(database)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
        })
        .await
        .context("applying embedded database migrations")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_migration_is_valid() {
        migrations().validate().expect("valid migrations");
    }
}

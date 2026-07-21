use crate::sqlite::Connection;
use anyhow::{Context, Result};
use rusqlite_migration::{M, Migrations};

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("migrations/V001__initial.sql")),
        M::up(include_str!("migrations/V002__stem_page_titles.sql")),
        M::up(include_str!("migrations/V003__attachment_image_cache.sql")),
    ])
}

pub(super) async fn migrate(connection: &Connection) -> Result<()> {
    connection
        .call(|database| {
            migrations()
                .to_latest(database)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;

            // V002 cannot run Snowball inside SQLite. Backfill after the schema
            // transaction; the empty-value predicate makes retry after a crash safe.
            let titles = {
                let mut statement = database.prepare(
                    "SELECT rowid, title FROM pages
                      WHERE title IS NOT NULL AND title_stemmed = ''",
                )?;
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let transaction = database.transaction()?;
            for (rowid, title) in titles {
                transaction.execute(
                    "UPDATE pages SET title_stemmed = ?2 WHERE rowid = ?1",
                    rusqlite::params![rowid, crate::stem::stem(&title)],
                )?;
            }
            transaction.commit()
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

    #[tokio::test]
    async fn v002_backfills_existing_titles_into_the_stemmed_index() {
        let file = tempfile::NamedTempFile::new().expect("temporary database");
        let page_uuid = uuid::Uuid::from_u128(1);
        {
            let mut database = rusqlite::Connection::open(file.path()).expect("open baseline");
            Migrations::new(vec![M::up(include_str!("migrations/V001__initial.sql"))])
                .to_latest(&mut database)
                .expect("apply baseline");
            database
                .execute(
                    "INSERT INTO page_identities(page_uuid, page_kind) VALUES (?1, 'note')",
                    [page_uuid],
                )
                .expect("insert page identity");
            database
                .execute(
                    "INSERT INTO pages(
                       uuid, title, normalized_title, layout, existence_hlc, created_at, updated_at
                     ) VALUES (?1, 'Проекты', 'проекты', 'outline', '1', 1, 1)",
                    [page_uuid],
                )
                .expect("insert legacy page");
        }

        let connection = Connection::open(file.path())
            .await
            .expect("reopen database");
        migrate(&connection).await.expect("apply V002");
        let (stored_stem, matches): (String, i64) = connection
            .call(move |database| {
                Ok((
                    database.query_row(
                        "SELECT title_stemmed FROM pages WHERE uuid = ?1",
                        [page_uuid],
                        |row| row.get(0),
                    )?,
                    database.query_row(
                        "SELECT count(*) FROM pages_fts WHERE pages_fts MATCH '\"проект\"*'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .await
            .expect("inspect migrated index");
        assert_eq!(stored_stem, crate::stem::stem("Проекты"));
        assert_eq!(matches, 1);
    }
}

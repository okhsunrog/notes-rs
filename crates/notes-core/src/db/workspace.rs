use super::*;

pub(super) async fn initialize_workspace(conn: &Connection) -> Result<()> {
    let workspace_uuid = uuid::Uuid::now_v7();
    conn.call(move |database| {
        database.execute(
            "INSERT OR IGNORE INTO workspace(singleton, uuid) VALUES (1, ?1)",
            [workspace_uuid],
        )?;
        Ok(())
    })
    .await
}

pub async fn workspace_uuid(conn: &Connection) -> Result<uuid::Uuid> {
    conn.call(|database| {
        database.query_row(
            "SELECT uuid FROM workspace WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
    })
    .await
}

pub(crate) fn transaction_workspace_uuid(
    transaction: &rusqlite::Transaction<'_>,
) -> rusqlite::Result<uuid::Uuid> {
    transaction.query_row(
        "SELECT uuid FROM workspace WHERE singleton = 1",
        [],
        |row| row.get(0),
    )
}

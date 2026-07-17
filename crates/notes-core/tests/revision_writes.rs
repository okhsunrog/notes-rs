use notes_core::CoreError;
use notes_core::db::{self, BlockContent};
use notes_core::{BlockStyle, Connection};

struct TestDatabase {
    _directory: tempfile::TempDir,
    connection: Connection,
}

async fn database() -> TestDatabase {
    let directory = tempfile::tempdir().expect("temporary directory");
    let connection = db::open(directory.path().join("notes.db"))
        .await
        .expect("open database");
    TestDatabase {
        _directory: directory,
        connection,
    }
}

async fn mutation_counts(connection: &Connection) -> (i64, i64, i64) {
    connection
        .call(|database| {
            database.query_row(
                "SELECT (SELECT COUNT(*) FROM applied_ops),
                        (SELECT COUNT(*) FROM sync_outbox),
                        ((SELECT COUNT(*) FROM history_undo) +
                         (SELECT COUNT(*) FROM history_redo))",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .await
        .expect("read mutation counts")
}

fn assert_conflict(error: &anyhow::Error) {
    assert!(
        matches!(
            error.downcast_ref::<CoreError>(),
            Some(CoreError::Conflict(_))
        ),
        "expected typed conflict, got {error:#}"
    );
}

fn assert_not_found(error: &anyhow::Error) {
    assert!(
        matches!(
            error.downcast_ref::<CoreError>(),
            Some(CoreError::NotFound(_))
        ),
        "expected typed not-found, got {error:#}"
    );
}

#[tokio::test]
async fn page_title_write_checks_revision_atomically_and_accepts_stale_idempotence() {
    let database = database().await;
    let connection = &database.connection;
    let initial = db::create_page(connection, "Initial".into())
        .await
        .expect("create page");
    db::create_page(connection, "Reserved title".into())
        .await
        .expect("create collision target");
    notes_core::configure_sync(connection, "https://sync.example")
        .await
        .expect("configure sync");

    let (updated, changed) = db::rename_page_if_revision(
        connection,
        initial.uuid,
        Some("Updated".into()),
        initial.title_revision.clone(),
    )
    .await
    .expect("matching revision writes");
    assert!(changed);
    assert_eq!(updated.title.as_deref(), Some("Updated"));
    assert_ne!(updated.title_revision, initial.title_revision);
    let counts_after_write = mutation_counts(connection).await;
    assert_eq!(counts_after_write.1, 1);

    let collision = db::rename_page_if_revision(
        connection,
        initial.uuid,
        Some("Reserved title".into()),
        updated.title_revision.clone(),
    )
    .await
    .expect_err("title uniqueness conflict stays typed");
    assert_conflict(&collision);
    let after_collision = db::get_page(connection, initial.uuid)
        .await
        .unwrap()
        .expect("page remains after collision");
    assert_eq!(after_collision, updated);
    assert_eq!(mutation_counts(connection).await, counts_after_write);

    let rejected = db::rename_page_if_revision(
        connection,
        initial.uuid,
        Some("Overwrite remote change".into()),
        initial.title_revision.clone(),
    )
    .await
    .expect_err("stale different value must conflict");
    assert_conflict(&rejected);
    let after_rejection = db::get_page(connection, initial.uuid)
        .await
        .unwrap()
        .expect("page remains");
    assert_eq!(after_rejection.title, updated.title);
    assert_eq!(after_rejection.title_revision, updated.title_revision);
    assert_eq!(mutation_counts(connection).await, counts_after_write);

    let (idempotent, changed) = db::rename_page_if_revision(
        connection,
        initial.uuid,
        Some("  Updated  ".into()),
        initial.title_revision,
    )
    .await
    .expect("stale equal value is idempotent");
    assert!(!changed);
    assert_eq!(idempotent, updated);
    assert_eq!(mutation_counts(connection).await, counts_after_write);

    let missing = db::rename_page_if_revision(
        connection,
        uuid::Uuid::now_v7(),
        Some("Missing".into()),
        updated.title_revision,
    )
    .await
    .expect_err("missing page must stay not-found");
    assert_not_found(&missing);
}

#[tokio::test]
async fn block_markdown_write_checks_revision_atomically_and_accepts_stale_idempotence() {
    let database = database().await;
    let connection = &database.connection;
    let page = db::create_page(connection, "Page".into())
        .await
        .expect("create page");
    let initial = db::create_block(
        connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "Initial".into(),
    )
    .await
    .expect("create block");
    notes_core::configure_sync(connection, "https://sync.example")
        .await
        .expect("configure sync");

    let (updated, changed, graph_changed) = db::set_block_content_if_revision(
        connection,
        initial.uuid,
        BlockContent {
            markdown: "Updated [[link]]".into(),
        },
        initial.markdown_revision.clone(),
    )
    .await
    .expect("matching revision writes");
    assert!(changed);
    assert!(graph_changed);
    assert_eq!(updated.markdown, "Updated [[link]]");
    assert_ne!(updated.markdown_revision, initial.markdown_revision);
    let counts_after_write = mutation_counts(connection).await;
    assert_eq!(counts_after_write.1, 1);

    let rejected = db::set_block_content_if_revision(
        connection,
        initial.uuid,
        BlockContent {
            markdown: "Overwrite remote change".into(),
        },
        initial.markdown_revision.clone(),
    )
    .await
    .expect_err("stale different value must conflict");
    assert_conflict(&rejected);
    let after_rejection = db::get_block(connection, initial.uuid)
        .await
        .unwrap()
        .expect("block remains");
    assert_eq!(after_rejection.markdown, updated.markdown);
    assert_eq!(after_rejection.markdown_revision, updated.markdown_revision);
    assert_eq!(mutation_counts(connection).await, counts_after_write);

    let (idempotent, changed, graph_changed) = db::set_block_content_if_revision(
        connection,
        initial.uuid,
        BlockContent {
            markdown: updated.markdown.clone(),
        },
        initial.markdown_revision,
    )
    .await
    .expect("stale equal value is idempotent");
    assert!(!changed);
    assert!(!graph_changed);
    assert_eq!(idempotent, updated);
    assert_eq!(mutation_counts(connection).await, counts_after_write);

    let missing = db::set_block_content_if_revision(
        connection,
        uuid::Uuid::now_v7(),
        BlockContent {
            markdown: "Missing".into(),
        },
        updated.markdown_revision,
    )
    .await
    .expect_err("missing block must stay not-found");
    assert_not_found(&missing);
}

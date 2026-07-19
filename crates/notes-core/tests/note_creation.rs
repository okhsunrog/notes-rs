use notes_core::Connection;
use notes_core::db::{self, CreateNoteResult};

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
                        (SELECT COUNT(*) FROM history_undo),
                        (SELECT COUNT(*) FROM sync_outbox)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .await
        .expect("read mutation counts")
}

#[tokio::test]
async fn create_note_without_a_title_creates_a_page_and_initial_block_atomically() {
    let database = database().await;
    let result = db::create_note(&database.connection, None)
        .await
        .expect("create untitled note");
    let CreateNoteResult::Created { note } = result else {
        panic!("untitled note unexpectedly resolved to an existing page");
    };

    assert_eq!(note.page.title, None);
    assert_eq!(note.initial_block.page_uuid, note.page.uuid);
    assert_eq!(note.initial_block.parent_uuid, None);
    assert!(note.initial_block.markdown.is_empty());
    assert_eq!(mutation_counts(&database.connection).await, (2, 1, 0));
}

#[tokio::test]
async fn create_note_trims_a_prefilled_title_and_whitespace_is_untitled() {
    let database = database().await;
    let titled = db::create_note(&database.connection, Some("  Project Alpha  ".into()))
        .await
        .expect("create titled note")
        .into_created()
        .expect("free title creates a note");
    let untitled = db::create_note(&database.connection, Some("   ".into()))
        .await
        .expect("create whitespace-titled note")
        .into_created()
        .expect("whitespace is treated as an untitled note");

    assert_eq!(titled.page.title.as_deref(), Some("Project Alpha"));
    assert_eq!(untitled.page.title, None);
    assert_ne!(titled.page.uuid, untitled.page.uuid);
}

#[tokio::test]
async fn create_note_returns_existing_without_mutating_when_the_normalized_title_is_taken() {
    let database = database().await;
    let created = db::create_note(&database.connection, Some("Project Alpha".into()))
        .await
        .expect("create titled note")
        .into_created()
        .expect("free title creates a note");
    let counts_before = mutation_counts(&database.connection).await;

    let result = db::create_note(&database.connection, Some("  project alpha  ".into()))
        .await
        .expect("resolve existing note");
    let CreateNoteResult::Existing { page } = result else {
        panic!("taken title unexpectedly created a note");
    };

    assert_eq!(page.uuid, created.page.uuid);
    assert_eq!(page.title.as_deref(), Some("Project Alpha"));
    assert_eq!(mutation_counts(&database.connection).await, counts_before);
}

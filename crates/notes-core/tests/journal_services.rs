use notes_core::db::{self, BlockContent, JournalListLimit};
use notes_core::{BlockStyle, JournalDate, PageKind, PageListFilter};

struct TestDatabase {
    _directory: tempfile::TempDir,
    connection: notes_core::Connection,
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

fn date(value: &str) -> JournalDate {
    value.parse().expect("valid test date")
}

fn content(markdown: &str) -> BlockContent {
    BlockContent {
        markdown: markdown.into(),
    }
}

fn page_journal_date(page: &db::Page) -> &JournalDate {
    let PageKind::Journal { date } = &page.kind else {
        panic!("expected Journal page")
    };
    date
}

#[tokio::test]
async fn ensure_is_idempotent_and_creates_no_placeholder_block() {
    let database = database().await;
    let journal_date = date("2026-07-17");

    let first = db::ensure_journal(&database.connection, journal_date.clone())
        .await
        .expect("ensure journal");
    assert_eq!(page_journal_date(&first), &journal_date);
    assert!(
        db::list_block_children(&database.connection, first.uuid, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db::history_status(&database.connection)
            .await
            .unwrap()
            .undo_count,
        1
    );

    let second = db::ensure_journal(&database.connection, journal_date.clone())
        .await
        .expect("ensure same journal");
    assert_eq!(second, first);
    assert_eq!(
        db::history_status(&database.connection)
            .await
            .unwrap()
            .undo_count,
        1
    );
    assert_eq!(
        db::get_journal(&database.connection, journal_date)
            .await
            .unwrap(),
        Some(first)
    );
}

#[tokio::test]
async fn journal_listing_is_paginated_and_normal_page_listing_is_typed() {
    let database = database().await;
    db::create_page(&database.connection, "Alpha".into())
        .await
        .unwrap();
    db::create_page(&database.connection, "Beta".into())
        .await
        .unwrap();
    for value in ["2026-07-15", "2026-07-17", "2026-07-16"] {
        db::ensure_journal(&database.connection, date(value))
            .await
            .unwrap();
    }

    let first_page = db::list_journals(
        &database.connection,
        None,
        JournalListLimit::new(2).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        first_page
            .iter()
            .map(|journal| page_journal_date(journal).as_str())
            .collect::<Vec<_>>(),
        ["2026-07-17", "2026-07-16"]
    );
    let next_page = db::list_journals(
        &database.connection,
        Some(page_journal_date(&first_page[1]).clone()),
        JournalListLimit::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        next_page
            .iter()
            .map(|journal| page_journal_date(journal).as_str())
            .collect::<Vec<_>>(),
        ["2026-07-15"]
    );

    let notes = db::list_pages(&database.connection, 20).await.unwrap();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|page| page.kind == PageKind::Note));
    let journals = db::list_pages_filtered(&database.connection, PageListFilter::Journals, 20)
        .await
        .unwrap();
    assert_eq!(journals.len(), 3);
    assert!(journals.iter().all(|page| page.kind.is_journal()));
    assert_eq!(
        db::list_pages_filtered(&database.connection, PageListFilter::All, 20)
            .await
            .unwrap()
            .len(),
        5
    );
}

#[tokio::test]
async fn append_is_atomic_ordered_and_one_history_action_per_capture() {
    let database = database().await;
    let journal_date = date("2026-07-17");

    db::append_to_journal(
        &database.connection,
        journal_date.clone(),
        content("  \n"),
        BlockStyle::Paragraph,
    )
    .await
    .expect_err("blank capture must fail");
    assert!(
        db::get_journal(&database.connection, journal_date.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        db::history_status(&database.connection)
            .await
            .unwrap()
            .undo_count,
        0
    );

    let first = db::append_to_journal(
        &database.connection,
        journal_date.clone(),
        content("first capture"),
        BlockStyle::Paragraph,
    )
    .await
    .unwrap();
    let second = db::append_to_journal(
        &database.connection,
        journal_date.clone(),
        content("second capture"),
        BlockStyle::Heading2,
    )
    .await
    .unwrap();
    assert_eq!(first.page_uuid, second.page_uuid);
    assert_eq!(second.style, BlockStyle::Heading2);
    assert_eq!(
        db::history_status(&database.connection)
            .await
            .unwrap()
            .undo_count,
        2
    );
    let latest_forward = database
        .connection
        .call(|database| {
            database.query_row(
                "SELECT forward_json FROM history_undo ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
        })
        .await
        .unwrap();
    let latest_operations: Vec<notes_core::OpKind> = serde_json::from_str(&latest_forward).unwrap();
    assert_eq!(latest_operations.len(), 1);
    assert!(matches!(
        latest_operations.as_slice(),
        [notes_core::OpKind::BlockCreate(_)]
    ));
    let children = db::list_block_children(&database.connection, first.page_uuid, None)
        .await
        .unwrap();
    assert_eq!(
        children
            .iter()
            .map(|block| block.markdown.as_str())
            .collect::<Vec<_>>(),
        ["first capture", "second capture"]
    );
    assert!(children[0].order_key < children[1].order_key);

    assert!(db::undo_history(&database.connection).await.unwrap());
    let children = db::list_block_children(&database.connection, first.page_uuid, None)
        .await
        .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].uuid, first.uuid);

    assert!(db::undo_history(&database.connection).await.unwrap());
    assert!(
        db::get_journal(&database.connection, journal_date)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn journal_blocks_remain_searchable_and_visible_to_backlinks() {
    let database = database().await;
    let project = db::create_page(&database.connection, "Project Aurora".into())
        .await
        .unwrap();
    let block = db::append_to_journal(
        &database.connection,
        date("2026-07-17"),
        content("journalneedle links [[Project Aurora]]"),
        BlockStyle::Paragraph,
    )
    .await
    .unwrap();

    let hits = db::search_fts(&database.connection, "journalneedle".into(), 10)
        .await
        .unwrap();
    assert!(hits.iter().any(|hit| hit.content.uuid() == block.uuid));
    let backlinks = db::find_backlinks(&database.connection, project.uuid)
        .await
        .unwrap();
    assert!(backlinks.iter().any(|content| content.uuid() == block.uuid));

    let graph = db::graph_snapshot(&database.connection, Some(block.page_uuid))
        .await
        .unwrap();
    assert!(
        graph
            .items
            .iter()
            .any(|item| { item.uuid == block.page_uuid && item.label == "2026-07-17" })
    );
}

#[test]
fn journal_list_limit_rejects_unbounded_requests() {
    assert!(JournalListLimit::new(0).is_err());
    assert!(JournalListLimit::new(JournalListLimit::MAX + 1).is_err());
    assert_eq!(JournalListLimit::new(1).unwrap().get(), 1);
    assert_eq!(
        JournalListLimit::new(JournalListLimit::MAX).unwrap().get(),
        JournalListLimit::MAX
    );
    assert!(serde_json::from_str::<JournalListLimit>("0").is_err());
    assert_eq!(
        serde_json::from_str::<JournalListLimit>("30")
            .unwrap()
            .get(),
        30
    );
}

use notes_core::db;
use notes_core::{
    BlockCreate, BlockStyle, Connection, Hlc, JournalDate, Op, OpKind, OrderKey, Origin,
    PageCreate, PageDelete, PageKind, PageLayout, journal_page_uuid,
};

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

fn operation(
    index: u128,
    workspace_uuid: uuid::Uuid,
    device: u128,
    wall_ms: u64,
    kind: OpKind,
) -> Op {
    let device_id = uuid::Uuid::from_u128(device);
    Op {
        op_id: uuid::Uuid::from_u128(0x1000 + index),
        workspace_uuid,
        device_id,
        hlc: Hlc::new(wall_ms, 0, device_id),
        format_version: notes_core::operation::FORMAT_VERSION,
        kind,
    }
}

#[tokio::test]
async fn empty_snapshot_adopts_one_workspace_identity() {
    let source = database().await;
    let destination = database().await;
    let source_uuid = db::workspace_uuid(&source.connection).await.unwrap();
    assert_ne!(
        source_uuid,
        db::workspace_uuid(&destination.connection).await.unwrap()
    );

    let snapshot = notes_core::export_sync_snapshot(&source.connection, 0)
        .await
        .unwrap();
    notes_core::import_sync_snapshot(&destination.connection, snapshot)
        .await
        .unwrap();

    assert_eq!(
        db::workspace_uuid(&destination.connection).await.unwrap(),
        source_uuid
    );
}

#[tokio::test]
async fn offline_journal_creation_and_capture_converge() {
    let left = database().await;
    let right = database().await;
    let bootstrap = notes_core::export_sync_snapshot(&left.connection, 0)
        .await
        .unwrap();
    notes_core::import_sync_snapshot(&right.connection, bootstrap)
        .await
        .unwrap();
    let workspace_uuid = db::workspace_uuid(&left.connection).await.unwrap();
    let date = "2026-07-17".parse::<JournalDate>().unwrap();
    let page_uuid = journal_page_uuid(workspace_uuid, &date);
    let left_block = uuid::Uuid::from_u128(0xA1);
    let right_block = uuid::Uuid::from_u128(0xB1);

    let left_page = operation(
        1,
        workspace_uuid,
        0xA,
        1_000,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: PageKind::Journal { date: date.clone() },
            title: None,
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    let right_page = operation(
        2,
        workspace_uuid,
        0xB,
        2_000,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: PageKind::Journal { date: date.clone() },
            title: None,
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    let left_capture = operation(
        3,
        workspace_uuid,
        0xA,
        1_002,
        OpKind::BlockCreate(BlockCreate {
            uuid: left_block,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::first(),
            style: BlockStyle::Paragraph,
            markdown: "left offline capture".into(),
            created_at: 1,
        }),
    );
    let right_capture = operation(
        4,
        workspace_uuid,
        0xB,
        2_001,
        OpKind::BlockCreate(BlockCreate {
            uuid: right_block,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::first(),
            style: BlockStyle::Paragraph,
            markdown: "right offline capture".into(),
            created_at: 1,
        }),
    );

    for operation in [&left_page, &left_capture, &right_page, &right_capture] {
        notes_core::apply(&left.connection, operation, Origin::Remote)
            .await
            .unwrap();
    }
    for operation in [&right_page, &right_capture, &left_page, &left_capture] {
        notes_core::apply(&right.connection, operation, Origin::Remote)
            .await
            .unwrap();
    }

    assert_eq!(
        notes_core::export_sync_snapshot(&left.connection, 0)
            .await
            .unwrap(),
        notes_core::export_sync_snapshot(&right.connection, 0)
            .await
            .unwrap()
    );
    let page = db::get_page(&left.connection, page_uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.kind, PageKind::Journal { date });
    assert_eq!(
        db::list_block_children(&left.connection, page_uuid, None)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn page_kind_cannot_change_even_after_deletion() {
    let database = database().await;
    let workspace_uuid = db::workspace_uuid(&database.connection).await.unwrap();
    let date = "2026-07-17".parse::<JournalDate>().unwrap();
    let page_uuid = journal_page_uuid(workspace_uuid, &date);
    let create = operation(
        1,
        workspace_uuid,
        0xA,
        1_000,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: PageKind::Journal { date },
            title: None,
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    notes_core::apply(&database.connection, &create, Origin::Remote)
        .await
        .unwrap();
    notes_core::apply(
        &database.connection,
        &operation(
            2,
            workspace_uuid,
            0xA,
            2_000,
            OpKind::PageDelete(PageDelete { uuid: page_uuid }),
        ),
        Origin::Remote,
    )
    .await
    .unwrap();

    let error = notes_core::apply(
        &database.connection,
        &operation(
            3,
            workspace_uuid,
            0xB,
            3_000,
            OpKind::PageCreate(PageCreate {
                uuid: page_uuid,
                kind: PageKind::Note,
                title: Some("not a journal".into()),
                layout: PageLayout::Outline,
                created_at: 2,
            }),
        ),
        Origin::Remote,
    )
    .await
    .expect_err("page kind reuse must fail");
    assert!(error.to_string().contains("different page kind"));
}

#[tokio::test]
async fn non_empty_workspace_rejects_a_foreign_snapshot() {
    let left = database().await;
    let right = database().await;
    db::create_page(&left.connection, "Left".into())
        .await
        .unwrap();
    db::create_page(&right.connection, "Right".into())
        .await
        .unwrap();
    let foreign = notes_core::export_sync_snapshot(&left.connection, 0)
        .await
        .unwrap();

    let error = notes_core::import_sync_snapshot(&right.connection, foreign)
        .await
        .expect_err("non-empty workspace mismatch must fail");
    assert!(error.to_string().contains("different non-empty workspace"));
}

#[tokio::test]
async fn operations_are_bound_to_one_workspace() {
    let database = database().await;
    let local_workspace = db::workspace_uuid(&database.connection).await.unwrap();
    let foreign_workspace = uuid::Uuid::from_u128(0xBAD);
    let error = notes_core::apply(
        &database.connection,
        &operation(
            1,
            foreign_workspace,
            0xA,
            1_000,
            OpKind::PageCreate(PageCreate {
                uuid: uuid::Uuid::from_u128(1),
                kind: PageKind::Note,
                title: Some("foreign".into()),
                layout: PageLayout::Outline,
                created_at: 1,
            }),
        ),
        Origin::Remote,
    )
    .await
    .expect_err("foreign workspace operation must fail");
    assert!(error.to_string().contains(&local_workspace.to_string()));
    assert!(
        db::list_pages(&database.connection, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn journal_identity_survives_delete_and_snapshot_bootstrap() {
    let source = database().await;
    let workspace_uuid = db::workspace_uuid(&source.connection).await.unwrap();
    let date = "2026-07-17".parse::<JournalDate>().unwrap();
    let page_uuid = journal_page_uuid(workspace_uuid, &date);
    notes_core::apply(
        &source.connection,
        &operation(
            1,
            workspace_uuid,
            0xA,
            1_000,
            OpKind::PageCreate(PageCreate {
                uuid: page_uuid,
                kind: PageKind::Journal { date: date.clone() },
                title: None,
                layout: PageLayout::Outline,
                created_at: 1,
            }),
        ),
        Origin::Remote,
    )
    .await
    .unwrap();
    notes_core::apply(
        &source.connection,
        &operation(
            2,
            workspace_uuid,
            0xA,
            2_000,
            OpKind::PageDelete(PageDelete { uuid: page_uuid }),
        ),
        Origin::Remote,
    )
    .await
    .unwrap();

    let snapshot = notes_core::export_sync_snapshot(&source.connection, 0)
        .await
        .unwrap();
    assert_eq!(
        snapshot.page_identities,
        vec![notes_core::SnapshotPageIdentity {
            uuid: page_uuid,
            kind: PageKind::Journal { date },
        }]
    );
    assert!(snapshot.pages.is_empty());

    let archive = db::export_archive(&source.connection).await.unwrap();
    assert_eq!(archive.page_identities, snapshot.page_identities);
    let archive_destination = database().await;
    db::import_archive(&archive_destination.connection, archive)
        .await
        .unwrap();
    assert_eq!(
        notes_core::export_sync_snapshot(&archive_destination.connection, 0)
            .await
            .unwrap()
            .page_identities,
        snapshot.page_identities
    );

    let destination = database().await;
    notes_core::import_sync_snapshot(&destination.connection, snapshot.clone())
        .await
        .unwrap();
    assert_eq!(
        notes_core::export_sync_snapshot(&destination.connection, 0)
            .await
            .unwrap(),
        snapshot
    );
    let foreign_key_violations = destination
        .connection
        .call(|database| {
            database
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |_| Ok(()))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    assert!(foreign_key_violations.is_empty());
}

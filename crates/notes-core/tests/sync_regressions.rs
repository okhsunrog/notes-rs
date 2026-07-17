use notes_core::db;
use notes_core::{
    AttachmentOwner, BlockCreate, BlockDelete, BlockMove, BlockStyle, Connection, Hlc, ObjectKind,
    Op, OpKind, OrderKey, Origin, PageCreate, PageDelete, PageLayout, SnapshotAttachment,
    SyncSnapshot,
};

const TEST_WORKSPACE_UUID: uuid::Uuid = uuid::Uuid::from_u128(0xC0DE);

struct TestDatabase {
    _directory: tempfile::TempDir,
    connection: Connection,
}

async fn database() -> TestDatabase {
    let directory = tempfile::tempdir().expect("temporary directory");
    let connection = db::open(directory.path().join("notes.db"))
        .await
        .expect("open database");
    connection
        .call(|database| {
            database.execute(
                "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                [TEST_WORKSPACE_UUID],
            )?;
            Ok(())
        })
        .await
        .expect("set deterministic workspace");
    TestDatabase {
        _directory: directory,
        connection,
    }
}

fn op(index: u128, wall_ms: u64, kind: OpKind) -> Op {
    let device_id = uuid::Uuid::from_u128(0xD3A1CE);
    Op {
        op_id: uuid::Uuid::from_u128(0x1000 + index),
        workspace_uuid: TEST_WORKSPACE_UUID,
        device_id,
        hlc: Hlc::new(wall_ms, 0, device_id),
        format_version: notes_core::operation::FORMAT_VERSION,
        kind,
    }
}

fn page_create(index: u128, page_uuid: uuid::Uuid) -> Op {
    op(
        index,
        index as u64 + 1,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: notes_core::PageKind::Note,
            title: Some(format!("Page {index}")),
            layout: PageLayout::Outline,
            created_at: index as i64,
        }),
    )
}

#[tokio::test]
async fn deferred_reference_projection_discards_blocks_deleted_in_the_same_batch() {
    let block_delete_database = database().await;
    let page_uuid = uuid::Uuid::from_u128(0xDD01);
    let block_uuid = uuid::Uuid::from_u128(0xDD02);
    notes_core::apply_batch(
        &block_delete_database.connection,
        &[
            page_create(1, page_uuid),
            op(
                2,
                2,
                OpKind::BlockCreate(BlockCreate {
                    uuid: block_uuid,
                    page_uuid,
                    parent_uuid: None,
                    order_key: OrderKey::first(),
                    style: BlockStyle::Paragraph,
                    markdown: "[[Page 1]]".into(),
                    created_at: 2,
                }),
            ),
            op(
                3,
                3,
                OpKind::BlockDelete(BlockDelete {
                    uuid: block_uuid,
                    page_uuid,
                }),
            ),
        ],
        Origin::Remote,
    )
    .await
    .expect("create then delete block atomically");
    let counts = block_delete_database
        .connection
        .call(|database| {
            database.query_row(
                "SELECT (SELECT COUNT(*) FROM blocks), (SELECT COUNT(*) FROM page_links)",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!(counts, (0, 0));

    let page_delete_database = database().await;
    let page_uuid = uuid::Uuid::from_u128(0xDD11);
    let block_uuid = uuid::Uuid::from_u128(0xDD12);
    notes_core::apply_batch(
        &page_delete_database.connection,
        &[
            page_create(11, page_uuid),
            op(
                12,
                12,
                OpKind::BlockCreate(BlockCreate {
                    uuid: block_uuid,
                    page_uuid,
                    parent_uuid: None,
                    order_key: OrderKey::first(),
                    style: BlockStyle::Paragraph,
                    markdown: "[[Page 11]]".into(),
                    created_at: 12,
                }),
            ),
            op(13, 13, OpKind::PageDelete(PageDelete { uuid: page_uuid })),
        ],
        Origin::Remote,
    )
    .await
    .expect("create page subtree then delete page atomically");
    let counts = page_delete_database
        .connection
        .call(|database| {
            database.query_row(
                "SELECT
                   (SELECT COUNT(*) FROM pages),
                   (SELECT COUNT(*) FROM blocks),
                   (SELECT COUNT(*) FROM page_links)",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
        })
        .await
        .unwrap();
    assert_eq!(counts, (0, 0, 0));
}

#[tokio::test]
async fn local_apply_is_idempotent_and_outbox_is_acknowledged_once() {
    let database = database().await;
    let connection = &database.connection;
    notes_core::configure_sync(connection, "https://notes.example.test")
        .await
        .expect("configure sync");
    let operation = page_create(1, uuid::Uuid::from_u128(1));

    assert!(
        notes_core::apply(connection, &operation, Origin::Local)
            .await
            .unwrap()
            .applied
    );
    assert!(
        !notes_core::apply(connection, &operation, Origin::Local)
            .await
            .unwrap()
            .applied
    );
    let pending = notes_core::pending_outbox(connection, 100)
        .await
        .expect("read outbox");
    assert_eq!(pending, vec![operation.clone()]);

    notes_core::acknowledge_server_op(connection, operation.op_id, 1)
        .await
        .expect("acknowledge operation");
    assert!(
        notes_core::pending_outbox(connection, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn sequenced_apply_rejects_gaps_and_conflicting_duplicates_atomically() {
    let database = database().await;
    let connection = &database.connection;
    let first_page = uuid::Uuid::from_u128(1);
    let second_page = uuid::Uuid::from_u128(2);
    let first = page_create(1, first_page);
    let second = page_create(2, second_page);

    let error = notes_core::apply_sequenced(connection, 2, &second)
        .await
        .expect_err("initial sequence gap must fail");
    assert!(error.to_string().contains("sequence gap"));
    assert_eq!(notes_core::sync_cursor(connection).await.unwrap(), 0);
    assert!(
        db::get_page(connection, second_page)
            .await
            .unwrap()
            .is_none()
    );

    let error = notes_core::apply_sequenced_batch(
        connection,
        vec![(1, first.clone()), (3, second.clone())],
    )
    .await
    .expect_err("gap inside a batch must roll the whole batch back");
    assert!(error.to_string().contains("sequence gap"));
    assert_eq!(notes_core::sync_cursor(connection).await.unwrap(), 0);
    assert!(
        db::get_page(connection, first_page)
            .await
            .unwrap()
            .is_none()
    );

    assert!(
        notes_core::apply_sequenced(connection, 1, &first)
            .await
            .unwrap()
            .applied
    );
    assert!(
        !notes_core::apply_sequenced(connection, 1, &first)
            .await
            .unwrap()
            .applied
    );
    assert_eq!(notes_core::sync_cursor(connection).await.unwrap(), 1);

    let conflicting = page_create(3, uuid::Uuid::from_u128(3));
    let error = notes_core::apply_sequenced(connection, 1, &conflicting)
        .await
        .expect_err("one sequence cannot identify two operations");
    assert!(error.to_string().contains("multiple operations"));

    let error = notes_core::apply_sequenced(connection, 2, &first)
        .await
        .expect_err("one operation cannot be assigned two sequences");
    assert!(error.to_string().contains("server sequences"));
    assert_eq!(notes_core::sync_cursor(connection).await.unwrap(), 1);
}

#[tokio::test]
async fn snapshots_preserve_raw_structure_intents_and_reobserve_future_hlc() {
    let source = database().await;
    let connection = &source.connection;
    let page_uuid = uuid::Uuid::from_u128(1);
    let first_uuid = uuid::Uuid::from_u128(2);
    let second_uuid = uuid::Uuid::from_u128(3);
    let setup = vec![
        op(
            1,
            10,
            OpKind::PageCreate(PageCreate {
                uuid: page_uuid,
                kind: notes_core::PageKind::Note,
                title: Some("Future title".into()),
                layout: PageLayout::Outline,
                created_at: 1,
            }),
        ),
        op(
            2,
            11,
            OpKind::BlockCreate(BlockCreate {
                uuid: first_uuid,
                page_uuid,
                parent_uuid: None,
                order_key: OrderKey::from_ordinal(1),
                style: BlockStyle::Paragraph,
                markdown: "First".into(),
                created_at: 1,
            }),
        ),
        op(
            3,
            12,
            OpKind::BlockCreate(BlockCreate {
                uuid: second_uuid,
                page_uuid,
                parent_uuid: None,
                order_key: OrderKey::from_ordinal(2),
                style: BlockStyle::Paragraph,
                markdown: "Second".into(),
                created_at: 1,
            }),
        ),
        op(
            4,
            20,
            OpKind::BlockMove(BlockMove {
                uuid: first_uuid,
                page_uuid,
                parent_uuid: Some(second_uuid),
                order_key: OrderKey::from_ordinal(1),
            }),
        ),
        op(
            5,
            21,
            OpKind::BlockMove(BlockMove {
                uuid: second_uuid,
                page_uuid,
                parent_uuid: Some(first_uuid),
                order_key: OrderKey::from_ordinal(1),
            }),
        ),
    ];
    notes_core::apply_batch(connection, &setup, Origin::Remote)
        .await
        .expect("apply cyclic raw intents");
    let snapshot = notes_core::export_sync_snapshot(connection, 77)
        .await
        .expect("export snapshot");
    assert_eq!(snapshot.seq, 77);
    assert_eq!(snapshot.structures.len(), 2);
    assert!(snapshot.structures.iter().any(|structure| {
        structure.block_uuid == first_uuid && structure.parent_uuid == Some(second_uuid)
    }));
    assert!(snapshot.structures.iter().any(|structure| {
        structure.block_uuid == second_uuid && structure.parent_uuid == Some(first_uuid)
    }));
    let materialized_parents = snapshot
        .blocks
        .iter()
        .map(|block| (block.uuid, block.parent_uuid))
        .collect::<std::collections::HashMap<_, _>>();
    assert!(
        !(materialized_parents[&first_uuid] == Some(second_uuid)
            && materialized_parents[&second_uuid] == Some(first_uuid)),
        "the materialized tree must be acyclic even though both raw LWW intents are retained"
    );

    let destination = database().await;
    notes_core::import_sync_snapshot(&destination.connection, snapshot.clone())
        .await
        .expect("import snapshot");
    assert_eq!(
        notes_core::sync_cursor(&destination.connection)
            .await
            .unwrap(),
        77
    );
    let imported = notes_core::export_sync_snapshot(&destination.connection, 77)
        .await
        .expect("re-export imported snapshot");
    assert_eq!(imported.structures, snapshot.structures);
    assert_eq!(
        imported
            .blocks
            .iter()
            .map(|block| (block.uuid, block.parent_uuid))
            .collect::<std::collections::HashMap<_, _>>(),
        materialized_parents
    );

    let future = Hlc::new(
        chrono::Utc::now().timestamp_millis() as u64 + 86_400_000,
        40,
        uuid::Uuid::from_u128(0xF0),
    );
    let mut future_snapshot = imported;
    let page = future_snapshot
        .pages
        .iter_mut()
        .find(|page| page.uuid == page_uuid)
        .expect("page in snapshot");
    page.title = Some("From the future".into());
    page.title_hlc = Some(future.clone());
    page.existence_hlc = future.clone();
    notes_core::import_sync_snapshot(&destination.connection, future_snapshot)
        .await
        .expect("import future clock");
    db::rename_page(
        &destination.connection,
        page_uuid,
        Some("Local rename wins".into()),
    )
    .await
    .expect("rename after observing future clock");
    assert_eq!(
        db::get_page(&destination.connection, page_uuid)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("Local rename wins")
    );
    let renamed = notes_core::export_sync_snapshot(&destination.connection, 77)
        .await
        .expect("export renamed state");
    assert!(
        renamed
            .pages
            .iter()
            .find(|page| page.uuid == page_uuid)
            .and_then(|page| page.title_hlc.as_ref())
            .is_some_and(|hlc| hlc > &future)
    );
}

#[tokio::test]
async fn invalid_order_keys_and_attachment_snapshots_are_rejected_before_replacing_state() {
    let source = database().await;
    let page = db::create_page(&source.connection, "Snapshot validation".into())
        .await
        .expect("create page");
    let block = db::create_block(
        &source.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "Body".into(),
    )
    .await
    .expect("create block");
    db::create_attachment(
        &source.connection,
        AttachmentOwner::Page(page.uuid),
        notes_core::BlobHash::from_bytes([0xaa; 32]),
        "file.txt".into(),
        "text/plain".into(),
        1,
    )
    .await
    .expect("create attachment");
    let snapshot = notes_core::export_sync_snapshot(&source.connection, 1)
        .await
        .expect("export valid snapshot");

    let mut json = serde_json::to_value(&snapshot).expect("serialize snapshot");
    json["blocks"][0]["order_key"] = serde_json::Value::String("not-an-order-key".into());
    assert!(serde_json::from_value::<SyncSnapshot>(json).is_err());

    let raw_order_error = source
        .connection
        .call(move |sqlite| {
            sqlite.execute(
                "UPDATE blocks SET order_key = 'lowercase00000000' WHERE uuid = ?1",
                [block.uuid],
            )
        })
        .await
        .expect_err("database constraint must reject invalid order keys");
    assert!(raw_order_error.to_string().contains("CHECK constraint"));

    let destination = database().await;
    let sentinel = db::create_page(&destination.connection, "Sentinel".into())
        .await
        .expect("create state that failed imports must preserve");

    let mut invalid_hash = serde_json::to_value(&snapshot).unwrap();
    invalid_hash["attachments"][0]["blob_hash"] = serde_json::json!("bad");
    assert!(serde_json::from_value::<SyncSnapshot>(invalid_hash).is_err());
    let mut uppercase_hash = serde_json::to_value(&snapshot).unwrap();
    uppercase_hash["attachments"][0]["blob_hash"] =
        serde_json::json!(snapshot.attachments[0].blob_hash.to_string().to_uppercase());
    assert!(serde_json::from_value::<SyncSnapshot>(uppercase_hash).is_err());
    assert!(
        db::get_page(&destination.connection, sentinel.uuid)
            .await
            .unwrap()
            .is_some()
    );

    let mut invalid_uuid = snapshot.clone();
    invalid_uuid.attachments[0].attachment_uuid = uuid::Uuid::now_v7();
    assert!(
        notes_core::import_sync_snapshot(&destination.connection, invalid_uuid)
            .await
            .expect_err("attachment UUID must be derived from owner and hash")
            .to_string()
            .contains("does not match")
    );

    let mut missing_metadata = snapshot.clone();
    missing_metadata.attachments[0].filename = None;
    assert!(
        notes_core::import_sync_snapshot(&destination.connection, missing_metadata)
            .await
            .expect_err("present attachment requires metadata")
            .to_string()
            .contains("filename")
    );

    let mut cross_kind_uuid = snapshot.clone();
    cross_kind_uuid.blocks[0].uuid = cross_kind_uuid.pages[0].uuid;
    assert!(
        notes_core::import_sync_snapshot(&destination.connection, cross_kind_uuid)
            .await
            .expect_err("page and block UUID spaces must not overlap")
            .to_string()
            .contains("page identity and a block")
    );

    let mut live_tombstone = snapshot.clone();
    live_tombstone
        .tombstones
        .push(notes_core::SnapshotTombstone {
            uuid: page.uuid,
            object_kind: ObjectKind::Page,
            deleted_hlc: Hlc::new(100, 0, uuid::Uuid::from_u128(1)),
            root_page_uuid: Some(page.uuid),
        });
    assert!(
        notes_core::import_sync_snapshot(&destination.connection, live_tombstone)
            .await
            .expect_err("live page cannot also be tombstoned")
            .to_string()
            .contains("live and tombstoned")
    );

    let mut missing_structure = snapshot.clone();
    missing_structure.structures.clear();
    assert!(
        notes_core::import_sync_snapshot(&destination.connection, missing_structure)
            .await
            .expect_err("every live block requires a raw structure intent")
            .to_string()
            .contains("no raw structure intent")
    );

    let mut mismatched_structure = snapshot.clone();
    mismatched_structure.structures[0].hlc = Hlc::new(101, 0, uuid::Uuid::from_u128(2));
    assert!(
        notes_core::import_sync_snapshot(&destination.connection, mismatched_structure)
            .await
            .expect_err("materialized and raw structure clocks must agree")
            .to_string()
            .contains("does not match")
    );

    let invalid_tombstone_kind = serde_json::json!({
        "uuid": uuid::Uuid::now_v7(),
        "object_kind": "node",
        "deleted_hlc": Hlc::new(1, 0, uuid::Uuid::from_u128(1)),
        "root_page_uuid": null
    });
    assert!(
        serde_json::from_value::<notes_core::SnapshotTombstone>(invalid_tombstone_kind).is_err()
    );

    let attachment = &snapshot.attachments[0];
    assert_eq!(attachment.owner, AttachmentOwner::Page(page.uuid));
    assert!(attachment.present);
    assert!(
        snapshot
            .tombstones
            .iter()
            .all(|row| row.object_kind != ObjectKind::Block)
    );
}

#[test]
fn malformed_attachment_snapshot_json_cannot_smuggle_an_untyped_owner() {
    let invalid = serde_json::json!({
        "owner": { "kind": "node", "uuid": uuid::Uuid::now_v7() },
        "blob_hash": "a".repeat(64),
        "attachment_uuid": uuid::Uuid::now_v7(),
        "hlc": Hlc::new(1, 0, uuid::Uuid::from_u128(1)),
        "present": false,
        "filename": null,
        "mime": null,
        "size": null
    });
    assert!(serde_json::from_value::<SnapshotAttachment>(invalid).is_err());
}

#[tokio::test]
async fn concurrent_same_title_creation_converges_without_rejecting_either_page() {
    let left = database().await;
    let right = database().await;
    let first_uuid = uuid::Uuid::from_u128(0xA1);
    let second_uuid = uuid::Uuid::from_u128(0xA2);
    let first = op(
        0xA1,
        100,
        OpKind::PageCreate(PageCreate {
            uuid: first_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Проект Ёж".into()),
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    let second = op(
        0xA2,
        200,
        OpKind::PageCreate(PageCreate {
            uuid: second_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Проект Ёж".into()),
            layout: PageLayout::Document,
            created_at: 2,
        }),
    );

    notes_core::apply_batch(
        &left.connection,
        &[first.clone(), second.clone()],
        Origin::Remote,
    )
    .await
    .expect("apply title conflict in chronological order");
    notes_core::apply_batch(&right.connection, &[second, first], Origin::Remote)
        .await
        .expect("apply title conflict in reverse order");

    let left_pages = db::list_pages(&left.connection, 10).await.unwrap();
    let right_pages = db::list_pages(&right.connection, 10).await.unwrap();
    assert_eq!(left_pages, right_pages);
    assert_eq!(left_pages.len(), 2);
    assert_eq!(
        db::get_page(&left.connection, second_uuid)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("Проект Ёж")
    );
    assert!(
        db::get_page(&left.connection, first_uuid)
            .await
            .unwrap()
            .unwrap()
            .title
            .is_none()
    );
}

#[tokio::test]
async fn page_delete_and_delayed_block_create_produce_the_same_tombstones() {
    let left = database().await;
    let right = database().await;
    let page_uuid = uuid::Uuid::from_u128(0xB1);
    let block_uuid = uuid::Uuid::from_u128(0xB2);
    let page = op(
        0xB1,
        10,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Deleted page".into()),
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    let block = op(
        0xB2,
        20,
        OpKind::BlockCreate(BlockCreate {
            uuid: block_uuid,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::first(),
            style: BlockStyle::Paragraph,
            markdown: "Delayed".into(),
            created_at: 2,
        }),
    );
    let delete = op(0xB3, 30, OpKind::PageDelete(PageDelete { uuid: page_uuid }));
    notes_core::apply(&left.connection, &page, Origin::Remote)
        .await
        .unwrap();
    notes_core::apply(&right.connection, &page, Origin::Remote)
        .await
        .unwrap();
    notes_core::apply_batch(
        &left.connection,
        &[block.clone(), delete.clone()],
        Origin::Remote,
    )
    .await
    .unwrap();
    notes_core::apply_batch(&right.connection, &[delete, block], Origin::Remote)
        .await
        .unwrap();

    let left_snapshot = notes_core::export_sync_snapshot(&left.connection, 0)
        .await
        .unwrap();
    let right_snapshot = notes_core::export_sync_snapshot(&right.connection, 0)
        .await
        .unwrap();
    assert_eq!(left_snapshot.pages, right_snapshot.pages);
    assert_eq!(left_snapshot.blocks, right_snapshot.blocks);
    assert_eq!(left_snapshot.tombstones, right_snapshot.tombstones);
    assert_eq!(left_snapshot.tombstones.len(), 2);
}

#[tokio::test]
async fn page_recreation_is_a_generation_boundary_for_delayed_blocks() {
    let databases = [database().await, database().await, database().await];
    let page_uuid = uuid::Uuid::from_u128(0xC1);
    let block_uuid = uuid::Uuid::from_u128(0xC2);
    let initial_page = op(
        0xC1,
        10,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Initial generation".into()),
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    let delayed_block = op(
        0xC2,
        20,
        OpKind::BlockCreate(BlockCreate {
            uuid: block_uuid,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::first(),
            style: BlockStyle::Paragraph,
            markdown: "Must not cross generations".into(),
            created_at: 2,
        }),
    );
    let delete = op(0xC3, 30, OpKind::PageDelete(PageDelete { uuid: page_uuid }));
    let recreate = op(
        0xC4,
        40,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Recreated generation".into()),
            layout: PageLayout::Document,
            created_at: 3,
        }),
    );

    for database in &databases {
        notes_core::apply(&database.connection, &initial_page, Origin::Remote)
            .await
            .unwrap();
    }
    for (database, operations) in databases.iter().zip([
        vec![delayed_block.clone(), delete.clone(), recreate.clone()],
        vec![delete.clone(), recreate.clone(), delayed_block.clone()],
        vec![recreate.clone(), delayed_block.clone(), delete.clone()],
    ]) {
        for operation in operations {
            notes_core::apply(&database.connection, &operation, Origin::Remote)
                .await
                .unwrap();
        }
    }

    let mut snapshots = Vec::new();
    for database in &databases {
        snapshots.push(
            notes_core::export_sync_snapshot(&database.connection, 0)
                .await
                .unwrap(),
        );
    }
    assert!(snapshots.iter().all(|snapshot| snapshot.blocks.is_empty()));
    assert_eq!(snapshots[0], snapshots[1]);
    assert_eq!(snapshots[1], snapshots[2]);
    assert_eq!(
        snapshots[0].pages[0].title.as_deref(),
        Some("Recreated generation")
    );
    assert_eq!(snapshots[0].pages[0].layout, PageLayout::Document);
    let block_tombstone = snapshots[0]
        .tombstones
        .iter()
        .find(|tombstone| tombstone.uuid == block_uuid)
        .expect("old block generation must stay tombstoned");
    assert_eq!(block_tombstone.deleted_hlc, recreate.hlc);
}

#[tokio::test]
async fn raw_parent_intent_is_order_independent_across_parent_recreation() {
    let left = database().await;
    let right = database().await;
    let page_uuid = uuid::Uuid::from_u128(0xD1);
    let parent_uuid = uuid::Uuid::from_u128(0xD2);
    let child_uuid = uuid::Uuid::from_u128(0xD3);
    let page = op(
        0xD1,
        1,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Structure generations".into()),
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    let parent = op(
        0xD2,
        10,
        OpKind::BlockCreate(BlockCreate {
            uuid: parent_uuid,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::from_ordinal(1),
            style: BlockStyle::Paragraph,
            markdown: "Parent".into(),
            created_at: 2,
        }),
    );
    let child = op(
        0xD3,
        11,
        OpKind::BlockCreate(BlockCreate {
            uuid: child_uuid,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::from_ordinal(2),
            style: BlockStyle::Paragraph,
            markdown: "Child".into(),
            created_at: 3,
        }),
    );
    let old_move = op(
        0xD4,
        15,
        OpKind::BlockMove(BlockMove {
            uuid: child_uuid,
            page_uuid,
            parent_uuid: Some(parent_uuid),
            order_key: OrderKey::first(),
        }),
    );
    let delete_parent = op(
        0xD5,
        20,
        OpKind::BlockDelete(notes_core::BlockDelete {
            uuid: parent_uuid,
            page_uuid,
        }),
    );
    let recreate_parent = op(
        0xD6,
        30,
        OpKind::BlockCreate(BlockCreate {
            uuid: parent_uuid,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::from_ordinal(1),
            style: BlockStyle::Paragraph,
            markdown: "Recreated parent".into(),
            created_at: 4,
        }),
    );

    for database in [&left, &right] {
        notes_core::apply_batch(
            &database.connection,
            &[page.clone(), parent.clone(), child.clone()],
            Origin::Remote,
        )
        .await
        .unwrap();
    }
    notes_core::apply_batch(
        &left.connection,
        &[
            old_move.clone(),
            delete_parent.clone(),
            recreate_parent.clone(),
        ],
        Origin::Remote,
    )
    .await
    .unwrap();
    notes_core::apply_batch(
        &right.connection,
        &[delete_parent, recreate_parent, old_move],
        Origin::Remote,
    )
    .await
    .unwrap();

    let left_snapshot = notes_core::export_sync_snapshot(&left.connection, 0)
        .await
        .unwrap();
    let right_snapshot = notes_core::export_sync_snapshot(&right.connection, 0)
        .await
        .unwrap();
    assert_eq!(left_snapshot, right_snapshot);
    assert_eq!(
        db::get_block(&left.connection, child_uuid)
            .await
            .unwrap()
            .unwrap()
            .parent_uuid,
        None,
        "an intent from the deleted parent generation must not bind to its recreation"
    );
    assert!(left_snapshot.structures.iter().any(|intent| {
        intent.block_uuid == child_uuid && intent.parent_uuid == Some(parent_uuid)
    }));

    let current_move = op(
        0xD7,
        35,
        OpKind::BlockMove(BlockMove {
            uuid: child_uuid,
            page_uuid,
            parent_uuid: Some(parent_uuid),
            order_key: OrderKey::first(),
        }),
    );
    for database in [&left, &right] {
        notes_core::apply(&database.connection, &current_move, Origin::Remote)
            .await
            .unwrap();
        assert_eq!(
            db::get_block(&database.connection, child_uuid)
                .await
                .unwrap()
                .unwrap()
                .parent_uuid,
            Some(parent_uuid)
        );
    }
}

#[tokio::test]
async fn uuid_identity_cannot_change_between_page_and_block_kinds() {
    let database = database().await;
    let page_uuid = uuid::Uuid::from_u128(0xE1);
    let page = op(
        0xE1,
        1,
        OpKind::PageCreate(PageCreate {
            uuid: page_uuid,
            kind: notes_core::PageKind::Note,
            title: Some("Global UUID".into()),
            layout: PageLayout::Outline,
            created_at: 1,
        }),
    );
    notes_core::apply(&database.connection, &page, Origin::Remote)
        .await
        .unwrap();
    let conflicting_block = op(
        0xE2,
        2,
        OpKind::BlockCreate(BlockCreate {
            uuid: page_uuid,
            page_uuid,
            parent_uuid: None,
            order_key: OrderKey::first(),
            style: BlockStyle::Paragraph,
            markdown: "collision".into(),
            created_at: 2,
        }),
    );
    assert!(
        notes_core::apply(&database.connection, &conflicting_block, Origin::Remote)
            .await
            .expect_err("page UUID cannot become a block UUID")
            .to_string()
            .contains("different object kind")
    );

    let raw_collision = database
        .connection
        .call(move |sqlite| {
            sqlite.execute(
                "INSERT INTO blocks(
                   uuid, page_uuid, parent_uuid, order_key, style, markdown, body_stemmed,
                   existence_hlc, created_at, updated_at
                 ) VALUES (?1, ?1, NULL, '0000000100000000', 'paragraph', '', '', '1', 1, 1)",
                [page_uuid],
            )
        })
        .await
        .expect_err("SQLite baseline must guard cross-kind UUID collisions");
    assert!(raw_collision.to_string().contains("already used by a page"));
}

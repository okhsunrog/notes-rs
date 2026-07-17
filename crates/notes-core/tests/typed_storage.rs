use notes_core::db::{self, BlockContent, Content};
use notes_core::{AttachmentOwner, BlobHash, BlockStyle, Connection, PageLayout, TaskState};

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

#[tokio::test]
async fn baseline_has_typed_page_and_block_tables_without_legacy_graph_tables() {
    let database = database().await;
    let tables = database
        .connection
        .call(|sqlite| {
            sqlite
                .prepare(
                    "SELECT name FROM sqlite_master
                       WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                       ORDER BY name",
                )?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .expect("list schema tables");

    assert!(tables.iter().any(|table| table == "pages"));
    assert!(tables.iter().any(|table| table == "blocks"));
    assert!(!tables.iter().any(|table| table == "nodes"));
    assert!(!tables.iter().any(|table| table == "edges"));
}

#[tokio::test]
async fn attachment_hashes_are_strict_32_byte_blobs() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Attachments".into())
        .await
        .expect("create page");
    let hash = BlobHash::from_bytes([0xab; 32]);
    db::create_attachment(
        &database.connection,
        AttachmentOwner::Page(page.uuid),
        hash,
        "asset.png".into(),
        "image/png".into(),
        128,
    )
    .await
    .expect("create attachment");

    let storage = database
        .connection
        .call(|sqlite| {
            sqlite.query_row(
                "SELECT
                   (SELECT typeof(blob_hash) FROM attachments LIMIT 1),
                   (SELECT length(blob_hash) FROM attachments LIMIT 1),
                   (SELECT typeof(blob_hash) FROM attachment_lww LIMIT 1),
                   (SELECT length(blob_hash) FROM attachment_lww LIMIT 1)",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
        })
        .await
        .expect("inspect typed hash storage");
    assert_eq!(storage, ("blob".into(), 32, "blob".into(), 32));

    let error = database
        .connection
        .call(|sqlite| sqlite.execute("UPDATE attachments SET blob_hash = ?1", ["a".repeat(32)]))
        .await
        .expect_err("text must not satisfy the blob storage constraint");
    assert!(error.to_string().contains("CHECK constraint"));
}

#[tokio::test]
async fn page_layouts_block_styles_tree_and_split_have_stable_typed_ordering() {
    let database = database().await;
    let connection = &database.connection;
    let first_page = db::create_page(connection, "Architecture".into())
        .await
        .expect("create first page");
    let second_page = db::create_page(connection, "Personal".into())
        .await
        .expect("create second page");

    let first_page = db::set_page_layout(connection, first_page.uuid, PageLayout::Document)
        .await
        .expect("set document layout");
    assert_eq!(first_page.layout, PageLayout::Document);

    let first = db::create_block(
        connection,
        first_page.uuid,
        None,
        None,
        BlockStyle::Heading1,
        "Overview".into(),
    )
    .await
    .expect("create heading");
    let last = db::create_block(
        connection,
        first_page.uuid,
        None,
        None,
        BlockStyle::task(TaskState::Todo),
        "Ship it".into(),
    )
    .await
    .expect("create task");
    let middle = db::create_block(
        connection,
        first_page.uuid,
        None,
        Some(first.uuid),
        BlockStyle::Quote,
        "A useful quote".into(),
    )
    .await
    .expect("insert quote after heading");
    let child = db::create_block(
        connection,
        first_page.uuid,
        Some(first.uuid),
        None,
        BlockStyle::Bullet,
        "Nested detail".into(),
    )
    .await
    .expect("create nested block");

    assert_eq!(child.parent_uuid, Some(first.uuid));
    assert_eq!(child.style, BlockStyle::Bullet);
    assert_eq!(
        db::list_block_children(connection, first_page.uuid, None)
            .await
            .expect("list top-level blocks")
            .into_iter()
            .map(|block| block.uuid)
            .collect::<Vec<_>>(),
        vec![first.uuid, middle.uuid, last.uuid]
    );

    let split = db::split_block(
        connection,
        middle.uuid,
        vec![
            BlockContent {
                markdown: "Quote one".into(),
            },
            BlockContent {
                markdown: "Quote two".into(),
            },
            BlockContent {
                markdown: "Quote three".into(),
            },
        ],
    )
    .await
    .expect("split quote");
    assert_eq!(split.len(), 3);
    assert!(split.iter().all(|block| block.style == BlockStyle::Quote));
    assert_eq!(
        split
            .iter()
            .map(|block| block.markdown.as_str())
            .collect::<Vec<_>>(),
        vec!["Quote one", "Quote two", "Quote three"]
    );

    let top_level = db::list_block_children(connection, first_page.uuid, None)
        .await
        .expect("list split result");
    assert_eq!(
        top_level.iter().map(|block| block.uuid).collect::<Vec<_>>(),
        vec![
            first.uuid,
            split[0].uuid,
            split[1].uuid,
            split[2].uuid,
            last.uuid
        ]
    );
    assert!(
        top_level
            .windows(2)
            .all(|pair| pair[0].order_key < pair[1].order_key)
    );

    let foreign_parent = db::create_block(
        connection,
        second_page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "Foreign".into(),
    )
    .await
    .expect("create foreign block");
    let error = db::create_block(
        connection,
        first_page.uuid,
        Some(foreign_parent.uuid),
        None,
        BlockStyle::Paragraph,
        "Invalid child".into(),
    )
    .await
    .expect_err("cross-page parent must be rejected");
    assert!(error.to_string().contains("different page"));

    let error = db::create_block(
        connection,
        first_page.uuid,
        None,
        Some(foreign_parent.uuid),
        BlockStyle::Paragraph,
        "Invalid sibling".into(),
    )
    .await
    .expect_err("cross-page after target must be rejected");
    assert!(error.to_string().contains("not a sibling"));

    let error = db::move_block(connection, first.uuid, Some(foreign_parent.uuid), None)
        .await
        .expect_err("cross-page move must be rejected");
    assert!(error.to_string().contains("different page"));

    let error = db::move_block(connection, first.uuid, None, Some(child.uuid))
        .await
        .expect_err("after target from another sibling set must be rejected");
    assert!(error.to_string().contains("not a sibling"));

    let subtree = db::read_subtree(connection, first_page.uuid, 8)
        .await
        .expect("read page tree");
    assert!(
        subtree
            .iter()
            .all(|content| matches!(content, Content::Block(_)))
    );
    assert!(subtree.iter().any(|content| content.uuid() == child.uuid));
}

#[tokio::test]
async fn every_page_layout_and_block_style_roundtrips_through_sqlite() {
    let database = database().await;
    let connection = &database.connection;
    let page = db::create_page(connection, "Typed variants".into())
        .await
        .expect("create page");

    for layout in [PageLayout::Outline, PageLayout::Document] {
        let updated = db::set_page_layout(connection, page.uuid, layout)
            .await
            .expect("set page layout");
        assert_eq!(updated.layout, layout);
        assert_eq!(
            db::get_page(connection, page.uuid)
                .await
                .expect("read page")
                .expect("page exists")
                .layout,
            layout
        );
    }

    let styles = [
        BlockStyle::Paragraph,
        BlockStyle::Bullet,
        BlockStyle::Numbered,
        BlockStyle::task(TaskState::Todo),
        BlockStyle::task(TaskState::Doing),
        BlockStyle::task(TaskState::Now),
        BlockStyle::task(TaskState::Later),
        BlockStyle::task(TaskState::Done),
        BlockStyle::task(TaskState::Waiting),
        BlockStyle::task(TaskState::Cancelled),
        BlockStyle::Heading1,
        BlockStyle::Heading2,
        BlockStyle::Heading3,
        BlockStyle::Quote,
        BlockStyle::Code,
        BlockStyle::Divider,
    ];
    for (index, style) in styles.into_iter().enumerate() {
        let block = db::create_block(
            connection,
            page.uuid,
            None,
            None,
            style,
            format!("variant {index}"),
        )
        .await
        .expect("create styled block");
        assert_eq!(block.style, style);
        assert_eq!(
            db::get_block(connection, block.uuid)
                .await
                .expect("read block")
                .expect("block exists")
                .style,
            style
        );
    }
}

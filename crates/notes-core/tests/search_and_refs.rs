use notes_core::db::{self, BlockContent, Content, GraphRelation};
use notes_core::{
    BlockCreate, BlockStyle, Connection, Hlc, ObjectKind, Op, OpKind, OrderKey, Origin,
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

fn remote_op(index: u128, wall_ms: u64, kind: OpKind) -> Op {
    let device_id = uuid::Uuid::from_u128(0xCAFE);
    Op {
        op_id: uuid::Uuid::from_u128(0x10_000 + index),
        workspace_uuid: TEST_WORKSPACE_UUID,
        device_id,
        hlc: Hlc::new(wall_ms, 0, device_id),
        format_version: notes_core::operation::FORMAT_VERSION,
        kind,
    }
}

#[tokio::test]
async fn search_normalizes_unicode_treats_wildcards_literally_and_stems_russian() {
    let database = database().await;
    let connection = &database.connection;
    let composed = db::create_page(connection, "Проект Йога".into())
        .await
        .expect("create unicode page");
    let decomposed = db::get_or_create_page_by_title(connection, "  ПРОЕКТ И\u{0306}ОГА  ".into())
        .await
        .expect("look up canonically equivalent title");
    assert_eq!(decomposed.uuid, composed.uuid);

    let literal = db::create_page(connection, "100%_готово".into())
        .await
        .expect("create title with SQL wildcard characters");
    db::create_page(connection, "100xxготово".into())
        .await
        .expect("create wildcard lookalike");
    assert_eq!(
        db::search_pages_by_title(connection, "%_".into(), 10)
            .await
            .expect("literal title search")
            .into_iter()
            .map(|page| page.uuid)
            .collect::<Vec<_>>(),
        vec![literal.uuid]
    );
    assert!(
        db::search_pages_by_title(connection, "   ".into(), 10)
            .await
            .expect("blank title search")
            .is_empty()
    );

    let block = db::create_block(
        connection,
        composed.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "Мы связываем заметки с другими заметками".into(),
    )
    .await
    .expect("create searchable Russian block");
    let hits = db::search_blocks_fts(connection, "заметка".into(), 10)
        .await
        .expect("stemmed FTS search");
    assert!(hits.iter().any(|hit| hit.uuid == block.uuid));
    assert!(
        db::search_blocks_fts(connection, "   ".into(), 10)
            .await
            .expect("blank FTS search")
            .is_empty()
    );

    db::set_block_content(
        connection,
        block.uuid,
        BlockContent {
            markdown: "Архивирование документов".into(),
        },
    )
    .await
    .expect("update indexed block");
    assert!(
        db::search_blocks_fts(connection, "заметка".into(), 10)
            .await
            .expect("search removed terms")
            .is_empty()
    );
    assert_eq!(
        db::search_blocks_fts(connection, "документами".into(), 10)
            .await
            .expect("search replacement terms")
            .into_iter()
            .map(|hit| hit.uuid)
            .collect::<Vec<_>>(),
        vec![block.uuid]
    );
    assert!(
        db::delete_block(connection, block.uuid)
            .await
            .expect("delete indexed block")
    );
    assert!(
        db::search_blocks_fts(connection, "документ".into(), 10)
            .await
            .expect("search deleted block")
            .is_empty()
    );
}

#[tokio::test]
async fn unresolved_page_links_resolve_on_create_and_follow_title_changes() {
    let database = database().await;
    let connection = &database.connection;
    let source_page = db::create_page(connection, "Inbox".into())
        .await
        .expect("create source page");
    let source = db::create_block(
        connection,
        source_page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "See [[  Проект И\u{0306}ога ]]".into(),
    )
    .await
    .expect("create unresolved link");

    let unresolved = connection
        .call(move |sqlite| {
            sqlite.query_row(
                "SELECT target_title, target_page_uuid FROM page_links
                  WHERE source_block_uuid = ?1",
                [source.uuid],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<uuid::Uuid>>(1)?,
                    ))
                },
            )
        })
        .await
        .expect("read unresolved link");
    assert_eq!(unresolved.0, "проект йога");
    assert_eq!(unresolved.1, None);

    let target = db::create_page(connection, "ПРОЕКТ ЙОГА".into())
        .await
        .expect("create link target");
    assert_eq!(
        db::find_backlinks(connection, target.uuid)
            .await
            .expect("resolved backlinks")
            .into_iter()
            .map(|content| content.uuid())
            .collect::<Vec<_>>(),
        vec![source.uuid]
    );

    db::rename_page(connection, target.uuid, Some("Other project".into()))
        .await
        .expect("rename target away");
    assert!(
        db::find_backlinks(connection, target.uuid)
            .await
            .expect("backlinks after rename")
            .is_empty()
    );
    let target_uuid = connection
        .call(move |sqlite| {
            sqlite.query_row(
                "SELECT target_page_uuid FROM page_links WHERE source_block_uuid = ?1",
                [source.uuid],
                |row| row.get::<_, Option<uuid::Uuid>>(0),
            )
        })
        .await
        .expect("read unlinked title reference");
    assert_eq!(target_uuid, None);

    db::rename_page(connection, target.uuid, Some("проект йога".into()))
        .await
        .expect("rename target back");
    assert_eq!(
        db::find_backlinks(connection, target.uuid)
            .await
            .expect("re-resolved backlinks")
            .into_iter()
            .map(|content| content.uuid())
            .collect::<Vec<_>>(),
        vec![source.uuid]
    );

    let snapshot = db::graph_snapshot(connection, Some(target.uuid))
        .await
        .expect("focused graph");
    assert!(snapshot.edges.iter().any(|edge| {
        edge.source_uuid == source.uuid
            && edge.target_uuid == target.uuid
            && edge.relation == GraphRelation::PageLink
    }));

    db::delete_page(connection, target.uuid)
        .await
        .expect("delete target page")
        .expect("target existed");
    let replacement = db::create_page(connection, "ПРОЕКТ ЙОГА".into())
        .await
        .expect("recreate title with a new identity");
    assert_ne!(replacement.uuid, target.uuid);
    assert_eq!(
        db::find_backlinks(connection, replacement.uuid)
            .await
            .expect("link resolves to replacement identity")
            .into_iter()
            .map(|content| content.uuid())
            .collect::<Vec<_>>(),
        vec![source.uuid]
    );
}

#[tokio::test]
async fn workspace_graph_aggregates_block_links_to_owning_pages() {
    let database = database().await;
    let connection = &database.connection;
    let source_page = db::create_page(connection, "Source".into())
        .await
        .expect("create source page");
    let target_page = db::create_page(connection, "Target".into())
        .await
        .expect("create target page");
    let isolated_page = db::create_page(connection, "Isolated".into())
        .await
        .expect("create isolated page");
    let target_block = db::create_block(
        connection,
        target_page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "Target block".into(),
    )
    .await
    .expect("create target block");
    let reference_markdown = format!("[[Target]] and (({}))", target_block.uuid);
    let first_source = db::create_block(
        connection,
        source_page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        reference_markdown.clone(),
    )
    .await
    .expect("create first source block");
    db::create_block(
        connection,
        source_page.uuid,
        None,
        Some(first_source.uuid),
        BlockStyle::Paragraph,
        reference_markdown,
    )
    .await
    .expect("create duplicate source block");

    let workspace = db::graph_snapshot(connection, None)
        .await
        .expect("workspace graph");
    assert_eq!(workspace.items.len(), 3);
    assert!(
        workspace
            .items
            .iter()
            .all(|item| item.kind == ObjectKind::Page)
    );
    assert!(
        workspace
            .items
            .iter()
            .any(|item| item.uuid == isolated_page.uuid)
    );
    assert_eq!(workspace.edges.len(), 2);
    assert!(workspace.edges.iter().any(|edge| {
        edge.source_uuid == source_page.uuid
            && edge.target_uuid == target_page.uuid
            && edge.relation == GraphRelation::PageLink
    }));
    assert!(workspace.edges.iter().any(|edge| {
        edge.source_uuid == source_page.uuid
            && edge.target_uuid == target_page.uuid
            && edge.relation == GraphRelation::BlockReference
    }));

    let focused = db::graph_snapshot(connection, Some(target_block.uuid))
        .await
        .expect("focused block graph");
    assert!(
        focused
            .items
            .iter()
            .any(|item| item.uuid == first_source.uuid && item.kind == ObjectKind::Block)
    );
    assert!(focused.edges.iter().any(|edge| {
        edge.source_uuid == first_source.uuid
            && edge.target_uuid == target_block.uuid
            && edge.relation == GraphRelation::BlockReference
    }));
}

#[tokio::test]
async fn block_references_are_kept_while_dangling_and_removed_with_source_markdown() {
    let database = database().await;
    let connection = &database.connection;
    let page = db::create_page(connection, "References".into())
        .await
        .expect("create page");
    let target_uuid = uuid::Uuid::from_u128(0xB10C);
    let source = db::create_block(
        connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        format!("Before (({target_uuid})) after"),
    )
    .await
    .expect("create dangling reference");

    let reference_count = connection
        .call(move |sqlite| {
            sqlite.query_row(
                "SELECT COUNT(*) FROM block_refs
                  WHERE source_block_uuid = ?1 AND target_block_uuid = ?2",
                rusqlite::params![source.uuid, target_uuid],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .expect("count dangling references");
    assert_eq!(reference_count, 1);

    notes_core::apply(
        connection,
        &remote_op(
            1,
            chrono::Utc::now().timestamp_millis().max(0) as u64 + 10_000,
            OpKind::BlockCreate(BlockCreate {
                uuid: target_uuid,
                page_uuid: page.uuid,
                parent_uuid: None,
                order_key: OrderKey::from_ordinal(2),
                style: BlockStyle::Paragraph,
                markdown: "Target".into(),
                created_at: 1,
            }),
        ),
        Origin::Remote,
    )
    .await
    .expect("materialize reference target");
    assert_eq!(
        db::find_backlinks(connection, target_uuid)
            .await
            .expect("target backlinks")
            .into_iter()
            .map(|content| content.uuid())
            .collect::<Vec<_>>(),
        vec![source.uuid]
    );

    assert!(
        db::delete_block(connection, target_uuid)
            .await
            .expect("delete target")
    );
    assert!(
        db::get_block(connection, target_uuid)
            .await
            .expect("read target")
            .is_none()
    );
    assert_eq!(
        db::find_backlinks(connection, target_uuid)
            .await
            .expect("dangling backlinks remain")
            .into_iter()
            .map(|content| content.uuid())
            .collect::<Vec<_>>(),
        vec![source.uuid]
    );

    let (_, graph_changed) = db::set_block_content(
        connection,
        source.uuid,
        BlockContent {
            markdown: "Reference removed".into(),
        },
    )
    .await
    .expect("remove reference from source");
    assert!(graph_changed);
    assert!(
        db::find_backlinks(connection, target_uuid)
            .await
            .expect("backlinks after source edit")
            .is_empty()
    );
    assert!(matches!(
        db::get_content(connection, source.uuid).await.unwrap(),
        Some(Content::Block(_))
    ));
}

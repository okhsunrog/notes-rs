use notes_core::db::{self, BlockContent, Content, GraphRelation};
use notes_core::{
    BlockCreate, BlockStyle, Connection, Hlc, JournalDate, ObjectKind, Op, OpKind, OrderKey,
    Origin, PageAlias, PageAliasSet, PageCreate, PageDelete, PageKind, PageLayout,
    journal_page_uuid,
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
async fn explicit_alias_lookup_opens_journal_and_survives_delete_recreation() {
    let database = database().await;
    let connection = &database.connection;
    let date = "2026-07-17".parse::<JournalDate>().unwrap();
    let journal_uuid = journal_page_uuid(TEST_WORKSPACE_UUID, &date);
    let alias = PageAlias::new("Jul 17th, 2026").unwrap();
    let source_page = uuid::Uuid::from_u128(0xA11CE);
    let source_block = uuid::Uuid::from_u128(0xB10C);
    notes_core::apply_batch(
        connection,
        &[
            remote_op(
                1,
                1,
                OpKind::PageCreate(PageCreate {
                    uuid: source_page,
                    kind: PageKind::Note,
                    title: Some("Source".into()),
                    layout: PageLayout::Outline,
                    created_at: 1,
                }),
            ),
            remote_op(
                2,
                2,
                OpKind::PageCreate(PageCreate {
                    uuid: journal_uuid,
                    kind: PageKind::Journal { date: date.clone() },
                    title: None,
                    layout: PageLayout::Outline,
                    created_at: 2,
                }),
            ),
            remote_op(
                3,
                3,
                OpKind::PageAliasSet(PageAliasSet {
                    uuid: journal_uuid,
                    alias: alias.clone(),
                    present: true,
                }),
            ),
            remote_op(
                4,
                4,
                OpKind::BlockCreate(BlockCreate {
                    uuid: source_block,
                    page_uuid: source_page,
                    parent_uuid: None,
                    order_key: OrderKey::first(),
                    style: BlockStyle::Paragraph,
                    markdown: "See [[Jul 17th, 2026]]".into(),
                    created_at: 4,
                }),
            ),
        ],
        Origin::Remote,
    )
    .await
    .unwrap();

    let by_alias = db::get_page_by_title(connection, " JUL 17TH, 2026 ".into())
        .await
        .unwrap()
        .expect("journal resolves through explicit alias");
    assert_eq!(by_alias.uuid, journal_uuid);
    assert_eq!(
        db::get_or_create_page_by_title(connection, "Jul 17th, 2026".into())
            .await
            .unwrap()
            .uuid,
        journal_uuid,
        "alias navigation must not create a duplicate note"
    );

    notes_core::apply(
        connection,
        &remote_op(5, 5, OpKind::PageDelete(PageDelete { uuid: journal_uuid })),
        Origin::Remote,
    )
    .await
    .unwrap();
    let target_after_delete = connection
        .call(move |database| {
            database.query_row(
                "SELECT target_page_uuid FROM page_links WHERE source_block_uuid = ?1",
                [source_block],
                |row| row.get::<_, Option<uuid::Uuid>>(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(target_after_delete, None);

    notes_core::apply(
        connection,
        &remote_op(
            6,
            6,
            OpKind::PageCreate(PageCreate {
                uuid: journal_uuid,
                kind: PageKind::Journal { date },
                title: None,
                layout: PageLayout::Outline,
                created_at: 6,
            }),
        ),
        Origin::Remote,
    )
    .await
    .unwrap();
    let target_after_recreate = connection
        .call(move |database| {
            database.query_row(
                "SELECT target_page_uuid FROM page_links WHERE source_block_uuid = ?1",
                [source_block],
                |row| row.get::<_, Option<uuid::Uuid>>(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(target_after_recreate, Some(journal_uuid));
}

#[tokio::test]
async fn colliding_explicit_aliases_are_ambiguous() {
    let database = database().await;
    let connection = &database.connection;
    let first = uuid::Uuid::from_u128(0x101);
    let second = uuid::Uuid::from_u128(0x102);
    let alias = PageAlias::new("shared alias").unwrap();
    notes_core::apply_batch(
        connection,
        &[
            remote_op(
                10,
                10,
                OpKind::PageCreate(PageCreate {
                    uuid: first,
                    kind: PageKind::Note,
                    title: Some("First".into()),
                    layout: PageLayout::Outline,
                    created_at: 10,
                }),
            ),
            remote_op(
                11,
                11,
                OpKind::PageCreate(PageCreate {
                    uuid: second,
                    kind: PageKind::Note,
                    title: Some("Second".into()),
                    layout: PageLayout::Outline,
                    created_at: 11,
                }),
            ),
            remote_op(
                12,
                12,
                OpKind::PageAliasSet(PageAliasSet {
                    uuid: first,
                    alias: alias.clone(),
                    present: true,
                }),
            ),
            remote_op(
                13,
                13,
                OpKind::PageAliasSet(PageAliasSet {
                    uuid: second,
                    alias,
                    present: true,
                }),
            ),
        ],
        Origin::Remote,
    )
    .await
    .unwrap();
    assert!(
        db::get_page_by_title(connection, "Shared Alias".into())
            .await
            .unwrap()
            .is_none()
    );

    notes_core::apply(
        connection,
        &remote_op(
            14,
            14,
            OpKind::PageAliasSet(PageAliasSet {
                uuid: second,
                alias: PageAlias::new("shared alias").unwrap(),
                present: false,
            }),
        ),
        Origin::Remote,
    )
    .await
    .unwrap();
    assert_eq!(
        db::get_page_by_title(connection, "shared alias".into())
            .await
            .unwrap()
            .unwrap()
            .uuid,
        first
    );

    notes_core::apply(
        connection,
        &remote_op(
            15,
            12,
            OpKind::PageAliasSet(PageAliasSet {
                uuid: second,
                alias: PageAlias::new("shared alias").unwrap(),
                present: true,
            }),
        ),
        Origin::Remote,
    )
    .await
    .unwrap();
    assert_eq!(
        db::get_page_by_title(connection, "shared alias".into())
            .await
            .unwrap()
            .unwrap()
            .uuid,
        first,
        "an older alias add cannot defeat a newer removal"
    );
}

#[tokio::test]
async fn title_search_orders_strict_fts_before_substring_fallback() {
    let database = database().await;
    let connection = &database.connection;

    assert_eq!(
        notes_core::stem::stem("Проекты"),
        notes_core::stem::stem("проекта"),
        "the primary morphology fixture must be handled by Snowball"
    );
    let projects = db::create_page(connection, "Проекты".into())
        .await
        .expect("create morphology fixture");
    assert_eq!(
        db::search_pages_by_title(connection, "проекта".into(), 10)
            .await
            .expect("strict stemmed title search")
            .into_iter()
            .map(|page| page.uuid)
            .collect::<Vec<_>>(),
        vec![projects.uuid]
    );

    let firmware = db::create_page(connection, "Прошивка ESP32".into())
        .await
        .expect("create multi-token title");
    assert_eq!(
        db::search_pages_by_title(connection, "esp32 прошивка".into(), 10)
            .await
            .expect("order-independent title search")
            .into_iter()
            .map(|page| page.uuid)
            .collect::<Vec<_>>(),
        vec![firmware.uuid]
    );
    assert_eq!(
        db::search_pages_by_title(connection, "шивк".into(), 10)
            .await
            .expect("mid-word substring fallback")
            .into_iter()
            .map(|page| page.uuid)
            .collect::<Vec<_>>(),
        vec![firmware.uuid]
    );

    let exact = db::create_page(connection, "ESP32".into())
        .await
        .expect("create exact-match title");
    assert_eq!(
        db::search_pages_by_title(connection, "esp32".into(), 10)
            .await
            .expect("exact title search")
            .first()
            .map(|page| page.uuid),
        Some(exact.uuid)
    );
}

#[tokio::test]
async fn title_search_relaxes_only_after_strict_and_substring_miss() {
    let database = database().await;
    let connection = &database.connection;

    let purchases = db::create_page(connection, "Покупки".into())
        .await
        .expect("create relaxed morphology fixture");
    let buyer = db::create_page(connection, "Покупатель".into())
        .await
        .expect("create related relaxed-prefix fixture");
    let relaxed_hits = db::search_pages_by_title(connection, "покупок".into(), 10)
        .await
        .expect("relaxed title search")
        .into_iter()
        .map(|page| page.uuid)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        relaxed_hits,
        std::collections::HashSet::from([purchases.uuid, buyer.uuid]),
        "the bounded покуп* family is intentionally relevant at the relaxed tier"
    );

    db::create_page(connection, "Абвге".into())
        .await
        .expect("create short-token guard fixture");
    assert!(
        db::search_pages_by_title(connection, "абвгд".into(), 10)
            .await
            .expect("short title search")
            .is_empty(),
        "five-character Cyrillic tokens must not be relaxed"
    );

    let strict = db::create_page(connection, "Покупок".into())
        .await
        .expect("create strict-hit guard fixture");
    assert_eq!(
        db::search_pages_by_title(connection, "покупок".into(), 10)
            .await
            .expect("strict title search after relaxed fixture")
            .into_iter()
            .map(|page| page.uuid)
            .collect::<Vec<_>>(),
        vec![strict.uuid],
        "a strict hit must prevent the relaxed tier from contributing results"
    );
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
    let hits = db::search_blocks_fts(
        connection,
        "заметка".into(),
        10,
        notes_core::SearchTokenMode::Plain,
    )
    .await
    .expect("stemmed FTS search");
    assert!(hits.iter().any(|hit| hit.uuid == block.uuid));
    assert!(
        db::search_blocks_fts(
            connection,
            "зам".into(),
            10,
            notes_core::SearchTokenMode::Prefix,
        )
        .await
        .expect("prefix FTS search")
        .iter()
        .any(|hit| hit.uuid == block.uuid)
    );
    assert!(
        db::search_blocks_fts(
            connection,
            "   ".into(),
            10,
            notes_core::SearchTokenMode::Plain,
        )
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
        db::search_blocks_fts(
            connection,
            "заметка".into(),
            10,
            notes_core::SearchTokenMode::Plain,
        )
        .await
        .expect("search removed terms")
        .is_empty()
    );
    assert_eq!(
        db::search_blocks_fts(
            connection,
            "документами".into(),
            10,
            notes_core::SearchTokenMode::Plain,
        )
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
        db::search_blocks_fts(
            connection,
            "документ".into(),
            10,
            notes_core::SearchTokenMode::Plain,
        )
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

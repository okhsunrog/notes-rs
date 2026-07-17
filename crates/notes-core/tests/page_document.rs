use notes_core::db::{
    self, BlockContent, DocumentUnitDraft, MAX_DOCUMENT_DEPTH, MAX_DOCUMENT_MARKDOWN_BYTES,
    MAX_DOCUMENT_UNITS, PageDocumentSnapshot,
};
use notes_core::{
    AttachmentOwner, BlobHash, BlockStyle, Connection, CoreError, DocumentRevision, OpKind, Origin,
};
use std::collections::HashMap;

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

async fn revision(connection: &Connection, page_uuid: uuid::Uuid) -> DocumentRevision {
    db::get_page_document(connection, page_uuid)
        .await
        .expect("read page document")
        .expect("page exists")
        .revision
}

fn drafts(snapshot: &PageDocumentSnapshot) -> Vec<DocumentUnitDraft> {
    let indices = snapshot
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.uuid, index as u32))
        .collect::<HashMap<_, _>>();
    snapshot
        .blocks
        .iter()
        .map(|block| DocumentUnitDraft {
            previous_uuid: Some(block.uuid),
            parent_index: block.parent_uuid.map(|uuid| indices[&uuid]),
            style: block.style,
            markdown: block.markdown.clone(),
        })
        .collect()
}

fn semantic_tree(
    snapshot: &PageDocumentSnapshot,
) -> Vec<(uuid::Uuid, Option<uuid::Uuid>, String, BlockStyle, String)> {
    snapshot
        .blocks
        .iter()
        .map(|block| {
            (
                block.uuid,
                block.parent_uuid,
                block.order_key.to_string(),
                block.style,
                block.markdown.clone(),
            )
        })
        .collect()
}

async fn mutation_counts(connection: &Connection) -> (i64, i64, i64, i64) {
    connection
        .call(|database| {
            database.query_row(
                "SELECT (SELECT COUNT(*) FROM blocks),
                        (SELECT COUNT(*) FROM applied_ops),
                        (SELECT COUNT(*) FROM sync_outbox),
                        (SELECT COUNT(*) FROM history_undo)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })
        .await
        .expect("read mutation counts")
}

fn assert_invalid(error: &anyhow::Error) {
    assert!(
        matches!(
            error.downcast_ref::<CoreError>(),
            Some(CoreError::InvalidInput(_))
        ),
        "expected invalid input, got {error:#}"
    );
}

fn assert_conflict(error: &anyhow::Error) {
    assert!(
        matches!(
            error.downcast_ref::<CoreError>(),
            Some(CoreError::Conflict(_))
        ),
        "expected conflict, got {error:#}"
    );
}

#[test]
fn document_revision_requires_canonical_lowercase_sha256_hex() {
    let canonical = "0123456789abcdef".repeat(4);
    let parsed = canonical
        .parse::<DocumentRevision>()
        .expect("canonical revision");
    assert_eq!(parsed.as_str(), canonical);
    assert_eq!(parsed.to_string(), canonical);
    assert!(
        canonical
            .to_uppercase()
            .parse::<DocumentRevision>()
            .is_err()
    );
    assert!("a".repeat(63).parse::<DocumentRevision>().is_err());
    assert!("a".repeat(65).parse::<DocumentRevision>().is_err());
    assert!(
        format!("{}g", "a".repeat(63))
            .parse::<DocumentRevision>()
            .is_err()
    );
    assert!(
        serde_json::from_str::<DocumentRevision>(&format!("\"{}\"", canonical.to_uppercase()))
            .is_err()
    );
}

#[tokio::test]
async fn empty_page_has_a_document_snapshot_and_missing_page_does_not() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Empty".into())
        .await
        .expect("create page");
    let snapshot = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read page")
        .expect("page exists");
    assert_eq!(snapshot.page_uuid, page.uuid);
    assert!(snapshot.blocks.is_empty());
    assert_eq!(snapshot.revision.as_str().len(), 64);
    assert!(
        db::get_page_document(&database.connection, uuid::Uuid::now_v7())
            .await
            .expect("read missing page")
            .is_none()
    );
}

#[tokio::test]
async fn blocks_are_returned_in_deterministic_full_tree_preorder() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Tree".into())
        .await
        .expect("create page");
    let first = db::create_block(
        &database.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Heading1,
        "first".into(),
    )
    .await
    .expect("create first root");
    let second = db::create_block(
        &database.connection,
        page.uuid,
        None,
        Some(first.uuid),
        BlockStyle::Paragraph,
        "second".into(),
    )
    .await
    .expect("create second root");
    let child = db::create_block(
        &database.connection,
        page.uuid,
        Some(first.uuid),
        None,
        BlockStyle::Bullet,
        "child".into(),
    )
    .await
    .expect("create child");
    let grandchild = db::create_block(
        &database.connection,
        page.uuid,
        Some(child.uuid),
        None,
        BlockStyle::Numbered,
        "grandchild".into(),
    )
    .await
    .expect("create grandchild");

    let snapshot = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read page")
        .expect("page exists");
    assert_eq!(
        snapshot
            .blocks
            .iter()
            .map(|block| block.uuid)
            .collect::<Vec<_>>(),
        vec![first.uuid, child.uuid, grandchild.uuid, second.uuid]
    );
}

#[tokio::test]
async fn revision_tracks_document_changes_but_ignores_title_and_attachments() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Original title".into())
        .await
        .expect("create page");
    let first = db::create_block(
        &database.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "first".into(),
    )
    .await
    .expect("create first block");
    let second = db::create_block(
        &database.connection,
        page.uuid,
        None,
        Some(first.uuid),
        BlockStyle::Paragraph,
        "second".into(),
    )
    .await
    .expect("create second block");
    let mut previous = revision(&database.connection, page.uuid).await;

    db::set_block_content(
        &database.connection,
        first.uuid,
        BlockContent {
            markdown: "changed".into(),
        },
    )
    .await
    .expect("change markdown");
    let changed_markdown = revision(&database.connection, page.uuid).await;
    assert_ne!(changed_markdown, previous);
    previous = changed_markdown;

    db::set_block_style(&database.connection, first.uuid, BlockStyle::Heading2)
        .await
        .expect("change style");
    let changed_style = revision(&database.connection, page.uuid).await;
    assert_ne!(changed_style, previous);
    previous = changed_style;

    db::move_block(&database.connection, second.uuid, Some(first.uuid), None)
        .await
        .expect("move block");
    let changed_structure = revision(&database.connection, page.uuid).await;
    assert_ne!(changed_structure, previous);
    previous = changed_structure;

    let created = db::create_block(
        &database.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Quote,
        "temporary".into(),
    )
    .await
    .expect("create block");
    let changed_create = revision(&database.connection, page.uuid).await;
    assert_ne!(changed_create, previous);
    previous = changed_create;

    assert!(
        db::delete_block(&database.connection, created.uuid)
            .await
            .expect("delete block")
    );
    let changed_delete = revision(&database.connection, page.uuid).await;
    assert_ne!(changed_delete, previous);

    db::rename_page(
        &database.connection,
        page.uuid,
        Some("Renamed without changing document".into()),
    )
    .await
    .expect("rename page");
    assert_eq!(
        revision(&database.connection, page.uuid).await,
        changed_delete
    );

    db::create_attachment(
        &database.connection,
        AttachmentOwner::Page(page.uuid),
        BlobHash::from_bytes([0x5a; 32]),
        "reference.pdf".into(),
        "application/pdf".into(),
        42,
    )
    .await
    .expect("create attachment");
    assert_eq!(
        revision(&database.connection, page.uuid).await,
        changed_delete
    );
}

#[tokio::test]
async fn identical_replicas_produce_the_same_document_revision() {
    let source = database().await;
    let destination = database().await;
    let page = db::create_page(&source.connection, "Replica".into())
        .await
        .expect("create page");
    let root = db::create_block(
        &source.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Heading1,
        "root".into(),
    )
    .await
    .expect("create root");
    db::create_block(
        &source.connection,
        page.uuid,
        Some(root.uuid),
        None,
        BlockStyle::Bullet,
        "child".into(),
    )
    .await
    .expect("create child");

    let sync_snapshot = notes_core::export_sync_snapshot(&source.connection, 0)
        .await
        .expect("export sync snapshot");
    notes_core::import_sync_snapshot(&destination.connection, sync_snapshot)
        .await
        .expect("import sync snapshot");

    let left = db::get_page_document(&source.connection, page.uuid)
        .await
        .expect("read source")
        .expect("source page exists");
    let right = db::get_page_document(&destination.connection, page.uuid)
        .await
        .expect("read destination")
        .expect("destination page exists");
    assert_eq!(left.blocks, right.blocks);
    assert_eq!(left.revision, right.revision);
}

#[tokio::test]
async fn exact_document_replace_is_a_zero_mutation_noop() {
    let database = database().await;
    let note = db::create_note(&database.connection)
        .await
        .expect("create note");
    db::set_block_content(
        &database.connection,
        note.initial_block.uuid,
        BlockContent {
            markdown: "unchanged".into(),
        },
    )
    .await
    .expect("seed content");
    notes_core::configure_sync(&database.connection, "https://sync.example")
        .await
        .expect("configure sync");
    let before = db::get_page_document(&database.connection, note.page.uuid)
        .await
        .expect("read document")
        .expect("page exists");
    let counts = mutation_counts(&database.connection).await;

    let outcome = db::replace_page_document_with_outcome(
        &database.connection,
        note.page.uuid,
        before.revision.clone(),
        drafts(&before),
    )
    .await
    .expect("replace with exact snapshot");

    assert!(!outcome.changed);
    assert!(outcome.block_uuids.is_empty());
    assert_eq!(outcome.snapshot, before);
    assert_eq!(mutation_counts(&database.connection).await, counts);
    assert!(
        notes_core::pending_outbox(&database.connection, 100)
            .await
            .expect("read outbox")
            .is_empty()
    );
}

#[tokio::test]
async fn exact_replace_preserves_fractional_order_keys_without_mutations() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Fractional order".into())
        .await
        .expect("create page");
    let first = db::create_block(
        &database.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "first".into(),
    )
    .await
    .expect("create first");
    let second = db::create_block(
        &database.connection,
        page.uuid,
        None,
        Some(first.uuid),
        BlockStyle::Paragraph,
        "second".into(),
    )
    .await
    .expect("create second");
    database
        .connection
        .call(move |connection| {
            let transaction = connection.transaction()?;
            for (uuid, order_key) in [
                (first.uuid, "0000000180000000"),
                (second.uuid, "0000000380000000"),
            ] {
                transaction.execute(
                    "UPDATE blocks SET order_key = ?2 WHERE uuid = ?1",
                    rusqlite::params![uuid, order_key],
                )?;
                transaction.execute(
                    "UPDATE block_structure_lww SET order_key = ?2 WHERE block_uuid = ?1",
                    rusqlite::params![uuid, order_key],
                )?;
            }
            transaction.commit()
        })
        .await
        .expect("seed fractional keys");
    notes_core::configure_sync(&database.connection, "https://sync.example")
        .await
        .expect("configure sync");
    let before = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read before")
        .expect("page exists");
    let counts = mutation_counts(&database.connection).await;

    let outcome = db::replace_page_document_with_outcome(
        &database.connection,
        page.uuid,
        before.revision.clone(),
        drafts(&before),
    )
    .await
    .expect("replace exact fractional projection");

    assert!(!outcome.changed);
    assert_eq!(outcome.snapshot, before);
    assert_eq!(mutation_counts(&database.connection).await, counts);
    assert!(
        notes_core::pending_outbox(&database.connection, 100)
            .await
            .expect("read outbox")
            .is_empty()
    );
}

#[tokio::test]
async fn mixed_replace_assigns_uuidv7_and_undo_restores_the_exact_semantic_tree() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Mixed".into())
        .await
        .expect("create page");
    let first = db::create_block(
        &database.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "first".into(),
    )
    .await
    .expect("create first");
    let child = db::create_block(
        &database.connection,
        page.uuid,
        Some(first.uuid),
        None,
        BlockStyle::Bullet,
        "child".into(),
    )
    .await
    .expect("create child");
    let removed = db::create_block(
        &database.connection,
        page.uuid,
        None,
        Some(first.uuid),
        BlockStyle::Quote,
        "removed".into(),
    )
    .await
    .expect("create removed block");
    let before = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read before")
        .expect("page exists");
    let history_before = db::history_status(&database.connection)
        .await
        .expect("history before");

    let outcome = db::replace_page_document_with_outcome(
        &database.connection,
        page.uuid,
        before.revision.clone(),
        vec![
            DocumentUnitDraft {
                previous_uuid: Some(child.uuid),
                parent_index: None,
                style: BlockStyle::Numbered,
                markdown: "child updated".into(),
            },
            DocumentUnitDraft {
                previous_uuid: Some(first.uuid),
                parent_index: None,
                style: BlockStyle::Heading1,
                markdown: "first updated".into(),
            },
            DocumentUnitDraft {
                previous_uuid: None,
                parent_index: None,
                style: BlockStyle::Quote,
                markdown: "new parent".into(),
            },
            DocumentUnitDraft {
                previous_uuid: None,
                parent_index: Some(2),
                style: BlockStyle::Bullet,
                markdown: "new child".into(),
            },
        ],
    )
    .await
    .expect("mixed replace");

    assert!(outcome.changed);
    assert!(outcome.structure_changed);
    assert!(outcome.graph_changed);
    assert!(outcome.deleted_block_uuids.contains(&removed.uuid));
    assert_eq!(outcome.snapshot.blocks.len(), 4);
    assert_eq!(outcome.snapshot.blocks[0].uuid, child.uuid);
    assert_eq!(outcome.snapshot.blocks[1].uuid, first.uuid);
    let new_parent = &outcome.snapshot.blocks[2];
    let new_child = &outcome.snapshot.blocks[3];
    assert_eq!(new_parent.uuid.get_version_num(), 7);
    assert_eq!(new_child.uuid.get_version_num(), 7);
    assert_eq!(new_child.parent_uuid, Some(new_parent.uuid));
    assert_eq!(
        db::history_status(&database.connection)
            .await
            .expect("history after")
            .undo_count,
        history_before.undo_count + 1
    );
    let latest_action = database
        .connection
        .call(|connection| {
            connection.query_row(
                "SELECT action FROM history_undo ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
        })
        .await
        .expect("read latest history action");
    assert_eq!(latest_action, "edit document");

    let counts_after_edit = mutation_counts(&database.connection).await;
    let exact_retry = db::replace_page_document_with_outcome(
        &database.connection,
        page.uuid,
        outcome.snapshot.revision.clone(),
        drafts(&outcome.snapshot),
    )
    .await
    .expect("repeat the exact mixed result");
    assert!(!exact_retry.changed);
    assert_eq!(exact_retry.snapshot, outcome.snapshot);
    assert_eq!(
        mutation_counts(&database.connection).await,
        counts_after_edit
    );

    assert!(
        db::undo_history(&database.connection)
            .await
            .expect("undo replacement")
    );
    let restored = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read restored")
        .expect("page exists");
    assert_eq!(semantic_tree(&restored), semantic_tree(&before));
}

#[tokio::test]
async fn retained_child_moves_before_its_deleted_parent() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Move from deletion".into())
        .await
        .expect("create page");
    let parent = db::create_block(
        &database.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "parent".into(),
    )
    .await
    .expect("create parent");
    let child = db::create_block(
        &database.connection,
        page.uuid,
        Some(parent.uuid),
        None,
        BlockStyle::Paragraph,
        "child".into(),
    )
    .await
    .expect("create child");
    let before = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read before")
        .expect("page exists");

    let replaced = db::replace_page_document(
        &database.connection,
        page.uuid,
        before.revision,
        vec![DocumentUnitDraft {
            previous_uuid: Some(child.uuid),
            parent_index: None,
            style: child.style,
            markdown: child.markdown.clone(),
        }],
    )
    .await
    .expect("retain child and remove parent");
    assert_eq!(replaced.blocks.len(), 1);
    assert_eq!(replaced.blocks[0].uuid, child.uuid);
    assert_eq!(replaced.blocks[0].parent_uuid, None);
    assert!(
        db::get_block(&database.connection, parent.uuid)
            .await
            .expect("read parent")
            .is_none()
    );
}

#[tokio::test]
async fn stale_and_invalid_replacements_leave_every_mutation_surface_unchanged() {
    let database = database().await;
    let page = db::create_page(&database.connection, "Validation".into())
        .await
        .expect("create page");
    let block = db::create_block(
        &database.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "current".into(),
    )
    .await
    .expect("create block");
    let foreign_page = db::create_page(&database.connection, "Foreign".into())
        .await
        .expect("create foreign page");
    let foreign = db::create_block(
        &database.connection,
        foreign_page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "foreign".into(),
    )
    .await
    .expect("create foreign block");
    notes_core::configure_sync(&database.connection, "https://sync.example")
        .await
        .expect("configure sync");
    let current = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read current")
        .expect("page exists");

    db::set_block_content(
        &database.connection,
        block.uuid,
        BlockContent {
            markdown: "remote".into(),
        },
    )
    .await
    .expect("advance revision");
    let before_stale = mutation_counts(&database.connection).await;
    let stale = db::replace_page_document(
        &database.connection,
        page.uuid,
        current.revision,
        vec![DocumentUnitDraft {
            previous_uuid: Some(block.uuid),
            parent_index: None,
            style: block.style,
            markdown: "stale overwrite".into(),
        }],
    )
    .await
    .expect_err("stale replace must fail");
    assert_conflict(&stale);
    assert_eq!(mutation_counts(&database.connection).await, before_stale);

    let current = db::get_page_document(&database.connection, page.uuid)
        .await
        .expect("read latest")
        .expect("page exists");
    let invalid_cases = vec![
        vec![
            DocumentUnitDraft {
                previous_uuid: Some(block.uuid),
                parent_index: None,
                style: block.style,
                markdown: "one".into(),
            },
            DocumentUnitDraft {
                previous_uuid: None,
                parent_index: Some(1),
                style: BlockStyle::Paragraph,
                markdown: "late parent".into(),
            },
        ],
        vec![
            DocumentUnitDraft {
                previous_uuid: Some(block.uuid),
                parent_index: None,
                style: block.style,
                markdown: "one".into(),
            },
            DocumentUnitDraft {
                previous_uuid: Some(block.uuid),
                parent_index: None,
                style: block.style,
                markdown: "duplicate".into(),
            },
        ],
        vec![DocumentUnitDraft {
            previous_uuid: Some(foreign.uuid),
            parent_index: None,
            style: foreign.style,
            markdown: foreign.markdown.clone(),
        }],
    ];
    for units in invalid_cases {
        let counts = mutation_counts(&database.connection).await;
        let error = db::replace_page_document(
            &database.connection,
            page.uuid,
            current.revision.clone(),
            units,
        )
        .await
        .expect_err("invalid replacement must fail");
        assert_invalid(&error);
        assert_eq!(mutation_counts(&database.connection).await, counts);
    }

    let limit_cases = vec![
        vec![
            DocumentUnitDraft {
                previous_uuid: None,
                parent_index: None,
                style: BlockStyle::Paragraph,
                markdown: String::new(),
            };
            MAX_DOCUMENT_UNITS + 1
        ],
        vec![DocumentUnitDraft {
            previous_uuid: None,
            parent_index: None,
            style: BlockStyle::Paragraph,
            markdown: "x".repeat(MAX_DOCUMENT_MARKDOWN_BYTES + 1),
        }],
        (0..=MAX_DOCUMENT_DEPTH)
            .map(|index| DocumentUnitDraft {
                previous_uuid: None,
                parent_index: index.checked_sub(1),
                style: BlockStyle::Paragraph,
                markdown: String::new(),
            })
            .collect(),
    ];
    for units in limit_cases {
        let counts = mutation_counts(&database.connection).await;
        let error = db::replace_page_document(
            &database.connection,
            page.uuid,
            current.revision.clone(),
            units,
        )
        .await
        .expect_err("limit violation must fail");
        assert_invalid(&error);
        assert_eq!(mutation_counts(&database.connection).await, counts);
    }
}

#[tokio::test]
async fn replacement_emits_only_needed_ops_and_converges_through_sync() {
    let source = database().await;
    let destination = database().await;
    let page = db::create_page(&source.connection, "Sync document".into())
        .await
        .expect("create page");
    let first = db::create_block(
        &source.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Paragraph,
        "first".into(),
    )
    .await
    .expect("create first");
    let second = db::create_block(
        &source.connection,
        page.uuid,
        None,
        Some(first.uuid),
        BlockStyle::Bullet,
        "second".into(),
    )
    .await
    .expect("create second");
    let bootstrap = notes_core::export_sync_snapshot(&source.connection, 0)
        .await
        .expect("export bootstrap");
    notes_core::import_sync_snapshot(&destination.connection, bootstrap)
        .await
        .expect("import bootstrap");
    notes_core::configure_sync(&source.connection, "https://sync.example")
        .await
        .expect("configure source sync");
    let before = db::get_page_document(&source.connection, page.uuid)
        .await
        .expect("read source before")
        .expect("page exists");

    let replaced = db::replace_page_document(
        &source.connection,
        page.uuid,
        before.revision,
        vec![
            DocumentUnitDraft {
                previous_uuid: Some(second.uuid),
                parent_index: None,
                style: second.style,
                markdown: second.markdown.clone(),
            },
            DocumentUnitDraft {
                previous_uuid: Some(first.uuid),
                parent_index: None,
                style: first.style,
                markdown: first.markdown.clone(),
            },
        ],
    )
    .await
    .expect("reorder only");
    let operations = notes_core::pending_outbox(&source.connection, 100)
        .await
        .expect("read replacement ops");
    assert!(!operations.is_empty());
    assert!(
        operations
            .iter()
            .all(|operation| matches!(operation.kind, OpKind::BlockMove(_)))
    );

    notes_core::apply_batch(&destination.connection, &operations, Origin::Remote)
        .await
        .expect("apply replacement ops");
    let converged = db::get_page_document(&destination.connection, page.uuid)
        .await
        .expect("read destination")
        .expect("destination page exists");
    assert_eq!(semantic_tree(&converged), semantic_tree(&replaced));
    assert_eq!(converged.revision, replaced.revision);
}

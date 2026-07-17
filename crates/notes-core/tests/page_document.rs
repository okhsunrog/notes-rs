use notes_core::db::{self, BlockContent};
use notes_core::{AttachmentOwner, BlobHash, BlockStyle, Connection, DocumentRevision};

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

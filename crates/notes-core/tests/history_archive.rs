use notes_core::db::{self, DataArchive};
use notes_core::{AttachmentOwner, BlockStyle, Connection, PageView};

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

fn hash(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

#[tokio::test]
async fn undo_and_redo_restore_a_deleted_page_subtree_and_attachments_exactly() {
    let database = database().await;
    let connection = &database.connection;
    let note = db::create_note(connection).await.expect("create note");
    let root = db::set_block_style(connection, note.initial_block.uuid, BlockStyle::Heading1)
        .await
        .expect("style root");
    let child = db::create_block(
        connection,
        note.page.uuid,
        Some(root.uuid),
        None,
        BlockStyle::Bullet,
        "Nested".into(),
    )
    .await
    .expect("create child");
    let page_attachment = db::create_attachment(
        connection,
        AttachmentOwner::Page(note.page.uuid),
        hash('a'),
        "page.pdf".into(),
        "application/pdf".into(),
        42,
    )
    .await
    .expect("attach to page");
    let block_attachment = db::create_attachment(
        connection,
        AttachmentOwner::Block(child.uuid),
        hash('b'),
        "diagram.svg".into(),
        "image/svg+xml".into(),
        84,
    )
    .await
    .expect("attach to child");

    db::delete_page(connection, note.page.uuid)
        .await
        .expect("delete page")
        .expect("page existed");
    assert!(
        db::get_page(connection, note.page.uuid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db::get_block(connection, root.uuid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db::get_block(connection, child.uuid)
            .await
            .unwrap()
            .is_none()
    );

    assert!(db::undo_history(connection).await.expect("undo deletion"));
    let restored_page = db::get_page(connection, note.page.uuid)
        .await
        .expect("read restored page")
        .expect("page restored");
    assert_eq!(restored_page.default_view, PageView::Outline);
    let restored_root = db::get_block(connection, root.uuid)
        .await
        .expect("read restored root")
        .expect("root restored");
    let restored_child = db::get_block(connection, child.uuid)
        .await
        .expect("read restored child")
        .expect("child restored");
    assert_eq!(restored_root.style, BlockStyle::Heading1);
    assert_eq!(restored_child.parent_uuid, Some(restored_root.uuid));
    assert_eq!(
        db::list_attachments(connection, AttachmentOwner::Page(note.page.uuid))
            .await
            .expect("restored page attachments")
            .into_iter()
            .map(|attachment| attachment.uuid)
            .collect::<Vec<_>>(),
        vec![page_attachment.uuid]
    );
    assert_eq!(
        db::list_attachments(connection, AttachmentOwner::Block(child.uuid))
            .await
            .expect("restored block attachments")
            .into_iter()
            .map(|attachment| attachment.uuid)
            .collect::<Vec<_>>(),
        vec![block_attachment.uuid]
    );

    assert!(db::redo_history(connection).await.expect("redo deletion"));
    assert!(
        db::get_page(connection, note.page.uuid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db::get_block(connection, root.uuid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db::get_block(connection, child.uuid)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(db::history_status(connection).await.unwrap().redo_count, 0);
}

#[tokio::test]
async fn archive_roundtrip_replaces_typed_content_and_resets_incompatible_history() {
    let source = database().await;
    let page = db::create_page(&source.connection, "Long document".into())
        .await
        .expect("create source page");
    db::set_page_view(&source.connection, page.uuid, PageView::Reading)
        .await
        .expect("set reading view");
    let root = db::create_block(
        &source.connection,
        page.uuid,
        None,
        None,
        BlockStyle::Heading2,
        "Chapter".into(),
    )
    .await
    .expect("create source block");
    let attachment = db::create_attachment(
        &source.connection,
        AttachmentOwner::Block(root.uuid),
        hash('c'),
        "chapter.txt".into(),
        "text/plain".into(),
        7,
    )
    .await
    .expect("create source attachment");
    let archive = db::export_archive(&source.connection)
        .await
        .expect("export archive");

    let destination = database().await;
    db::create_page(&destination.connection, "Stale page".into())
        .await
        .expect("create destination history and stale state");
    assert_ne!(
        db::history_status(&destination.connection).await.unwrap(),
        db::HistoryStatus {
            undo_count: 0,
            redo_count: 0,
        }
    );
    db::import_archive(&destination.connection, archive)
        .await
        .expect("import archive");

    let pages = db::list_pages(&destination.connection, 100)
        .await
        .expect("list imported pages");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].uuid, page.uuid);
    assert_eq!(pages[0].title.as_deref(), Some("Long document"));
    assert_eq!(pages[0].default_view, PageView::Reading);
    let imported_root = db::get_block(&destination.connection, root.uuid)
        .await
        .expect("read imported block")
        .expect("block imported");
    assert_eq!(imported_root.style, BlockStyle::Heading2);
    assert_eq!(imported_root.markdown, "Chapter");
    assert_eq!(
        db::list_attachments(&destination.connection, AttachmentOwner::Block(root.uuid))
            .await
            .expect("list imported attachments")
            .into_iter()
            .map(|record| (record.uuid, record.owner))
            .collect::<Vec<_>>(),
        vec![(attachment.uuid, AttachmentOwner::Block(root.uuid))]
    );
    assert_eq!(
        db::history_status(&destination.connection)
            .await
            .expect("history after state replacement"),
        db::HistoryStatus {
            undo_count: 0,
            redo_count: 0,
        },
        "archive replacement must not retain undo entries for the discarded database"
    );

    let roundtrip = db::export_archive(&destination.connection)
        .await
        .expect("re-export archive");
    assert_archive_semantics(&roundtrip, page.uuid, root.uuid, attachment.uuid);
}

fn assert_archive_semantics(
    archive: &DataArchive,
    page_uuid: uuid::Uuid,
    block_uuid: uuid::Uuid,
    attachment_uuid: uuid::Uuid,
) {
    assert_eq!(archive.format, "notes-rs");
    assert_eq!(archive.version, 2);
    assert_eq!(archive.pages.len(), 1);
    assert_eq!(archive.pages[0].uuid, page_uuid);
    assert_eq!(archive.blocks.len(), 1);
    assert_eq!(archive.blocks[0].uuid, block_uuid);
    assert_eq!(archive.attachments.len(), 1);
    assert_eq!(archive.attachments[0].uuid, attachment_uuid);
}

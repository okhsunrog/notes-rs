use notes_core::db::{self, DataArchive};
use notes_core::{AttachmentOwner, BlockStyle, Connection, PageLayout, TaskState};

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
async fn task_state_updates_are_typed_undoable_and_reject_non_tasks() {
    let database = database().await;
    let note = db::create_note(&database.connection)
        .await
        .expect("create note");

    let error = db::set_task_state(
        &database.connection,
        note.initial_block.uuid,
        TaskState::Done,
    )
    .await
    .expect_err("a paragraph cannot carry task state");
    assert!(error.to_string().contains("requires a task block"));

    db::set_block_style(
        &database.connection,
        note.initial_block.uuid,
        BlockStyle::task(TaskState::Now),
    )
    .await
    .expect("turn block into task");
    let completed = db::set_task_state(
        &database.connection,
        note.initial_block.uuid,
        TaskState::Done,
    )
    .await
    .expect("complete task");
    assert_eq!(completed.style, BlockStyle::task(TaskState::Done));

    assert!(
        db::undo_history(&database.connection)
            .await
            .expect("undo state")
    );
    assert_eq!(
        db::get_block(&database.connection, note.initial_block.uuid)
            .await
            .expect("read task")
            .expect("task exists")
            .style,
        BlockStyle::task(TaskState::Now)
    );
    assert!(
        db::redo_history(&database.connection)
            .await
            .expect("redo state")
    );
    assert_eq!(
        db::get_block(&database.connection, note.initial_block.uuid)
            .await
            .expect("read task")
            .expect("task exists")
            .style,
        BlockStyle::task(TaskState::Done)
    );
}

#[tokio::test]
async fn archive_roundtrip_preserves_task_state_inside_block_style() {
    let source = database().await;
    let note = db::create_note(&source.connection)
        .await
        .expect("create note");
    db::set_block_style(
        &source.connection,
        note.initial_block.uuid,
        BlockStyle::task(TaskState::Waiting),
    )
    .await
    .expect("set waiting task");
    let archive = db::export_archive(&source.connection)
        .await
        .expect("export archive");

    let destination = database().await;
    let workspace_uuid = archive.workspace_uuid;
    destination
        .connection
        .call(move |database| {
            database.execute(
                "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                [workspace_uuid],
            )?;
            Ok(())
        })
        .await
        .expect("align workspace");
    db::import_archive(&destination.connection, archive)
        .await
        .expect("import archive");

    assert_eq!(
        db::get_block(&destination.connection, note.initial_block.uuid)
            .await
            .expect("read imported task")
            .expect("task imported")
            .style,
        BlockStyle::task(TaskState::Waiting)
    );
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
    assert_eq!(restored_page.layout, PageLayout::Outline);
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
    db::set_page_layout(&source.connection, page.uuid, PageLayout::Document)
        .await
        .expect("set document layout");
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
    let source_workspace_uuid = archive.workspace_uuid;

    let destination = database().await;
    destination
        .connection
        .call(move |database| {
            database.execute(
                "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
                [source_workspace_uuid],
            )?;
            Ok(())
        })
        .await
        .expect("align restore workspace");
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
    assert_eq!(pages[0].layout, PageLayout::Document);
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
    assert_eq!(roundtrip.workspace_uuid, source_workspace_uuid);
    assert_archive_semantics(&roundtrip, page.uuid, root.uuid, attachment.uuid);
}

fn assert_archive_semantics(
    archive: &DataArchive,
    page_uuid: uuid::Uuid,
    block_uuid: uuid::Uuid,
    attachment_uuid: uuid::Uuid,
) {
    assert_eq!(archive.format, "notes-rs");
    assert_eq!(archive.version, 5);
    assert!(
        archive
            .page_identities
            .iter()
            .any(|identity| identity.uuid == page_uuid),
        "the archived live page must retain its immutable identity"
    );
    assert_eq!(archive.pages.len(), 1);
    assert_eq!(archive.pages[0].uuid, page_uuid);
    assert_eq!(archive.blocks.len(), 1);
    assert_eq!(archive.blocks[0].uuid, block_uuid);
    assert_eq!(archive.attachments.len(), 1);
    assert_eq!(archive.attachments[0].uuid, attachment_uuid);
}

#[tokio::test]
async fn archive_workspace_identity_cannot_mix_with_existing_state() {
    let source = database().await;
    db::create_page(&source.connection, "Source".into())
        .await
        .unwrap();
    let archive = db::export_archive(&source.connection).await.unwrap();

    let destination = database().await;
    let existing = db::create_page(&destination.connection, "Destination".into())
        .await
        .unwrap();
    let error = db::import_archive(&destination.connection, archive)
        .await
        .expect_err("cross-workspace restore must fail");
    assert!(error.to_string().contains("different workspace"));
    assert!(
        db::get_page(&destination.connection, existing.uuid)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn archive_rejects_a_nil_workspace_identity() {
    let source = database().await;
    let mut archive = db::export_archive(&source.connection).await.unwrap();
    archive.workspace_uuid = uuid::Uuid::nil();
    let destination = database().await;
    let error = db::import_archive(&destination.connection, archive)
        .await
        .expect_err("nil workspace must fail");
    assert!(error.to_string().contains("cannot be nil"));
}

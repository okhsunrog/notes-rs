use notes_core::{Hlc, Op, OpKind, Origin, PageCreate, PageDelete, PageKind, PageLayout, db};

#[tokio::test]
async fn handwriting_is_an_immutable_note_kind_in_lists_and_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let left = db::open(dir.path().join("left.db")).await.unwrap();
    let right = db::open(dir.path().join("right.db")).await.unwrap();
    let workspace_uuid = db::workspace_uuid(&left).await.unwrap();
    let device_id = uuid::Uuid::now_v7();
    let page_uuid = uuid::Uuid::now_v7();
    let make_op = |wall, kind| Op {
        op_id: uuid::Uuid::now_v7(),
        workspace_uuid,
        device_id,
        hlc: Hlc::new(wall, 0, device_id),
        format_version: notes_core::operation::FORMAT_VERSION,
        kind,
    };
    let page = PageCreate {
        uuid: page_uuid,
        kind: PageKind::Handwriting,
        title: Some("Ink".into()),
        layout: PageLayout::Outline,
        created_at: 1,
    };
    notes_core::apply(
        &left,
        &make_op(1, OpKind::PageCreate(page.clone())),
        Origin::Remote,
    )
    .await
    .unwrap();
    let block = notes_core::BlockCreate {
        uuid: uuid::Uuid::now_v7(),
        page_uuid,
        parent_uuid: None,
        order_key: notes_core::OrderKey::first(),
        style: notes_core::BlockStyle::Paragraph,
        markdown: "Must not become an ink body".into(),
        created_at: 1,
    };
    for kind in [
        OpKind::BlockCreate(block.clone()),
        OpKind::BlockMove(notes_core::BlockMove {
            uuid: block.uuid,
            page_uuid,
            parent_uuid: None,
            order_key: notes_core::OrderKey::first(),
        }),
        OpKind::PageSetLayout(notes_core::PageSetLayout {
            uuid: page_uuid,
            layout: PageLayout::Outline,
        }),
    ] {
        assert!(
            notes_core::apply(&left, &make_op(2, kind), Origin::Remote)
                .await
                .is_err()
        );
    }
    db::create_page(&left, "Text".into()).await.unwrap();
    let notes = db::list_pages(&left, 10).await.unwrap();
    assert_eq!(notes.len(), 2);
    assert!(
        notes
            .iter()
            .any(|p| p.uuid == page_uuid && p.kind == PageKind::Handwriting)
    );
    let snapshot = notes_core::export_sync_snapshot(&left, 0).await.unwrap();
    notes_core::import_sync_snapshot(&right, snapshot)
        .await
        .unwrap();
    assert_eq!(
        db::get_page(&right, page_uuid).await.unwrap().unwrap().kind,
        PageKind::Handwriting
    );
    notes_core::apply(
        &left,
        &make_op(2, OpKind::PageDelete(PageDelete { uuid: page_uuid })),
        Origin::Remote,
    )
    .await
    .unwrap();
    let mut wrong = page.clone();
    wrong.kind = PageKind::Note;
    assert!(
        notes_core::apply(
            &left,
            &make_op(3, OpKind::PageCreate(wrong)),
            Origin::Remote
        )
        .await
        .is_err()
    );
    notes_core::apply(&left, &make_op(4, OpKind::PageCreate(page)), Origin::Remote)
        .await
        .unwrap();
    let archive = db::export_archive(&left).await.unwrap();
    assert!(
        archive
            .page_identities
            .iter()
            .any(|p| p.uuid == page_uuid && p.kind == PageKind::Handwriting)
    );
}

use notes_core::{Hlc, Op, OpKind, Origin, PageCreate, PageKind, PageLayout, db, ink::*};
use uuid::Uuid;

async fn note(conn: &notes_core::Connection) -> Uuid {
    let id = Uuid::now_v7();
    let device = Uuid::now_v7();
    notes_core::apply(
        conn,
        &Op {
            op_id: Uuid::now_v7(),
            workspace_uuid: db::workspace_uuid(conn).await.unwrap(),
            device_id: device,
            hlc: Hlc::new(1, 0, device),
            format_version: notes_core::operation::FORMAT_VERSION,
            kind: OpKind::PageCreate(PageCreate {
                uuid: id,
                kind: PageKind::Handwriting,
                title: Some("Ink".into()),
                layout: PageLayout::Outline,
                created_at: 1,
            }),
        },
        Origin::Remote,
    )
    .await
    .unwrap();
    id
}
fn stroke(seed: u32) -> InkStroke {
    InkStroke {
        id: Uuid::now_v7(),
        width: 2.,
        points: (0..60)
            .map(|n| InkPoint {
                x: f64::from(n + seed),
                y: f64::from(n),
                pressure: 0.5,
                tilt_x: 0.,
                tilt_y: 0.,
                time: f64::from(n),
            })
            .collect(),
    }
}
fn patch(strokes: Vec<InkStroke>) -> InkDraftPatch {
    InkDraftPatch {
        order: strokes.iter().map(|s| s.id).collect(),
        upserts: strokes,
        background: InkBackground::Grid,
    }
}
fn bytes(store: &Store) -> Vec<u8> {
    serde_json::to_vec(&store.read().unwrap()).unwrap()
}

#[tokio::test]
async fn common_database_isolates_notes_history_and_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    let conn = db::open(&path).await.unwrap();
    let text = db::create_page(&conn, "Keep text".into()).await.unwrap();
    let left_id = note(&conn).await;
    let left = Store::new(&path, left_id);
    let right = Store::new(&path, note(&conn).await);
    let rev = right
        .patch(patch(vec![stroke(1), stroke(2)]), None)
        .unwrap();
    let saved = bytes(&right);
    let mut revision = None;
    // Evict the beginning of one history while another document pins its data.
    for i in 0..53 {
        revision = Some(
            left.patch(patch(vec![stroke(i), stroke(i + 1)]), revision)
                .unwrap(),
        );
    }
    while left.compact().unwrap() {}
    assert_eq!(bytes(&right), saved);
    assert_eq!(left.read().unwrap().revision, revision);
    let before = bytes(&left);
    let undo = left.history(Some(false), revision).unwrap();
    let InkHistoryUpdate::Patch {
        revision: undo_rev, ..
    } = undo
    else {
        panic!("expected history delta")
    };
    left.history(Some(true), undo_rev).unwrap();
    let restored = left.read().unwrap();
    assert_eq!(
        serde_json::to_value(&restored.draft).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&before).unwrap()["draft"]
    );
    assert_eq!(right.read().unwrap().revision, Some(rev));
    assert_eq!(
        db::get_page(&conn, text.uuid)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("Keep text")
    );
    assert!(Store::new(&path, text.uuid).read().is_err());
    db::delete_page(&conn, left_id).await.unwrap();
    assert!(left.patch(patch(vec![]), restored.revision).is_err());
    assert!(left.compact().is_err());
    assert_eq!(bytes(&right), saved);
    let sql = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        sql.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(
        !sql.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
}

#[tokio::test]
async fn stale_writes_and_failed_gc_roll_back_the_entire_gesture() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    let conn = db::open(&path).await.unwrap();
    let store = Store::new(&path, note(&conn).await);
    let rev = store.patch(patch(vec![stroke(1)]), None).unwrap();
    let before = bytes(&store);
    assert!(store.patch(patch(vec![stroke(2)]), None).is_err());
    let sql = rusqlite::Connection::open(&path).unwrap();
    // Failure after new chunks, history and head have already been written.
    sql.execute_batch("CREATE TRIGGER fail_gc BEFORE DELETE ON ink_root_refs BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let undo = store.history(Some(false), Some(rev)).unwrap();
    let InkHistoryUpdate::Patch { revision, .. } = undo else {
        panic!()
    };
    let after_undo = bytes(&store);
    assert!(
        store
            .patch(patch(vec![stroke(3)]), revision.clone())
            .is_err()
    );
    assert_eq!(bytes(&store), after_undo);
    sql.execute_batch("DROP TRIGGER fail_gc").unwrap();
    store.history(Some(true), revision).unwrap();
    assert_eq!(
        serde_json::to_value(store.read().unwrap().draft).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&before).unwrap()["draft"]
    );
}

#[tokio::test]
async fn published_graphs_sync_causally_and_conflicts_keep_both_notes() {
    let dir = tempfile::tempdir().unwrap();
    let lp = dir.path().join("left.db");
    let rp = dir.path().join("right.db");
    let l = db::open(&lp).await.unwrap();
    let r = db::open(&rp).await.unwrap();
    let id = note(&l).await;
    notes_core::import_sync_snapshot(&r, notes_core::export_sync_snapshot(&l, 0).await.unwrap())
        .await
        .unwrap();
    let left = Store::new(&lp, id);
    let right = Store::new(&rp, id);
    let rev = left.patch(patch(vec![stroke(1), stroke(2)]), None).unwrap();
    while left.compact().unwrap() {}
    let first = left.publish("Book".into()).unwrap().unwrap();
    let first_graph = transfer::export_graph(&l, first.root_hash).await.unwrap();
    transfer::stage_graph(&r, first.root_hash, first_graph.clone())
        .await
        .unwrap();
    notes_core::import_sync_snapshot(&r, notes_core::export_sync_snapshot(&l, 0).await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(left.read().unwrap().draft).unwrap(),
        serde_json::to_value(right.read().unwrap().draft).unwrap()
    );
    // Both devices edit while offline. Applying remote metadata must not erase local ink.
    left.patch(patch(vec![stroke(3)]), Some(rev)).unwrap();
    right
        .patch(patch(vec![stroke(4)]), right.read().unwrap().revision)
        .unwrap();
    let unpub = bytes(&right);
    let lv = left.publish("Book".into()).unwrap().unwrap();
    transfer::stage_graph(
        &r,
        lv.root_hash,
        transfer::export_graph(&l, lv.root_hash).await.unwrap(),
    )
    .await
    .unwrap();
    notes_core::import_sync_snapshot(&r, notes_core::export_sync_snapshot(&l, 0).await.unwrap())
        .await
        .unwrap();
    assert_eq!(bytes(&right), unpub);
    let rv = right.publish("Desktop".into()).unwrap().unwrap();
    assert_eq!(rv.parents, vec![first.version_uuid]);
    assert_eq!(right.versions().unwrap().len(), 2);
    // Missing/corrupt content never installs a partial graph.
    let mut corrupt = first_graph.clone();
    corrupt.get_mut(&first.root_hash).unwrap()[0] ^= 1;
    assert!(
        transfer::stage_graph(&r, first.root_hash, corrupt)
            .await
            .is_err()
    );
    // Keep both: original resolves every competing head, copy gets its own identity.
    let heads = vec![lv.version_uuid, rv.version_uuid];
    let kept = right
        .resolve(
            heads.clone(),
            vec![rv.version_uuid, lv.version_uuid],
            "Desktop".into(),
        )
        .unwrap();
    assert_eq!(kept[0], id);
    assert_eq!(kept.len(), 2);
    assert_eq!(right.versions().unwrap().len(), 1);
    assert!(
        right
            .resolve(heads, vec![rv.version_uuid], "Desktop".into())
            .is_err()
    );
    let copy = Store::new(&rp, kept[1]);
    assert_eq!(copy.read().unwrap().draft.strokes.len(), 1);
    let snapshot = notes_core::export_sync_snapshot(&r, 0).await.unwrap();
    for version in &snapshot.ink_versions {
        transfer::stage_graph(
            &l,
            version.publication.root_hash,
            transfer::export_graph(&r, version.publication.root_hash)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    }
    notes_core::import_sync_snapshot(&l, snapshot.clone())
        .await
        .unwrap();
    notes_core::import_sync_snapshot(&l, snapshot)
        .await
        .unwrap();
    assert_eq!(left.versions().unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(left.read().unwrap().draft).unwrap(),
        serde_json::to_value(right.read().unwrap().draft).unwrap()
    );
    // Old published graphs remain usable after later compaction/GC.
    transfer::validate_graph(
        first.root_hash,
        &transfer::export_graph(&l, first.root_hash).await.unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn publication_outbox_and_root_commit_together_and_reject_bad_ancestry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    let conn = db::open(&path).await.unwrap();
    notes_core::configure_sync(&conn, "https://notes.example.test")
        .await
        .unwrap();
    let created = db::create_handwritten_note_with_ops(&conn, Some("Ink".into()))
        .await
        .unwrap();
    let store = Store::new(&path, created.value.uuid);
    store.patch(patch(vec![stroke(1)]), None).unwrap();
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch("CREATE TRIGGER fail_outbox BEFORE INSERT ON sync_outbox BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(store.publish("Book".into()).is_err());
    assert!(store.versions().unwrap().is_empty());
    sql.execute_batch("DROP TRIGGER fail_outbox").unwrap();
    let p = store.publish("Book".into()).unwrap().unwrap();
    assert!(store.publish("Book".into()).unwrap().is_none());
    let outbox = notes_core::pending_outbox(&conn, 100).await.unwrap();
    assert!(
        outbox
            .iter()
            .any(|o| matches!(&o.kind,OpKind::InkPublish(v) if *v==p))
    );
    let device = Uuid::now_v7();
    let workspace = db::workspace_uuid(&conn).await.unwrap();
    let a = Uuid::now_v7();
    let b = Uuid::now_v7();
    let operation = |id, parents| Op {
        op_id: Uuid::now_v7(),
        workspace_uuid: workspace,
        device_id: device,
        hlc: Hlc::new(2, 0, device),
        format_version: notes_core::operation::FORMAT_VERSION,
        kind: OpKind::InkPublish(Publish {
            version_uuid: id,
            page_uuid: created.value.uuid,
            parents,
            root_hash: p.root_hash,
            device_name: "Other".into(),
        }),
    };
    notes_core::apply(&conn, &operation(a, vec![b]), Origin::Remote)
        .await
        .unwrap();
    assert!(
        notes_core::apply(&conn, &operation(b, vec![a]), Origin::Remote)
            .await
            .is_err()
    );
    let mut legacy = operation(Uuid::now_v7(), vec![]);
    legacy.format_version = 6;
    assert!(
        notes_core::apply(&conn, &legacy, Origin::Remote)
            .await
            .is_err()
    );
    // Existing persisted version-6 text operations remain readable after upgrade.
    legacy.kind = OpKind::PageCreate(PageCreate {
        uuid: Uuid::now_v7(),
        kind: PageKind::Note,
        title: None,
        layout: PageLayout::Outline,
        created_at: 1,
    });
    notes_core::apply(&conn, &legacy, Origin::Remote)
        .await
        .unwrap();
}

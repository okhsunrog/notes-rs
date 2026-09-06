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

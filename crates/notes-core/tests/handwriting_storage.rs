use notes_core::{
    Hlc, Op, OpKind, Origin, PageCreate, PageDelete, PageKind, PageLayout, db, ink::*,
};
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
async fn replacement_snapshot_cannot_hide_unpublished_handwriting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    let conn = db::open(&path).await.unwrap();
    let empty = notes_core::export_sync_snapshot(&conn, 0).await.unwrap();
    let id = note(&conn).await;
    let store = Store::new(path, id);
    store.open_editor(true).unwrap();
    assert!(
        notes_core::import_sync_snapshot(&conn, empty.clone())
            .await
            .is_err()
    );
    store.patch(patch(vec![stroke(1)]), None).unwrap();
    store.close_editor().unwrap();
    let before = bytes(&store);
    assert!(
        notes_core::import_sync_snapshot(&conn, empty)
            .await
            .is_err()
    );
    assert_eq!(bytes(&store), before);
    assert!(db::get_page(&conn, id).await.unwrap().is_some());
}

#[tokio::test]
async fn workspace_archive_restores_unpublished_ink_and_rolls_back_all_binary_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let conn = db::open(&path).await.unwrap();
    let id = note(&conn).await;
    let empty = note(&conn).await;
    let store = Store::new(&path, id);
    let a = stroke(1);
    let b = stroke(2);
    let revision = store.patch(patch(vec![a.clone()]), None).unwrap();
    store.publish("Book".into()).unwrap();
    store.patch(patch(vec![a, b]), Some(revision)).unwrap();
    let expected = serde_json::to_value(store.read().unwrap().draft).unwrap();
    let archive = db::export_archive(&conn).await.unwrap();
    assert_eq!(
        archive
            .ink
            .documents
            .iter()
            .find(|d| d.page_uuid == id)
            .unwrap()
            .variants
            .len(),
        1,
        "the published base is not a conflict with its local successor"
    );
    let json = serde_json::to_value(&archive).unwrap();
    assert!(
        json.get("ink_blobs").is_none(),
        "binary data must not enter the JSON manifest"
    );
    let missing_binary: db::DataArchive = serde_json::from_value(json).unwrap();
    let target_path = dir.path().join("target.db");
    let target = db::open(&target_path).await.unwrap();
    assert!(db::import_archive(&target, missing_binary).await.is_err());
    let failure = db::import_archive_with_precommit(&target, archive.clone(), || {
        Err(anyhow::anyhow!("injected"))
    })
    .await;
    assert!(failure.is_err());
    assert!(db::list_pages(&target, 10).await.unwrap().is_empty());
    let sql = rusqlite::Connection::open(&target_path).unwrap();
    let count: i64 = sql
        .query_row("SELECT count(*) FROM ink_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    db::import_archive(&target, archive.clone()).await.unwrap();
    let restored = Store::new(&target_path, id);
    assert_eq!(
        serde_json::to_value(restored.read().unwrap().draft).unwrap(),
        expected
    );
    assert_eq!(restored.versions().unwrap().len(), 1);
    assert!(
        Store::new(&target_path, empty)
            .read()
            .unwrap()
            .draft
            .strokes
            .is_empty()
    );
    assert!(
        !restored.compact().unwrap(),
        "restored publication is already packed"
    );
    let first = restored.versions().unwrap()[0].publication.version_uuid;
    db::import_archive(&target, archive).await.unwrap();
    assert_eq!(restored.versions().unwrap().len(), 1);
    assert_eq!(
        restored.versions().unwrap()[0].publication.parents,
        vec![first]
    );
    sql.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
        .map(|v| assert_eq!(v, "ok"))
        .unwrap();
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
    let mut altered = first_graph.clone();
    let mut record = ink_format::record::Record::decode(&altered[&first.root_hash]).unwrap();
    record.required_features.clear();
    let encoded = record.encode().unwrap();
    let altered_hash = notes_blob::BlobHash::digest(&encoded);
    altered.remove(&first.root_hash);
    altered.insert(altered_hash, encoded);
    assert!(
        transfer::validate_graph(altered_hash, &altered).is_err(),
        "different metadata envelopes must not alias the same record ID"
    );
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
    let archive_path = dir.path().join("archive.db");
    let archive_conn = db::open(&archive_path).await.unwrap();
    db::import_archive(&archive_conn, db::export_archive(&r).await.unwrap())
        .await
        .unwrap();
    let archived = Store::new(&archive_path, id);
    assert_eq!(
        archived.versions().unwrap().len(),
        2,
        "backups retain every conflicting variant"
    );
    assert_eq!(
        serde_json::to_value(archived.read().unwrap().draft).unwrap(),
        serde_json::to_value(right.read().unwrap().draft).unwrap()
    );
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

#[tokio::test]
async fn remote_arrival_does_not_swap_an_open_editor_before_its_first_gesture() {
    let dir = tempfile::tempdir().unwrap();
    let lp = dir.path().join("left.db");
    let rp = dir.path().join("right.db");
    let l = db::open(&lp).await.unwrap();
    let r = db::open(&rp).await.unwrap();
    let id = note(&l).await;
    let left = Store::new(&lp, id);
    let right = Store::new(&rp, id);
    left.patch(patch(vec![stroke(1)]), None).unwrap();
    let base = left.publish("Book".into()).unwrap().unwrap();
    transfer::stage_graph(
        &r,
        base.root_hash,
        transfer::export_graph(&l, base.root_hash).await.unwrap(),
    )
    .await
    .unwrap();
    notes_core::import_sync_snapshot(&r, notes_core::export_sync_snapshot(&l, 0).await.unwrap())
        .await
        .unwrap();
    let initial = right.open_editor(true).unwrap();
    left.patch(patch(vec![stroke(2)]), left.read().unwrap().revision)
        .unwrap();
    let remote = left.publish("Book".into()).unwrap().unwrap();
    transfer::stage_graph(
        &r,
        remote.root_hash,
        transfer::export_graph(&l, remote.root_hash).await.unwrap(),
    )
    .await
    .unwrap();
    notes_core::import_sync_snapshot(&r, notes_core::export_sync_snapshot(&l, 0).await.unwrap())
        .await
        .unwrap();
    assert_eq!(right.read().unwrap().revision, initial.snapshot.revision);
    assert!(!right.status().unwrap().unpublished_changes);
    right
        .patch(patch(vec![stroke(3)]), initial.snapshot.revision)
        .unwrap();
    let local = right.publish("Desktop".into()).unwrap().unwrap();
    assert_eq!(local.parents, vec![base.version_uuid]);
    assert_eq!(right.versions().unwrap().len(), 2);
    right
        .resolve(
            vec![remote.version_uuid, local.version_uuid],
            vec![remote.version_uuid],
            "Desktop".into(),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(right.read().unwrap().draft).unwrap(),
        serde_json::to_value(left.read().unwrap().draft).unwrap()
    );
}

fn ink_body_rows(path: &std::path::Path) -> (i64, i64) {
    let sql = rusqlite::Connection::open(path).unwrap();
    (
        sql.query_row("SELECT count(*) FROM ink_records", [], |r| r.get(0))
            .unwrap(),
        sql.query_row("SELECT count(*) FROM ink_chunks", [], |r| r.get(0))
            .unwrap(),
    )
}

#[tokio::test]
async fn deleting_a_note_purges_its_drawing_on_every_replica() {
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
    left.patch(patch(vec![stroke(1), stroke(2)]), None).unwrap();
    while left.compact().unwrap() {}
    let published = left.publish("Book".into()).unwrap().unwrap();
    transfer::stage_graph(
        &r,
        published.root_hash,
        transfer::export_graph(&l, published.root_hash)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    notes_core::import_sync_snapshot(&r, notes_core::export_sync_snapshot(&l, 0).await.unwrap())
        .await
        .unwrap();
    let right = Store::new(&rp, id);
    assert_eq!(right.read().unwrap().draft.strokes.len(), 2);
    for path in [&lp, &rp] {
        let (records, chunks) = ink_body_rows(path);
        assert!(records > 0 && chunks > 0, "the note has bodies to lose");
    }

    assert!(db::delete_page(&l, id).await.unwrap().is_some());

    // The same delete operation arrives on the second replica through the
    // ordinary apply path, and must purge there too.
    let device = Uuid::now_v7();
    notes_core::apply(
        &r,
        &Op {
            op_id: Uuid::now_v7(),
            workspace_uuid: db::workspace_uuid(&r).await.unwrap(),
            device_id: device,
            hlc: Hlc::new(9, 0, device),
            format_version: notes_core::operation::FORMAT_VERSION,
            kind: OpKind::PageDelete(PageDelete { uuid: id }),
        },
        Origin::Remote,
    )
    .await
    .unwrap();

    for (path, conn) in [(&lp, &l), (&rp, &r)] {
        assert_eq!(
            ink_body_rows(path),
            (0, 0),
            "a deleted note leaves no records or chunks behind"
        );
        assert!(
            notes_core::export_sync_snapshot(conn, 0)
                .await
                .unwrap()
                .ink_versions
                .is_empty(),
            "a deleted note is not exported as a published version"
        );
        let sql = rusqlite::Connection::open(path).unwrap();
        for table in [
            "ink_documents",
            "ink_history",
            "ink_versions",
            "ink_staged_roots",
        ] {
            let rows: i64 = sql
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(rows, 0, "{table} still holds rows for the deleted note");
        }
    }
    assert!(
        Store::new(&lp, id).read().is_err()
            || Store::new(&lp, id).read().unwrap().draft.strokes.is_empty()
    );
}

/// Undo followed by redo leaves the document exactly where it started, but marks
/// it dirty. Publishing that would add a version identical to its own parent and
/// hand every other replica a conflict to resolve.
#[tokio::test]
async fn an_undo_redo_round_trip_publishes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    let conn = db::open(&path).await.unwrap();
    let id = note(&conn).await;
    let store = Store::new(&path, id);
    let revision = store.patch(patch(vec![stroke(1)]), None).unwrap();
    store
        .patch(patch(vec![stroke(1), stroke(2)]), Some(revision))
        .unwrap();
    while store.compact().unwrap() {}
    let base = store.publish("Book".into()).unwrap().unwrap();

    store
        .history(Some(false), store.read().unwrap().revision)
        .unwrap();
    store
        .history(Some(true), store.read().unwrap().revision)
        .unwrap();
    assert!(store.status().unwrap().unpublished_changes);

    assert!(
        store.publish("Book".into()).unwrap().is_none(),
        "an unchanged root must not become a second version"
    );
    let versions = store.versions().unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].publication.version_uuid, base.version_uuid);
    assert!(!store.status().unwrap().unpublished_changes);
}

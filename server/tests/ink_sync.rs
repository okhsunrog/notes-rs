use futures::{SinkExt, StreamExt};
use notes_core::{OpKind, db, ink::*};
use notes_server::{ServerConfig, config::UserConfig};
use notes_sync::HttpTransport;
use uuid::Uuid;
const TOKEN: &str = "ink-owner-token-with-at-least-thirty-two-characters";
const OTHER: &str = "ink-other-token-with-at-least-thirty-two-characters";
fn patch(seed: u32) -> InkDraftPatch {
    let stroke = InkStroke {
        id: Uuid::now_v7(),
        width: 2.,
        points: (0..40)
            .map(|n| InkPoint {
                x: f64::from(seed + n),
                y: f64::from(n),
                pressure: 0.5,
                tilt_x: 0.,
                tilt_y: 0.,
                time: f64::from(n),
            })
            .collect(),
    };
    InkDraftPatch {
        order: vec![stroke.id],
        upserts: vec![stroke],
        background: InkBackground::Grid,
    }
}
async fn pull(http: &HttpTransport, conn: &notes_core::Connection) {
    let cursor = notes_core::sync_cursor(conn).await.unwrap();
    let ops = http.ops_since(cursor, 100).await.unwrap();
    for op in &ops {
        if let OpKind::InkPublish(p) = &op.envelope.kind {
            http.download_ink_graph(conn, p.root_hash).await.unwrap();
        }
    }
    notes_core::apply_sequenced_batch(conn, ops.into_iter().map(|o| (o.seq, o.envelope)).collect())
        .await
        .unwrap();
}
async fn push(http: &HttpTransport, conn: &notes_core::Connection) {
    let ops = notes_core::pending_outbox(conn, 100).await.unwrap();
    for op in &ops {
        if let OpKind::InkPublish(p) = &op.kind {
            http.upload_ink_graph(conn, p.root_hash).await.unwrap();
        }
    }
    let accepted = http.push(ops).await.unwrap();
    notes_core::acknowledge_server_ops(
        conn,
        accepted.iter().map(|o| (o.envelope.op_id, o.seq)).collect(),
    )
    .await
    .unwrap();
    pull(http, conn).await;
}
#[tokio::test]
async fn binary_ink_sync_deduplicates_and_resolves_offline_branches_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        log_filter: "warn".into(),
        data_dir: dir.path().join("server"),
        snapshot_every_ops: 2,
        max_blob_bytes: 16 * 1024 * 1024,
        max_user_blob_bytes: 64 * 1024 * 1024,
        ai: None,
        users: vec![
            UserConfig {
                id: "owner".into(),
                admin: true,
                token_sha256: None,
                token: None,
                tokens: vec![TOKEN.into()],
            },
            UserConfig {
                id: "other".into(),
                admin: false,
                token_sha256: None,
                token: None,
                tokens: vec![OTHER.into()],
            },
        ],
    };
    let state = notes_server::build_state(&config).await.unwrap();
    let listener = tokio::net::TcpListener::bind(config.listen).await.unwrap();
    let url = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, notes_server::router(state))
            .await
            .unwrap();
    });
    let http = HttpTransport::new(url.clone(), TOKEN).unwrap();
    let other = HttpTransport::new(url.clone(), OTHER).unwrap();
    let lp = dir.path().join("left.db");
    let rp = dir.path().join("right.db");
    let l = db::open(&lp).await.unwrap();
    let r = db::open(&rp).await.unwrap();
    for conn in [&l, &r] {
        notes_core::import_sync_snapshot(conn, http.snapshot().await.unwrap())
            .await
            .unwrap();
        notes_core::configure_sync(conn, url.as_str())
            .await
            .unwrap();
    }
    let id = db::create_handwritten_note_with_ops(&l, Some("Ink over HTTP".into()))
        .await
        .unwrap()
        .value
        .uuid;
    let left = Store::new(&lp, id);
    let right = Store::new(&rp, id);
    left.patch(patch(1), None).unwrap();
    while left.compact().unwrap() {}
    let first = left.publish("Book".into()).unwrap().unwrap();
    // No half-published note when root or dependencies are unavailable.
    assert!(
        http.push(notes_core::pending_outbox(&l, 100).await.unwrap())
            .await
            .is_err()
    );
    assert!(http.snapshot().await.unwrap().pages.is_empty());
    // The WebSocket ingestion path enforces the same complete-graph rule.
    let mut socket = http.connect(0).await.unwrap();
    let request = notes_protocol::ClientMessage::Push {
        ops: notes_core::pending_outbox(&l, 100).await.unwrap(),
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&request).unwrap().into(),
        ))
        .await
        .unwrap();
    let rejected = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let message: notes_protocol::ServerMessage =
        serde_json::from_str(rejected.to_text().unwrap()).unwrap();
    assert!(matches!(
        message,
        notes_protocol::ServerMessage::Error { .. }
    ));
    socket.close(None).await.unwrap();
    let graph = transfer::export_graph(&l, first.root_hash).await.unwrap();
    assert_eq!(
        http.upload_ink_graph(&l, first.root_hash).await.unwrap(),
        graph.len()
    );
    assert_eq!(http.upload_ink_graph(&l, first.root_hash).await.unwrap(), 0);
    assert!(
        other
            .download_ink_blobs(vec![first.root_hash])
            .await
            .is_err()
    );
    assert_eq!(
        other
            .missing_ink_blobs(vec![first.root_hash])
            .await
            .unwrap(),
        vec![first.root_hash]
    );
    let mut root_only = transfer::Blobs::new();
    root_only.insert(first.root_hash, graph[&first.root_hash].clone());
    other.upload_ink_blobs(root_only).await.unwrap();
    let other_workspace = other.snapshot().await.unwrap().workspace_uuid;
    let mut forged = notes_core::pending_outbox(&l, 100).await.unwrap();
    for op in &mut forged {
        op.workspace_uuid = other_workspace;
    }
    assert!(other.push(forged).await.is_err());
    assert!(other.snapshot().await.unwrap().pages.is_empty());
    push(&http, &l).await;
    pull(&http, &r).await;
    assert_eq!(
        http.download_ink_graph(&r, first.root_hash).await.unwrap(),
        0
    );
    assert_eq!(
        serde_json::to_value(left.read().unwrap().draft).unwrap(),
        serde_json::to_value(right.read().unwrap().draft).unwrap()
    );
    // Independent offline edits, then publish in either order.
    left.patch(patch(2), left.read().unwrap().revision).unwrap();
    right
        .patch(patch(3), right.read().unwrap().revision)
        .unwrap();
    left.publish("Book".into()).unwrap();
    right.publish("Desktop".into()).unwrap();
    push(&http, &l).await;
    push(&http, &r).await;
    pull(&http, &l).await;
    assert_eq!(left.versions().unwrap().len(), 2);
    assert_eq!(right.versions().unwrap().len(), 2);
    let heads = right
        .versions()
        .unwrap()
        .iter()
        .map(|v| v.publication.version_uuid)
        .collect::<Vec<_>>();
    let kept = right
        .resolve(heads.clone(), heads, "Desktop".into())
        .unwrap();
    push(&http, &r).await;
    pull(&http, &l).await;
    assert_eq!(left.versions().unwrap().len(), 1);
    assert_eq!(
        Store::new(&lp, kept[1]).read().unwrap().draft.strokes.len(),
        1
    );
    // A freshly joined client obtains version metadata and only then binary bodies.
    let snapshot = http.snapshot().await.unwrap();
    let np = dir.path().join("new.db");
    let new = db::open(&np).await.unwrap();
    notes_core::import_sync_snapshot(&new, snapshot.clone())
        .await
        .unwrap();
    assert!(Store::new(&np, id).read().is_err());
    for v in &snapshot.ink_versions {
        http.download_ink_graph(&new, v.publication.root_hash)
            .await
            .unwrap();
    }
    assert_eq!(Store::new(&np, id).versions().unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(Store::new(&np, id).read().unwrap().draft).unwrap(),
        serde_json::to_value(right.read().unwrap().draft).unwrap()
    );
    server.abort();
}

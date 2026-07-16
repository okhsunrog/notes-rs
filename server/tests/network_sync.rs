use futures::StreamExt;
use notes_core::{NodeKind, acknowledge_server_op, apply_sequenced, export_sync_snapshot};
use notes_server::config::{ServerConfig, StorageConfig, UserConfig};
use notes_sync::{HttpTransport, ServerMessage};
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "network-test-token-with-at-least-thirty-two-characters";

#[tokio::test]
async fn bootstraps_and_fanouts_operations_over_the_real_network_protocol() {
    let server_directory = tempfile::tempdir().expect("server directory");
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().expect("listen address"),
        data_dir: server_directory.path().to_owned(),
        snapshot_every_ops: 10_000,
        max_blob_bytes: 1024,
        storage: StorageConfig {
            embedding_provider_id: "test".into(),
            embedding_dimensions: 8,
        },
        users: vec![UserConfig {
            id: "owner".into(),
            tokens: vec![TOKEN.into()],
        }],
    };
    let state = notes_server::build_state(&config)
        .await
        .expect("server state");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn(async move {
        axum::serve(listener, notes_server::router(state))
            .await
            .expect("test server");
    });
    let transport = HttpTransport::new(
        url::Url::parse(&format!("http://{address}")).expect("server URL"),
        TOKEN,
    )
    .expect("transport");
    transport.health().await.expect("health");

    let left_directory = tempfile::tempdir().expect("left directory");
    let left = notes_core::db::open(left_directory.path().join("notes.db"), "test", 8)
        .await
        .expect("left database");
    notes_core::db::create_node(
        &left,
        NodeKind::Page,
        Some("Bootstrap page".into()),
        String::new(),
        None,
    )
    .await
    .expect("bootstrap note");
    let bootstrap = export_sync_snapshot(&left, 0)
        .await
        .expect("local snapshot");
    let server_snapshot = transport
        .bootstrap(bootstrap)
        .await
        .expect("bootstrap server");
    assert_eq!(server_snapshot.nodes.len(), 1);

    let right_directory = tempfile::tempdir().expect("right directory");
    let right = notes_core::db::open(right_directory.path().join("notes.db"), "test", 8)
        .await
        .expect("right database");
    notes_core::import_sync_snapshot(&right, transport.snapshot().await.expect("snapshot"))
        .await
        .expect("import right snapshot");

    notes_core::configure_sync(&left, transport.base_url().as_str())
        .await
        .expect("enable left sync");
    let mut socket = transport.connect(0).await.expect("websocket");
    notes_core::db::create_node(
        &left,
        NodeKind::Page,
        Some("Realtime page".into()),
        String::new(),
        None,
    )
    .await
    .expect("realtime note");
    let pending = notes_core::pending_outbox(&left, 256)
        .await
        .expect("left outbox");
    assert_eq!(pending.len(), 1);
    let accepted = transport.push(pending).await.expect("push operation");
    assert_eq!(accepted[0].seq, 1);
    acknowledge_server_op(&left, accepted[0].envelope.op_id, accepted[0].seq)
        .await
        .expect("acknowledge left op");

    let incoming = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("websocket fanout timeout")
        .expect("websocket message")
        .expect("valid websocket message");
    let Message::Text(text) = incoming else {
        panic!("expected a text message");
    };
    let ServerMessage::Ops { ops } =
        serde_json::from_str::<ServerMessage>(&text).expect("server message")
    else {
        panic!("expected an operations message");
    };
    assert_eq!(ops.len(), 1);
    apply_sequenced(&right, ops[0].seq, &ops[0].envelope)
        .await
        .expect("apply right operation");

    let mut titles = notes_core::db::list_pages(&right, 10)
        .await
        .expect("right pages")
        .into_iter()
        .filter_map(|node| node.title)
        .collect::<Vec<_>>();
    titles.sort();
    assert_eq!(titles, vec!["Bootstrap page", "Realtime page"]);
    server.abort();
}

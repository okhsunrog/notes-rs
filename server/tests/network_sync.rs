use futures::StreamExt;
use notes_core::{BlockStyle, acknowledge_server_op, apply_sequenced, export_sync_snapshot};
use notes_protocol::ServerMessage;
use notes_server::config::{ServerConfig, UserConfig};
use notes_sync::HttpTransport;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "network-test-token-with-at-least-thirty-two-characters";

#[tokio::test]
async fn bootstraps_and_fanouts_operations_over_the_real_network_protocol() {
    let server_directory = tempfile::tempdir().expect("server directory");
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().expect("listen address"),
        log_filter: "info".into(),
        data_dir: server_directory.path().to_owned(),
        snapshot_every_ops: 10_000,
        max_blob_bytes: 1024,
        max_user_blob_bytes: 1024,
        ai: None,
        mcp_allowed_hosts: Vec::new(),
        public_url: None,
        users: vec![UserConfig {
            id: "owner".into(),
            admin: true,
            token_sha256: None,
            token: None,
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
    let left = notes_core::db::open(left_directory.path().join("notes.db"))
        .await
        .expect("left database");
    let bootstrap_page = notes_core::db::create_page(&left, "Bootstrap page".into())
        .await
        .expect("bootstrap note");
    let bootstrap_block = notes_core::db::create_block(
        &left,
        bootstrap_page.uuid,
        None,
        None,
        BlockStyle::Heading1,
        "Typed block snapshot".into(),
    )
    .await
    .expect("bootstrap block");
    let bootstrap = export_sync_snapshot(&left, 0)
        .await
        .expect("local snapshot");
    let server_snapshot = transport
        .bootstrap(bootstrap.clone())
        .await
        .expect("bootstrap server");
    assert_eq!(server_snapshot.pages.len(), 1);
    assert_eq!(server_snapshot.blocks.len(), 1);
    assert_eq!(server_snapshot.blocks[0].style, BlockStyle::Heading1);

    let conflict = transport
        .bootstrap(bootstrap)
        .await
        .expect_err("a second bootstrap must be rejected");
    let conflict = notes_sync::transport_error(&conflict).expect("typed transport error");
    assert!(matches!(
        conflict,
        notes_sync::TransportError::Conflict(message)
            if message == "server workspace has already been initialized"
    ));
    assert!(conflict.is_permanent());

    let right_directory = tempfile::tempdir().expect("right directory");
    let right = notes_core::db::open(right_directory.path().join("notes.db"))
        .await
        .expect("right database");
    notes_core::import_sync_snapshot(&right, transport.snapshot().await.expect("snapshot"))
        .await
        .expect("import right snapshot");
    let imported_block = notes_core::db::get_block(&right, bootstrap_block.uuid)
        .await
        .expect("read imported block")
        .expect("imported block");
    assert_eq!(imported_block.page_uuid, bootstrap_page.uuid);
    assert_eq!(imported_block.style, BlockStyle::Heading1);
    assert_eq!(imported_block.markdown, "Typed block snapshot");

    notes_core::configure_sync(&left, transport.base_url().as_str())
        .await
        .expect("enable left sync");
    let mut socket = transport.connect(0).await.expect("websocket");
    notes_core::db::create_page(&left, "Realtime page".into())
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
        .filter_map(|page| page.title)
        .collect::<Vec<_>>();
    titles.sort();
    assert_eq!(titles, vec!["Bootstrap page", "Realtime page"]);
    server.abort();
}

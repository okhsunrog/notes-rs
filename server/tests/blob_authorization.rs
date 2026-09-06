//! Downloading a blob takes more than naming it in your own oplog.
//!
//! Operations are client-authored, so any user can declare any hash. The
//! server therefore authorizes bytes by ownership, and validates declared
//! attachment blobs on every ingest path, sockets included.

use futures::{SinkExt, StreamExt};
use notes_core::{AttachmentOwner, Hlc, Op, OpKind, PageKind, PageLayout};
use notes_server::{ServerConfig, config::UserConfig};
use notes_sync::{AttachmentAdd, HttpTransport, PageCreate};
use uuid::Uuid;

const OWNER: &str = "blob-owner-token-with-at-least-thirty-two-characters";
const OTHER: &str = "blob-other-token-with-at-least-thirty-two-characters";

struct Server {
    url: url::Url,
    client: reqwest::Client,
    _directory: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            log_filter: "warn".into(),
            data_dir: directory.path().join("server"),
            snapshot_every_ops: 1000,
            max_blob_bytes: 1024 * 1024,
            max_user_blob_bytes: 8 * 1024 * 1024,
            ai: None,
            mcp_allowed_hosts: Vec::new(),
            public_url: None,
            users: vec![
                UserConfig {
                    id: "owner".into(),
                    admin: true,
                    token_sha256: None,
                    token: None,
                    tokens: vec![OWNER.into()],
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
        let task = tokio::spawn(async move {
            axum::serve(listener, notes_server::router(state))
                .await
                .unwrap();
        });
        Self {
            url,
            client: reqwest::Client::new(),
            _directory: directory,
            task,
        }
    }

    fn transport(&self, token: &str) -> HttpTransport {
        HttpTransport::new(self.url.clone(), token).unwrap()
    }

    async fn upload_blob(&self, token: &str, hash: notes_core::BlobHash, bytes: Vec<u8>) -> u16 {
        self.client
            .put(self.url.join(&format!("v1/blobs/{hash}")).unwrap())
            .bearer_auth(token)
            .body(bytes)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    async fn download_blob(&self, token: &str, hash: notes_core::BlobHash) -> u16 {
        self.client
            .get(self.url.join(&format!("v1/blobs/{hash}")).unwrap())
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn envelope(workspace_uuid: Uuid, index: u64, kind: OpKind) -> Op {
    let device_id = Uuid::from_u128(u128::from(index) + 0xD0);
    Op {
        op_id: Uuid::now_v7(),
        workspace_uuid,
        device_id,
        hlc: Hlc::new(index, 0, device_id),
        format_version: notes_sync::FORMAT_VERSION,
        kind,
    }
}

fn attachment_operations(
    workspace_uuid: Uuid,
    page_uuid: Uuid,
    hash: notes_core::BlobHash,
    size: u64,
) -> Vec<Op> {
    vec![
        envelope(
            workspace_uuid,
            1,
            OpKind::PageCreate(PageCreate {
                uuid: page_uuid,
                kind: PageKind::Note,
                title: Some("Attachment carrier".into()),
                layout: PageLayout::Outline,
                created_at: 1,
            }),
        ),
        envelope(
            workspace_uuid,
            2,
            OpKind::AttachmentAdd(AttachmentAdd {
                owner: AttachmentOwner::Page(page_uuid),
                blob_hash: hash,
                filename: "attachment.bin".into(),
                mime: "application/octet-stream".into(),
                size,
            }),
        ),
    ]
}

#[tokio::test]
async fn a_declared_hash_never_authorizes_another_users_blob() {
    let server = Server::start().await;
    let owner = server.transport(OWNER);
    let other = server.transport(OTHER);
    let contents = b"private attachment bytes".to_vec();
    let hash = notes_core::BlobHash::digest(&contents);

    assert_eq!(
        server.upload_blob(OWNER, hash, contents.clone()).await,
        201,
        "the owner uploads and therefore owns the blob"
    );
    let owner_workspace = owner.snapshot().await.unwrap().workspace_uuid;
    owner
        .push(attachment_operations(
            owner_workspace,
            Uuid::from_u128(0xA11),
            hash,
            contents.len() as u64,
        ))
        .await
        .unwrap();
    assert_eq!(
        server.download_blob(OWNER, hash).await,
        200,
        "the owner still downloads its own blob"
    );

    // The other user declares the same hash in its own workspace. Whether the
    // push is accepted or refused, the bytes must stay out of reach.
    let other_workspace = other.snapshot().await.unwrap().workspace_uuid;
    let _ = other
        .push(attachment_operations(
            other_workspace,
            Uuid::from_u128(0xB22),
            hash,
            contents.len() as u64,
        ))
        .await;
    assert_eq!(
        server.download_blob(OTHER, hash).await,
        404,
        "referencing another user's hash must not authorize the download"
    );
    assert_eq!(
        server.download_blob(OWNER, hash).await,
        200,
        "the owner's own access is unaffected"
    );
}

#[tokio::test]
async fn socket_pushes_validate_declared_attachment_blobs() {
    let server = Server::start().await;
    let other = server.transport(OTHER);
    let workspace_uuid = other.snapshot().await.unwrap().workspace_uuid;
    let absent = notes_core::BlobHash::digest(b"never uploaded anywhere");

    let mut socket = other.connect(0).await.unwrap();
    let request = notes_protocol::ClientMessage::Push {
        ops: attachment_operations(workspace_uuid, Uuid::from_u128(0xC33), absent, 23),
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&request).unwrap().into(),
        ))
        .await
        .unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let message: notes_protocol::ServerMessage =
        serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert!(
        matches!(message, notes_protocol::ServerMessage::Error { .. }),
        "a socket push declaring an absent blob must be rejected, got {message:?}"
    );
    socket.close(None).await.unwrap();

    assert!(
        other.snapshot().await.unwrap().pages.is_empty(),
        "the rejected batch must not have mutated the workspace"
    );
}

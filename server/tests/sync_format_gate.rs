//! An old client must be told to update, not handed operations it cannot read.
//!
//! Before the gate the server answered every `/v1/ops` request in the format it
//! writes today; a client one format behind decoded nothing and reported a
//! generic "decoding sync response" failure on every reconnect, forever.

use notes_protocol::{ApiErrorCode, ApiErrorResponse, FORMAT_VERSION_HEADER, ServerInfo};
use notes_server::config::{ServerConfig, UserConfig};
use notes_sync::{FORMAT_VERSION, HttpTransport};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const TOKEN: &str = "format-gate-token-with-at-least-thirty-two-characters";

struct Server {
    url: url::Url,
    client: reqwest::Client,
    _directory: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start() -> Server {
    let directory = tempfile::tempdir().expect("server directory");
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().expect("listen address"),
        log_filter: "warn".into(),
        data_dir: directory.path().to_owned(),
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
    let state = notes_server::build_state(&config).await.expect("state");
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .expect("listener");
    let url = url::Url::parse(&format!(
        "http://{}/",
        listener.local_addr().expect("address")
    ))
    .expect("server URL");
    let task = tokio::spawn(async move {
        axum::serve(listener, notes_server::router(state))
            .await
            .expect("test server");
    });
    Server {
        url,
        client: reqwest::Client::new(),
        _directory: directory,
        task,
    }
}

impl Server {
    async fn get_ops(&self, declared: Option<&str>) -> reqwest::Response {
        let mut request = self
            .client
            .get(self.url.join("v1/ops?since=0").expect("ops URL"))
            .bearer_auth(TOKEN);
        if let Some(declared) = declared {
            request = request.header(FORMAT_VERSION_HEADER, declared);
        }
        request.send().await.expect("ops response")
    }

    async fn connect_socket(
        &self,
        declared: Option<&str>,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        let mut url = self.url.join("v1/sync?since=0").expect("socket URL");
        url.set_scheme("ws").expect("websocket scheme");
        let mut request = url
            .as_str()
            .into_client_request()
            .expect("websocket request");
        request.headers_mut().insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {TOKEN}").parse().expect("bearer header"),
        );
        if let Some(declared) = declared {
            request.headers_mut().insert(
                FORMAT_VERSION_HEADER,
                declared.parse().expect("version header"),
            );
        }
        tokio_tungstenite::connect_async(request).await.map(|_| ())
    }
}

#[tokio::test]
async fn operation_endpoints_refuse_clients_that_cannot_read_the_current_format() {
    let server = start().await;
    let older = (FORMAT_VERSION - 1).to_string();

    for declared in [None, Some(older.as_str())] {
        let response = server.get_ops(declared).await;
        assert_eq!(
            response.status().as_u16(),
            426,
            "declared version {declared:?} must be refused"
        );
        let body: ApiErrorResponse = response.json().await.expect("typed refusal");
        assert_eq!(body.error.code, ApiErrorCode::FormatUnsupported);
        assert!(
            body.error.message.contains("update the app"),
            "refusal must say what to do: {}",
            body.error.message
        );
        assert!(
            server.connect_socket(declared).await.is_err(),
            "the socket handshake must refuse the same client"
        );
    }

    let current = FORMAT_VERSION.to_string();
    assert_eq!(server.get_ops(Some(&current)).await.status().as_u16(), 200);
    server
        .connect_socket(Some(&current))
        .await
        .expect("a current client still connects");
}

#[tokio::test]
async fn the_current_client_announces_its_format_and_reads_the_servers() {
    let server = start().await;
    let transport = HttpTransport::new(server.url.clone(), TOKEN).expect("transport");

    let info: ServerInfo = transport.info().await.expect("server info");
    assert_eq!(info.format_version, FORMAT_VERSION);

    assert!(
        transport
            .ops_since(0, 10)
            .await
            .expect("the shipped client is accepted")
            .is_empty()
    );
    transport.connect(0).await.expect("socket handshake");
}

/// A server that predates the field still answers `/v1/info`; the client must
/// read that as "unknown", not as a reason to stop syncing.
#[test]
fn server_info_without_a_format_version_decodes_as_zero() {
    let info: ServerInfo = serde_json::from_str(
        r#"{"workspaceUuid":"00000000-0000-0000-0000-000000000001","aiEnabled":false}"#,
    )
    .expect("legacy server info");
    assert_eq!(info.format_version, 0);
}

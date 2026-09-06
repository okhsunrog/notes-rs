//! A running server with one user, for tests that need the real HTTP surface.

use notes_server::config::{ServerConfig, UserConfig};
use notes_sync::HttpTransport;
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};

pub const TOKEN: &str = "integration-token-with-at-least-thirty-two-characters";

pub struct Harness {
    /// Each test binary compiles this module separately, so a field only one of
    /// them reaches for still reads as dead here.
    #[allow(dead_code)]
    pub state: notes_server::AppState,
    pub address: std::net::SocketAddr,
    _directory: tempfile::TempDir,
}

impl Harness {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    /// Compiled into every test binary, reached from only some of them.
    #[allow(dead_code)]
    pub fn sync_transport(&self) -> HttpTransport {
        HttpTransport::new(url::Url::parse(&self.url("")).expect("server URL"), TOKEN)
            .expect("transport")
    }
}

#[allow(dead_code)]
pub async fn start_server() -> Harness {
    start(false, Vec::new()).await
}

/// A server that knows the origin it is reached at, which is what turns the
/// OAuth endpoints on.
#[allow(dead_code)]
pub async fn start_public_server() -> Harness {
    start(true, Vec::new()).await
}

/// A public server that answers for more than one name, as a deployment with
/// two domains does.
#[allow(dead_code)]
pub async fn start_public_server_for(hosts: Vec<String>) -> Harness {
    start(true, hosts).await
}

async fn start(public: bool, mcp_allowed_hosts: Vec<String>) -> Harness {
    let directory = tempfile::tempdir().expect("server directory");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("listener address");
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().expect("listen address"),
        log_filter: "info".into(),
        data_dir: directory.path().to_owned(),
        snapshot_every_ops: 10_000,
        max_blob_bytes: 1024,
        max_user_blob_bytes: 1024,
        ai: None,
        mcp_allowed_hosts,
        public_url: public
            .then(|| url::Url::parse(&format!("http://{address}")).expect("public URL")),
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
    let served = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, notes_server::router(served))
            .await
            .expect("test server");
    });
    Harness {
        state,
        address,
        _directory: directory,
    }
}

/// Opens an MCP session against the harness, carrying `token` as the bearer
/// credential — a server token or an OAuth access token, the endpoint takes
/// either.
#[allow(dead_code)]
pub async fn connect_mcp(
    harness: &Harness,
    token: &str,
) -> Result<RunningService<RoleClient, ()>, String> {
    let config = StreamableHttpClientTransportConfig::with_uri(harness.url("/mcp"))
        .auth_header(token.to_owned());
    let transport = StreamableHttpClientTransport::with_client(reqwest::Client::default(), config);
    ().serve(transport).await.map_err(|error| error.to_string())
}

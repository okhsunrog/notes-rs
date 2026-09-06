//! A running server with one user, for tests that need the real HTTP surface.

use notes_server::config::{ServerConfig, UserConfig};
use notes_sync::HttpTransport;

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

    pub fn sync_transport(&self) -> HttpTransport {
        HttpTransport::new(url::Url::parse(&self.url("")).expect("server URL"), TOKEN)
            .expect("transport")
    }
}

pub async fn start_server() -> Harness {
    let directory = tempfile::tempdir().expect("server directory");
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().expect("listen address"),
        log_filter: "info".into(),
        data_dir: directory.path().to_owned(),
        snapshot_every_ops: 10_000,
        max_blob_bytes: 1024,
        max_user_blob_bytes: 1024,
        ai: None,
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

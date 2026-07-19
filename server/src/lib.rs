pub mod ai;
pub mod api;
mod blob_ownership;
pub mod config;
mod oplog;
pub mod state;

pub use api::{AppState, router};
pub use config::ServerConfig;
pub use state::UserRegistry;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub async fn build_state(config: &ServerConfig) -> Result<AppState> {
    let shutdown = CancellationToken::new();
    let registry = UserRegistry::open(config, shutdown.child_token()).await?;
    let blob_ownership =
        blob_ownership::BlobOwnership::open(&config.data_dir.join("blob-ownership.db")).await?;
    let ai = match &config.ai {
        Some(ai) => Some(std::sync::Arc::new(
            ai::AiRuntime::open(ai, &registry, &config.data_dir, shutdown.child_token()).await?,
        )),
        None => None,
    };
    Ok(AppState {
        registry,
        data_dir: config.data_dir.clone(),
        max_blob_bytes: config.max_blob_bytes,
        max_user_blob_bytes: config.max_user_blob_bytes,
        blob_ownership,
        ai,
        shutdown,
    })
}

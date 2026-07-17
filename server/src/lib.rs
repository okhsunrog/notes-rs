pub mod ai;
pub mod api;
pub mod config;
mod oplog;
pub mod state;

pub use api::{AppState, router};
pub use config::ServerConfig;
pub use state::UserRegistry;

use anyhow::Result;

pub async fn build_state(config: &ServerConfig) -> Result<AppState> {
    let registry = UserRegistry::open(config).await?;
    let ai = match &config.ai {
        Some(ai) => Some(std::sync::Arc::new(
            ai::AiRuntime::open(ai, &registry, &config.data_dir).await?,
        )),
        None => None,
    };
    Ok(AppState {
        registry,
        data_dir: config.data_dir.clone(),
        max_blob_bytes: config.max_blob_bytes,
        ai,
        embedding_provider_id: config.storage.embedding_provider_id.clone(),
        embedding_dimensions: config.storage.embedding_dimensions,
    })
}

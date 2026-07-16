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
    let ai = config
        .ai
        .as_ref()
        .map(ai::AiRuntime::new)
        .transpose()?
        .map(std::sync::Arc::new);
    if let Some(ai) = &ai {
        for user in registry.users() {
            ai.spawn_workers(user.notes.clone());
        }
    }
    Ok(AppState {
        registry,
        data_dir: config.data_dir.clone(),
        max_blob_bytes: config.max_blob_bytes,
        ai,
        embedding_provider_id: config.storage.embedding_provider_id.clone(),
        embedding_dimensions: config.storage.embedding_dimensions,
    })
}

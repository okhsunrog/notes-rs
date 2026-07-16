pub mod api;
pub mod config;
mod oplog;
pub mod state;

pub use api::{AppState, router};
pub use config::ServerConfig;
pub use state::UserRegistry;

use anyhow::Result;

pub async fn build_state(config: &ServerConfig) -> Result<AppState> {
    Ok(AppState {
        registry: UserRegistry::open(config).await?,
        data_dir: config.data_dir.clone(),
        max_blob_bytes: config.max_blob_bytes,
    })
}

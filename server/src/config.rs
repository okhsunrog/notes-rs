use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    #[serde(default = "default_snapshot_interval")]
    pub snapshot_every_ops: u64,
    #[serde(default = "default_max_blob_bytes")]
    pub max_blob_bytes: u64,
    #[serde(default)]
    pub storage: StorageConfig,
    pub users: Vec<UserConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    pub id: String,
    pub tokens: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub embedding_provider_id: String,
    pub embedding_dimensions: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            embedding_provider_id: "openrouter:qwen/qwen3-embedding-8b".into(),
            embedding_dimensions: 4096,
        }
    }
}

impl ServerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading server config {}", path.display()))?;
        let config: Self = toml::from_str(&contents)
            .with_context(|| format!("parsing server config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.users.is_empty() {
            bail!("at least one user must be configured");
        }
        if self.storage.embedding_dimensions == 0 {
            bail!("storage.embedding_dimensions must be positive");
        }
        if self.max_blob_bytes == 0 {
            bail!("max_blob_bytes must be positive");
        }
        let mut user_ids = std::collections::HashSet::new();
        let mut tokens = std::collections::HashSet::new();
        for user in &self.users {
            if user.id.is_empty()
                || !user
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                bail!("user id must contain only ASCII letters, digits, '-' or '_'");
            }
            if !user_ids.insert(&user.id) {
                bail!("duplicate user id {}", user.id);
            }
            if user.tokens.is_empty() {
                bail!("user {} must have at least one token", user.id);
            }
            for token in &user.tokens {
                if token.len() < 32 {
                    bail!("tokens must contain at least 32 characters");
                }
                if !tokens.insert(token) {
                    bail!("the same token cannot be assigned more than once");
                }
            }
        }
        Ok(())
    }
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:8787".parse().expect("valid default address")
}

const fn default_snapshot_interval() -> u64 {
    10_000
}

const fn default_max_blob_bytes() -> u64 {
    1024 * 1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_shaped_user_ids_and_short_tokens() {
        let config = ServerConfig {
            listen: default_listen(),
            data_dir: "/tmp/notes".into(),
            snapshot_every_ops: 10_000,
            max_blob_bytes: 1,
            storage: StorageConfig::default(),
            users: vec![UserConfig {
                id: "../owner".into(),
                tokens: vec!["short".into()],
            }],
        };
        assert!(config.validate().is_err());
    }
}

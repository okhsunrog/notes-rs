use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_log_filter")]
    pub log_filter: String,
    pub data_dir: PathBuf,
    #[serde(default = "default_snapshot_interval")]
    pub snapshot_every_ops: u64,
    #[serde(default = "default_max_blob_bytes")]
    pub max_blob_bytes: u64,
    pub ai: Option<AiConfig>,
    pub users: Vec<UserConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    pub openrouter_api_key: String,
    pub openrouter_base_url: String,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub rerank_model: String,
    pub chat_model: String,
    pub extraction_model: String,
    pub automatic_embeddings: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            openrouter_api_key: String::new(),
            openrouter_base_url: notes_ai::config::DEFAULT_OPENROUTER_BASE_URL.into(),
            embedding_model: "qwen/qwen3-embedding-8b".into(),
            embedding_dimensions: 4096,
            rerank_model: "cohere/rerank-v3.5".into(),
            chat_model: "google/gemini-3.1-flash-lite".into(),
            extraction_model: "google/gemini-3.1-flash-lite".into(),
            automatic_embeddings: true,
            entity_extraction_enabled: true,
            query_rewriting_enabled: true,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    pub id: String,
    pub tokens: Vec<String>,
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
        tracing_subscriber::EnvFilter::try_new(&self.log_filter)
            .context("log_filter must be a valid tracing filter")?;
        if self.max_blob_bytes == 0 {
            bail!("max_blob_bytes must be positive");
        }
        if let Some(ai) = &self.ai {
            if ai.openrouter_api_key.trim().is_empty() {
                bail!("ai.openrouter_api_key cannot be empty");
            }
            notes_ai::config::validate_http_base_url(&ai.openrouter_base_url)?;
            if ai.embedding_dimensions == 0 {
                bail!("ai.embedding_dimensions must be positive");
            }
            for (name, value) in [
                ("embedding_model", &ai.embedding_model),
                ("rerank_model", &ai.rerank_model),
                ("chat_model", &ai.chat_model),
                ("extraction_model", &ai.extraction_model),
            ] {
                if value.trim().is_empty() {
                    bail!("ai.{name} cannot be empty");
                }
            }
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

fn default_log_filter() -> String {
    "info".into()
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
            log_filter: default_log_filter(),
            data_dir: "/tmp/notes".into(),
            snapshot_every_ops: 10_000,
            max_blob_bytes: 1,
            ai: None,
            users: vec![UserConfig {
                id: "../owner".into(),
                tokens: vec!["short".into()],
            }],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_invalid_ai_configuration() {
        let ai = AiConfig {
            openrouter_api_key: "test-key".into(),
            ..AiConfig::default()
        };
        let config = ServerConfig {
            listen: default_listen(),
            log_filter: default_log_filter(),
            data_dir: "/tmp/notes".into(),
            snapshot_every_ops: 10_000,
            max_blob_bytes: 1,
            ai: Some(AiConfig {
                embedding_dimensions: 0,
                ..ai
            }),
            users: vec![UserConfig {
                id: "owner".into(),
                tokens: vec!["a-token-with-at-least-thirty-two-characters".into()],
            }],
        };
        assert!(config.validate().is_err());
    }
}

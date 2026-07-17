use anyhow::{Context, Result, bail};
use notes_protocol::{AiProviderSettings, AiProviderSettingsUpdate, CompletionProtocol};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use url::Url;

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

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    pub retrieval_api_key: String,
    pub retrieval_base_url: Url,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub rerank_model: String,
    pub completion_protocol: CompletionProtocol,
    pub completion_api_key: String,
    pub completion_base_url: Url,
    pub chat_model: String,
    pub extraction_model: String,
    pub automatic_embeddings: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            retrieval_api_key: String::new(),
            retrieval_base_url: Url::parse(notes_ai::config::DEFAULT_OPENROUTER_BASE_URL)
                .expect("valid default retrieval URL"),
            embedding_model: "qwen/qwen3-embedding-8b".into(),
            embedding_dimensions: 4096,
            rerank_model: "cohere/rerank-v3.5".into(),
            completion_protocol: CompletionProtocol::Openai,
            completion_api_key: String::new(),
            completion_base_url: Url::parse(notes_ai::config::DEFAULT_OPENROUTER_BASE_URL)
                .expect("valid default completion URL"),
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
            ai.validate()?;
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

impl AiConfig {
    pub fn validate(&self) -> Result<()> {
        if self.retrieval_api_key.trim().is_empty() {
            bail!("AI retrieval API key cannot be empty");
        }
        if self.completion_api_key.trim().is_empty() {
            bail!("AI completion API key cannot be empty");
        }
        validate_provider_url(&self.retrieval_base_url)?;
        validate_provider_url(&self.completion_base_url)?;
        if self.embedding_dimensions == 0 {
            bail!("AI embedding dimensions must be positive");
        }
        for (name, value) in [
            ("embedding model", &self.embedding_model),
            ("rerank model", &self.rerank_model),
            ("chat model", &self.chat_model),
            ("extraction model", &self.extraction_model),
        ] {
            if value.trim().is_empty() {
                bail!("AI {name} cannot be empty");
            }
        }
        Ok(())
    }

    pub fn public_settings(&self) -> AiProviderSettings {
        AiProviderSettings {
            retrieval_base_url: self.retrieval_base_url.clone(),
            retrieval_api_key_configured: !self.retrieval_api_key.is_empty(),
            embedding_model: self.embedding_model.clone(),
            embedding_dimensions: self.embedding_dimensions,
            rerank_model: self.rerank_model.clone(),
            completion_protocol: self.completion_protocol,
            completion_base_url: self.completion_base_url.clone(),
            completion_api_key_configured: !self.completion_api_key.is_empty(),
            chat_model: self.chat_model.clone(),
            extraction_model: self.extraction_model.clone(),
        }
    }

    pub fn applying(&self, update: AiProviderSettingsUpdate) -> Self {
        Self {
            retrieval_api_key: update
                .retrieval_api_key
                .filter(|secret| !secret.trim().is_empty())
                .unwrap_or_else(|| self.retrieval_api_key.clone()),
            retrieval_base_url: update.retrieval_base_url,
            embedding_model: update.embedding_model,
            embedding_dimensions: update.embedding_dimensions,
            rerank_model: update.rerank_model,
            completion_protocol: update.completion_protocol,
            completion_api_key: update
                .completion_api_key
                .filter(|secret| !secret.trim().is_empty())
                .unwrap_or_else(|| self.completion_api_key.clone()),
            completion_base_url: update.completion_base_url,
            chat_model: update.chat_model,
            extraction_model: update.extraction_model,
            automatic_embeddings: self.automatic_embeddings,
            entity_extraction_enabled: self.entity_extraction_enabled,
            query_rewriting_enabled: self.query_rewriting_enabled,
        }
    }
}

fn validate_provider_url(url: &Url) -> Result<()> {
    notes_ai::config::validate_http_base_url(url.as_str())?;
    if !url.username().is_empty() || url.password().is_some() {
        bail!("AI provider base URL cannot include credentials");
    }
    Ok(())
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
            retrieval_api_key: "test-key".into(),
            completion_api_key: "test-key".into(),
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

    #[test]
    fn rejects_malformed_provider_urls_during_deserialization() {
        for field in ["retrieval_base_url", "completion_base_url"] {
            let contents = format!(r#"{field} = "not a URL""#);
            assert!(
                toml::from_str::<AiConfig>(&contents).is_err(),
                "{field} must be parsed as a URL, not retained as an unchecked string"
            );
        }
    }

    #[test]
    fn rejects_non_http_or_credentialed_provider_urls() {
        for invalid in [
            "ftp://provider.example.test/v1",
            "https://user:password@provider.example.test/v1",
            "https://provider.example.test/v1?secret=value",
        ] {
            let ai = AiConfig {
                retrieval_api_key: "retrieval-key".into(),
                completion_api_key: "completion-key".into(),
                retrieval_base_url: Url::parse(invalid).expect("syntactically valid URL"),
                ..AiConfig::default()
            };
            assert!(ai.validate().is_err(), "{invalid:?} must be rejected");
        }
    }

    #[test]
    fn provider_updates_preserve_write_only_secrets() {
        let current = AiConfig {
            retrieval_api_key: "retrieval-secret".into(),
            completion_api_key: "completion-secret".into(),
            ..AiConfig::default()
        };
        let mut public = current.public_settings();
        public.chat_model = "custom-chat".into();
        let updated = current.applying(AiProviderSettingsUpdate {
            retrieval_base_url: public.retrieval_base_url,
            retrieval_api_key: None,
            embedding_model: public.embedding_model,
            embedding_dimensions: public.embedding_dimensions,
            rerank_model: public.rerank_model,
            completion_protocol: public.completion_protocol,
            completion_base_url: public.completion_base_url,
            completion_api_key: None,
            chat_model: public.chat_model,
            extraction_model: public.extraction_model,
        });

        assert_eq!(updated.retrieval_api_key, "retrieval-secret");
        assert_eq!(updated.completion_api_key, "completion-secret");
        assert_eq!(updated.chat_model, "custom-chat");
        let serialized =
            serde_json::to_string(&updated.public_settings()).expect("public settings");
        assert!(!serialized.contains("retrieval-secret"));
        assert!(!serialized.contains("completion-secret"));
    }
}

use anyhow::{Context, Result, bail};
use notes_ai::embed::{EmbedderConfig, RerankerConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tauri::{AppHandle, Manager};

pub use notes_ai::config::DEFAULT_OPENROUTER_BASE_URL;
pub use notes_ai::config::{
    CompletionProtocol, EmbeddingProvider, ExtractionProtocol, RerankProvider,
};

const SETTINGS_VERSION: u32 = 1;
const DEFAULT_CHAT_MODEL: &str = "deepseek/deepseek-v4-flash";
const DEFAULT_EMBEDDING_MODEL: &str = "qwen/qwen3-embedding-8b";
const DEFAULT_RERANK_MODEL: &str = "cohere/rerank-v3.5";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

macro_rules! settings_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type,
        )]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(value: &str) -> Result<Self> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => bail!("unsupported {}: {value}", stringify!($name)),
                }
            }
        }
    };
}

settings_enum!(WindowDecorationMode {
    Native => "native",
    Borderless => "borderless",
});
settings_enum!(ProviderKeyScope {
    Chat => "chat",
    Extraction => "extraction",
});

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub enum SecretKey {
    #[serde(rename = "CHAT_API_KEY")]
    Chat,
    #[serde(rename = "EXTRACT_API_KEY")]
    Extraction,
    #[serde(rename = "OPENROUTER_API_KEY")]
    OpenRouter,
    #[serde(rename = "OPENAI_API_KEY")]
    OpenAi,
    #[serde(rename = "COHERE_API_KEY")]
    Cohere,
    #[serde(rename = "VOYAGE_API_KEY")]
    Voyage,
    #[serde(rename = "GEMINI_API_KEY")]
    Gemini,
    #[serde(rename = "SYNC_TOKEN")]
    SyncToken,
}

impl SecretKey {
    const ALL: [Self; 8] = [
        Self::Chat,
        Self::Extraction,
        Self::OpenRouter,
        Self::OpenAi,
        Self::Cohere,
        Self::Voyage,
        Self::Gemini,
        Self::SyncToken,
    ];
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub local_only: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
    pub chat_model: String,
    pub chat_protocol: CompletionProtocol,
    pub chat_base_url: url::Url,
    pub extraction_model: String,
    pub extraction_protocol: ExtractionProtocol,
    pub extraction_base_url: Option<url::Url>,
    pub embedding_provider: EmbeddingProvider,
    pub embedding_model: String,
    pub embedding_ndims: Option<u32>,
    pub rerank_provider: RerankProvider,
    pub rerank_model: String,
    pub openrouter_base_url: url::Url,
    pub openai_base_url: url::Url,
    pub window_decoration_mode: WindowDecorationMode,
    pub sync_directory: Option<PathBuf>,
    pub sync_server_url: Option<url::Url>,
    pub configured_keys: Vec<SecretKey>,
    pub local_models_available: bool,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub local_only: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
    pub chat_model: String,
    pub chat_protocol: CompletionProtocol,
    pub chat_base_url: url::Url,
    pub extraction_model: String,
    pub extraction_protocol: ExtractionProtocol,
    pub extraction_base_url: Option<url::Url>,
    pub embedding_provider: EmbeddingProvider,
    pub embedding_model: String,
    pub embedding_ndims: Option<u32>,
    pub rerank_provider: RerankProvider,
    pub rerank_model: String,
    pub openrouter_base_url: url::Url,
    pub openai_base_url: url::Url,
    pub window_decoration_mode: WindowDecorationMode,
    pub sync_directory: Option<PathBuf>,
    pub sync_server_url: Option<url::Url>,
    #[serde(default)]
    pub api_keys: BTreeMap<SecretKey, String>,
    #[serde(default)]
    pub clear_keys: Vec<SecretKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct StoredSettings {
    version: u32,
    local_only: bool,
    entity_extraction_enabled: bool,
    query_rewriting_enabled: bool,
    chat_model: String,
    chat_protocol: CompletionProtocol,
    chat_base_url: url::Url,
    extraction_model: String,
    extraction_protocol: ExtractionProtocol,
    extraction_base_url: Option<url::Url>,
    embedding_provider: EmbeddingProvider,
    embedding_model: String,
    embedding_ndims: Option<u32>,
    rerank_provider: RerankProvider,
    rerank_model: String,
    openrouter_base_url: url::Url,
    openai_base_url: url::Url,
    window_decoration_mode: WindowDecorationMode,
    sync_directory: Option<PathBuf>,
    sync_server_url: Option<url::Url>,
    secrets: BTreeMap<SecretKey, String>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            local_only: false,
            entity_extraction_enabled: true,
            query_rewriting_enabled: true,
            chat_model: DEFAULT_CHAT_MODEL.into(),
            chat_protocol: CompletionProtocol::Openai,
            chat_base_url: parse_default_url(DEFAULT_OPENROUTER_BASE_URL),
            extraction_model: DEFAULT_CHAT_MODEL.into(),
            extraction_protocol: ExtractionProtocol::Inherit,
            extraction_base_url: None,
            embedding_provider: EmbeddingProvider::Openrouter,
            embedding_model: DEFAULT_EMBEDDING_MODEL.into(),
            embedding_ndims: Some(4096),
            rerank_provider: RerankProvider::Openrouter,
            rerank_model: DEFAULT_RERANK_MODEL.into(),
            openrouter_base_url: parse_default_url(DEFAULT_OPENROUTER_BASE_URL),
            openai_base_url: parse_default_url(DEFAULT_OPENAI_BASE_URL),
            window_decoration_mode: WindowDecorationMode::Native,
            sync_directory: None,
            sync_server_url: None,
            secrets: BTreeMap::new(),
        }
    }
}

fn parse_default_url(value: &str) -> url::Url {
    value.parse().expect("built-in settings URL must be valid")
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeSettings {
    stored: StoredSettings,
}

impl RuntimeSettings {
    pub fn cloud_ai_enabled(&self) -> bool {
        !self.stored.local_only
    }

    pub fn entity_extraction_enabled(&self) -> bool {
        self.stored.entity_extraction_enabled && !self.stored.local_only
    }

    pub fn query_rewriting_enabled(&self) -> bool {
        self.stored.query_rewriting_enabled && !self.stored.local_only
    }

    pub fn embedding_provider_id(&self) -> String {
        format!(
            "{}:{}",
            self.stored.embedding_provider, self.stored.embedding_model
        )
    }

    pub fn embedding_dimensions(&self) -> Option<u32> {
        self.stored.embedding_ndims
    }

    pub fn embedder_config(&self) -> EmbedderConfig {
        let provider = self.stored.embedding_provider;
        let (base_url, secret_key) = match provider {
            EmbeddingProvider::Openrouter => (
                Some(self.stored.openrouter_base_url.to_string()),
                Some(SecretKey::OpenRouter),
            ),
            EmbeddingProvider::Openai => (
                Some(self.stored.openai_base_url.to_string()),
                Some(SecretKey::OpenAi),
            ),
            EmbeddingProvider::Cohere => (None, Some(SecretKey::Cohere)),
            EmbeddingProvider::Voyageai => (None, Some(SecretKey::Voyage)),
            EmbeddingProvider::Gemini => (None, Some(SecretKey::Gemini)),
            EmbeddingProvider::Local => (None, None),
        };
        EmbedderConfig {
            provider,
            model: self.stored.embedding_model.clone(),
            ndims: self.stored.embedding_ndims.map(|value| value as usize),
            base_url,
            api_key: secret_key.and_then(|key| self.secret(key)),
            local_only: self.stored.local_only,
        }
    }

    pub fn reranker_config(&self) -> RerankerConfig {
        RerankerConfig {
            provider: self.stored.rerank_provider,
            model: self.stored.rerank_model.clone(),
            base_url: (self.stored.rerank_provider == RerankProvider::Openrouter)
                .then(|| self.stored.openrouter_base_url.to_string()),
            api_key: (self.stored.rerank_provider == RerankProvider::Openrouter)
                .then(|| self.secret(SecretKey::OpenRouter))
                .flatten(),
            local_only: self.stored.local_only,
        }
    }

    pub fn chat_config(&self) -> Result<llm_relay::ClientConfig> {
        if self.stored.local_only {
            bail!("chat is disabled in local-only mode");
        }
        self.completion_config(
            self.stored.chat_protocol,
            self.stored.chat_base_url.clone(),
            self.stored.chat_model.clone(),
            SecretKey::Chat,
        )
    }

    pub fn extraction_config(&self) -> Result<llm_relay::ClientConfig> {
        if self.stored.local_only {
            bail!("entity extraction is disabled in local-only mode");
        }
        if self.stored.extraction_protocol == ExtractionProtocol::Inherit {
            let mut config = self.chat_config()?;
            config.model = self.stored.extraction_model.clone();
            return Ok(config);
        }
        self.completion_config(
            self.stored.extraction_protocol.into(),
            self.stored
                .extraction_base_url
                .clone()
                .context("extraction base URL is not configured")?,
            self.stored.extraction_model.clone(),
            SecretKey::Extraction,
        )
    }

    pub fn sync_credentials(&self) -> Result<Option<(url::Url, String)>> {
        let Some(server_url) = self.stored.sync_server_url.clone() else {
            return Ok(None);
        };
        let token = self
            .secret(SecretKey::SyncToken)
            .context("a sync token is required when a sync server is configured")?;
        Ok(Some((server_url, token)))
    }

    fn completion_config(
        &self,
        protocol: CompletionProtocol,
        base_url: url::Url,
        model: String,
        primary_key: SecretKey,
    ) -> Result<llm_relay::ClientConfig> {
        let api_key = self.secret(primary_key).or_else(|| {
            notes_ai::config::uses_openrouter_endpoint(protocol, Some(base_url.as_str()))
                .then(|| self.secret(SecretKey::OpenRouter))
                .flatten()
        });
        notes_ai::config::completion_config(protocol, Some(base_url.to_string()), api_key, model)
    }

    fn secret(&self, key: SecretKey) -> Option<String> {
        self.stored
            .secrets
            .get(&key)
            .filter(|value| !value.trim().is_empty())
            .cloned()
    }
}

pub fn config_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .context("resolving app data directory")?
        .join("settings.json"))
}

pub(crate) fn runtime(app: &AppHandle) -> Result<RuntimeSettings> {
    Ok(RuntimeSettings {
        stored: load_stored(app)?,
    })
}

pub fn load(app: &AppHandle) -> Result<SettingsSnapshot> {
    snapshot(load_stored(app)?, config_path(app)?)
}

fn snapshot(stored: StoredSettings, path: PathBuf) -> Result<SettingsSnapshot> {
    validate_stored(&stored)?;
    let configured_keys = SecretKey::ALL
        .into_iter()
        .filter(|key| {
            stored
                .secrets
                .get(key)
                .is_some_and(|value| !value.trim().is_empty())
        })
        .collect();
    Ok(SettingsSnapshot {
        local_only: stored.local_only,
        entity_extraction_enabled: stored.entity_extraction_enabled,
        query_rewriting_enabled: stored.query_rewriting_enabled,
        chat_model: stored.chat_model,
        chat_protocol: stored.chat_protocol,
        chat_base_url: stored.chat_base_url,
        extraction_model: stored.extraction_model,
        extraction_protocol: stored.extraction_protocol,
        extraction_base_url: stored.extraction_base_url,
        embedding_provider: stored.embedding_provider,
        embedding_model: stored.embedding_model,
        embedding_ndims: stored.embedding_ndims,
        rerank_provider: stored.rerank_provider,
        rerank_model: stored.rerank_model,
        openrouter_base_url: stored.openrouter_base_url,
        openai_base_url: stored.openai_base_url,
        window_decoration_mode: stored.window_decoration_mode,
        sync_directory: stored.sync_directory,
        sync_server_url: stored.sync_server_url,
        configured_keys,
        local_models_available: cfg!(feature = "local-models"),
        config_path: path,
    })
}

pub fn save(app: &AppHandle, update: SettingsUpdate) -> Result<SettingsSnapshot> {
    validate_update(&update)?;
    let mut stored = load_stored(app)?;
    stored.local_only = update.local_only;
    stored.entity_extraction_enabled = update.entity_extraction_enabled;
    stored.query_rewriting_enabled = update.query_rewriting_enabled;
    stored.chat_model = update.chat_model.trim().into();
    stored.chat_protocol = update.chat_protocol;
    stored.chat_base_url = update.chat_base_url;
    stored.extraction_model = update.extraction_model.trim().into();
    stored.extraction_protocol = update.extraction_protocol;
    stored.extraction_base_url = update.extraction_base_url;
    stored.embedding_provider = update.embedding_provider;
    stored.embedding_model = update.embedding_model.trim().into();
    stored.embedding_ndims = update.embedding_ndims;
    stored.rerank_provider = update.rerank_provider;
    stored.rerank_model = update.rerank_model.trim().into();
    stored.openrouter_base_url = update.openrouter_base_url;
    stored.openai_base_url = update.openai_base_url;
    stored.window_decoration_mode = update.window_decoration_mode;
    stored.sync_directory = update.sync_directory;
    stored.sync_server_url = update.sync_server_url;

    for key in update.clear_keys {
        stored.secrets.remove(&key);
    }
    for (key, secret) in update.api_keys {
        if !secret.trim().is_empty() {
            stored.secrets.insert(key, secret.trim().into());
        }
    }
    validate_stored(&stored)?;
    let path = config_path(app)?;
    write_settings(&path, &stored)?;
    apply_window_decorations(app, &stored.window_decoration_mode)?;
    snapshot(stored, path)
}

#[cfg(not(mobile))]
pub fn apply_saved_window_preferences(app: &AppHandle) -> Result<()> {
    let settings = load_stored(app)?;
    apply_window_decorations(app, &settings.window_decoration_mode)
}

fn apply_window_decorations(app: &AppHandle, mode: &WindowDecorationMode) -> Result<()> {
    #[cfg(not(mobile))]
    {
        app.get_webview_window("main")
            .context("main window is unavailable")?
            .set_decorations(*mode != WindowDecorationMode::Borderless)
            .context("applying window decorations")
    }

    #[cfg(mobile)]
    {
        let _ = (app, mode);
        Ok(())
    }
}

fn validate_update(update: &SettingsUpdate) -> Result<()> {
    if (update.embedding_provider == EmbeddingProvider::Local
        || update.rerank_provider == RerankProvider::Local)
        && !cfg!(feature = "local-models")
    {
        bail!("this build does not include the local-models feature");
    }
    if update.local_only {
        if !cfg!(feature = "local-models") {
            bail!("local-only mode requires a build with the local-models feature");
        }
        if update.embedding_provider != EmbeddingProvider::Local
            || update.rerank_provider != RerankProvider::Local
        {
            bail!("local-only mode requires local embeddings and local reranking");
        }
        if update.entity_extraction_enabled {
            bail!("entity extraction must be disabled in local-only mode");
        }
    }
    if update.chat_model.trim().is_empty()
        || update.extraction_model.trim().is_empty()
        || update.embedding_model.trim().is_empty()
        || update.rerank_model.trim().is_empty()
    {
        bail!("AI model IDs cannot be empty");
    }
    validate_url(&update.chat_base_url)?;
    if update.extraction_protocol != ExtractionProtocol::Inherit {
        validate_url(
            update
                .extraction_base_url
                .as_ref()
                .context("an extraction base URL is required for a separate protocol")?,
        )?;
    }
    if let Some(dimensions) = update.embedding_ndims
        && (dimensions == 0 || dimensions > 65_536)
    {
        bail!("embedding dimensions must be between 1 and 65536");
    }
    validate_url(&update.openrouter_base_url)?;
    validate_url(&update.openai_base_url)?;
    if let Some(server_url) = &update.sync_server_url {
        validate_url(server_url)?;
    }
    Ok(())
}

fn validate_stored(stored: &StoredSettings) -> Result<()> {
    if stored.version != SETTINGS_VERSION {
        bail!("unsupported settings format version {}", stored.version);
    }
    if stored.sync_server_url.is_some()
        && stored
            .secrets
            .get(&SecretKey::SyncToken)
            .is_none_or(|token| token.trim().is_empty())
    {
        bail!("a sync token is required when a sync server is configured");
    }
    Ok(())
}

fn validate_url(value: &url::Url) -> Result<()> {
    notes_ai::config::validate_http_base_url(value.as_str())
}

pub(crate) fn completion_config_for_probe(
    app: &AppHandle,
    protocol: CompletionProtocol,
    base_url: String,
    model: String,
    api_key: Option<String>,
    key_scope: Option<ProviderKeyScope>,
) -> Result<llm_relay::ClientConfig> {
    let runtime = runtime(app)?;
    let base_url = base_url.parse::<url::Url>().context("invalid base URL")?;
    validate_url(&base_url)?;
    let primary_key = match key_scope {
        Some(ProviderKeyScope::Extraction) => SecretKey::Extraction,
        Some(ProviderKeyScope::Chat) | None => SecretKey::Chat,
    };
    let api_key = api_key
        .filter(|value| !value.trim().is_empty())
        .or_else(|| runtime.secret(primary_key))
        .or_else(|| {
            notes_ai::config::uses_openrouter_endpoint(protocol, Some(base_url.as_str()))
                .then(|| runtime.secret(SecretKey::OpenRouter))
                .flatten()
        });
    notes_ai::config::completion_config(protocol, Some(base_url.to_string()), api_key, model)
}

fn load_stored(app: &AppHandle) -> Result<StoredSettings> {
    let path = config_path(app)?;
    if path.exists() {
        return read_settings(&path);
    }
    let stored = StoredSettings::default();
    validate_stored(&stored)?;
    write_settings(&path, &stored)?;
    read_settings(&path)
}

fn read_settings(path: &Path) -> Result<StoredSettings> {
    let body = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let settings = serde_json::from_slice::<StoredSettings>(&body)
        .with_context(|| format!("parsing {}", path.display()))?;
    validate_stored(&settings)?;
    Ok(settings)
}

fn write_settings(path: &Path, settings: &StoredSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = serde_json::to_vec_pretty(settings)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, body).with_context(|| format!("writing {}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_settings_round_trip_without_exposing_secrets() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let mut settings = StoredSettings::default();
        settings
            .secrets
            .insert(SecretKey::OpenRouter, "secret with quotes \"here\"".into());
        write_settings(&path, &settings).expect("write settings");
        assert_eq!(
            read_settings(&path)
                .expect("read settings")
                .secrets
                .get(&SecretKey::OpenRouter)
                .map(String::as_str),
            Some("secret with quotes \"here\"")
        );
    }
}

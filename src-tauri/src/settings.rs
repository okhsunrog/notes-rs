use anyhow::{Context, Result, bail};
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

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "CHAT_API_KEY",
            Self::Extraction => "EXTRACT_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Cohere => "COHERE_API_KEY",
            Self::Voyage => "VOYAGE_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::SyncToken => "SYNC_TOKEN",
        }
    }
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

pub fn config_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .context("resolving app data directory")?
        .join(".env"))
}

pub fn load(app: &AppHandle) -> Result<SettingsSnapshot> {
    let path = config_path(app)?;
    let values = read_env(&path)?;
    let value = |key: &str, default: &str| {
        values
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
            .unwrap_or_else(|| default.into())
    };
    let configured_keys = SecretKey::ALL
        .into_iter()
        .filter(|key| {
            values
                .get(key.as_str())
                .is_some_and(|secret| !secret.trim().is_empty())
                || std::env::var(key.as_str()).is_ok_and(|secret| !secret.trim().is_empty())
        })
        .collect();
    Ok(SettingsSnapshot {
        local_only: value("AI_LOCAL_ONLY", "false") == "true",
        entity_extraction_enabled: value("ENTITY_EXTRACTION_ENABLED", "true") == "true",
        query_rewriting_enabled: value("QUERY_REWRITING_ENABLED", "true") == "true",
        chat_model: value("CHAT_MODEL", "deepseek/deepseek-v4-flash"),
        chat_protocol: value("CHAT_PROTOCOL", "openai").parse()?,
        chat_base_url: value(
            "CHAT_BASE_URL",
            &value("OPENROUTER_BASE_URL", DEFAULT_OPENROUTER_BASE_URL),
        )
        .parse()
        .context("CHAT_BASE_URL must be an absolute URL")?,
        extraction_model: value("EXTRACT_MODEL", "deepseek/deepseek-v4-flash"),
        extraction_protocol: value("EXTRACT_PROTOCOL", "inherit").parse()?,
        extraction_base_url: non_empty(&value("EXTRACT_BASE_URL", ""))
            .map(str::parse)
            .transpose()
            .context("EXTRACT_BASE_URL must be an absolute URL")?,
        embedding_provider: value("EMBED_PROVIDER", "openrouter").parse()?,
        embedding_model: value("EMBED_MODEL", "qwen/qwen3-embedding-8b"),
        embedding_ndims: non_empty(&value("EMBED_NDIMS", "4096"))
            .map(str::parse)
            .transpose()
            .context("EMBED_NDIMS must be a positive integer")?,
        rerank_provider: value("RERANK_PROVIDER", "openrouter").parse()?,
        rerank_model: value("RERANK_MODEL", "cohere/rerank-v3.5"),
        openrouter_base_url: value("OPENROUTER_BASE_URL", DEFAULT_OPENROUTER_BASE_URL)
            .parse()
            .context("OPENROUTER_BASE_URL must be an absolute URL")?,
        openai_base_url: value("OPENAI_BASE_URL", "https://api.openai.com/v1")
            .parse()
            .context("OPENAI_BASE_URL must be an absolute URL")?,
        window_decoration_mode: match values
            .get("WINDOW_DECORATION_MODE")
            .cloned()
            .unwrap_or_else(|| {
                if value("WINDOW_DECORATIONS", "true") == "false" {
                    "borderless".into()
                } else {
                    "native".into()
                }
            })
            .as_str()
        {
            // Legacy mode: the patched tao no longer installs a CSD titlebar
            // on decorated Wayland windows, so KWin server-side decorations
            // are what "native" already produces.
            "kde" => WindowDecorationMode::Native,
            other => other.parse()?,
        },
        sync_directory: non_empty(&value("SYNC_DIRECTORY", "")).map(PathBuf::from),
        sync_server_url: non_empty(&value("SYNC_SERVER_URL", ""))
            .map(str::parse)
            .transpose()
            .context("SYNC_SERVER_URL must be an absolute URL")?,
        configured_keys,
        local_models_available: cfg!(feature = "local-models"),
        config_path: path,
    })
}

pub fn save(app: &AppHandle, update: SettingsUpdate) -> Result<SettingsSnapshot> {
    validate(&update)?;
    let path = config_path(app)?;
    let mut values = read_env(&path)?;
    values.insert("AI_LOCAL_ONLY".into(), update.local_only.to_string());
    values.insert(
        "ENTITY_EXTRACTION_ENABLED".into(),
        update.entity_extraction_enabled.to_string(),
    );
    values.insert(
        "QUERY_REWRITING_ENABLED".into(),
        update.query_rewriting_enabled.to_string(),
    );
    set_or_remove(&mut values, "CHAT_MODEL", update.chat_model);
    set_or_remove(
        &mut values,
        "CHAT_PROTOCOL",
        update.chat_protocol.to_string(),
    );
    set_or_remove(
        &mut values,
        "CHAT_BASE_URL",
        update.chat_base_url.to_string(),
    );
    set_or_remove(&mut values, "EXTRACT_MODEL", update.extraction_model);
    set_or_remove(
        &mut values,
        "EXTRACT_PROTOCOL",
        update.extraction_protocol.to_string(),
    );
    set_or_remove(
        &mut values,
        "EXTRACT_BASE_URL",
        update
            .extraction_base_url
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
    );
    set_or_remove(
        &mut values,
        "EMBED_PROVIDER",
        update.embedding_provider.to_string(),
    );
    set_or_remove(&mut values, "EMBED_MODEL", update.embedding_model);
    set_or_remove(
        &mut values,
        "EMBED_NDIMS",
        update
            .embedding_ndims
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    set_or_remove(
        &mut values,
        "RERANK_PROVIDER",
        update.rerank_provider.to_string(),
    );
    set_or_remove(&mut values, "RERANK_MODEL", update.rerank_model);
    set_or_remove(
        &mut values,
        "OPENROUTER_BASE_URL",
        update.openrouter_base_url.to_string(),
    );
    set_or_remove(
        &mut values,
        "OPENAI_BASE_URL",
        update.openai_base_url.to_string(),
    );
    values.remove("WINDOW_DECORATIONS");
    values.insert(
        "WINDOW_DECORATION_MODE".into(),
        update.window_decoration_mode.to_string(),
    );
    set_or_remove(
        &mut values,
        "SYNC_DIRECTORY",
        update
            .sync_directory
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    set_or_remove(
        &mut values,
        "SYNC_SERVER_URL",
        update
            .sync_server_url
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
    );

    for key in update.clear_keys {
        values.remove(key.as_str());
    }
    for (key, secret) in update.api_keys {
        if !secret.trim().is_empty() {
            values.insert(key.as_str().into(), secret.trim().to_string());
        }
    }
    if update.sync_server_url.is_some()
        && values
            .get(SecretKey::SyncToken.as_str())
            .is_none_or(|token| token.trim().is_empty())
    {
        bail!("a sync token is required when a sync server is configured");
    }
    write_env(&path, &values)?;
    apply_window_decorations(app, &update.window_decoration_mode)?;
    load(app)
}

pub fn apply_saved_window_preferences(app: &AppHandle) -> Result<()> {
    let settings = load(app)?;
    apply_window_decorations(app, &settings.window_decoration_mode)
}

fn apply_window_decorations(app: &AppHandle, mode: &WindowDecorationMode) -> Result<()> {
    app.get_webview_window("main")
        .context("main window is unavailable")?
        .set_decorations(*mode != WindowDecorationMode::Borderless)
        .context("applying window decorations")
}

fn validate(update: &SettingsUpdate) -> Result<()> {
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
            bail!(
                "entity extraction is cloud-only for now and must be disabled in local-only mode"
            );
        }
    }
    if update.chat_model.trim().is_empty() || update.extraction_model.trim().is_empty() {
        bail!("chat and extraction model IDs cannot be empty");
    }
    notes_ai::config::validate_http_base_url(update.chat_base_url.as_str())?;
    if update.extraction_protocol != ExtractionProtocol::Inherit {
        let extraction_base_url = update
            .extraction_base_url
            .as_ref()
            .context("an extraction base URL is required for a separate protocol")?;
        notes_ai::config::validate_http_base_url(extraction_base_url.as_str())?;
    }
    if let Some(dimensions) = update.embedding_ndims
        && (dimensions == 0 || dimensions > 65_536)
    {
        bail!("embedding dimensions must be between 1 and 65536");
    }
    notes_ai::config::validate_http_base_url(update.openrouter_base_url.as_str())?;
    notes_ai::config::validate_http_base_url(update.openai_base_url.as_str())?;
    if let Some(server_url) = &update.sync_server_url {
        notes_ai::config::validate_http_base_url(server_url.as_str())?;
    }
    Ok(())
}

pub fn sync_credentials(app: &AppHandle) -> Result<Option<(url::Url, String)>> {
    let values = read_env(&config_path(app)?)?;
    let Some(server_url) = values
        .get("SYNC_SERVER_URL")
        .filter(|url| !url.trim().is_empty())
    else {
        return Ok(None);
    };
    let server_url = server_url
        .parse::<url::Url>()
        .context("SYNC_SERVER_URL must be an absolute URL")?;
    notes_ai::config::validate_http_base_url(server_url.as_str())?;
    let token = values
        .get(SecretKey::SyncToken.as_str())
        .filter(|token| !token.trim().is_empty())
        .cloned()
        .context("a sync token is required when a sync server is configured")?;
    Ok(Some((server_url, token)))
}

pub(crate) fn completion_config_for_probe(
    protocol: CompletionProtocol,
    base_url: String,
    model: String,
    api_key: Option<String>,
    key_scope: Option<ProviderKeyScope>,
) -> Result<llm_relay::ClientConfig> {
    let configured_key = match key_scope {
        Some(ProviderKeyScope::Extraction) => std::env::var("EXTRACT_API_KEY").ok(),
        Some(ProviderKeyScope::Chat) | None => std::env::var("CHAT_API_KEY").ok(),
    };
    let api_key = api_key
        .filter(|value| !value.trim().is_empty())
        .or(configured_key)
        .or_else(|| notes_ai::config::legacy_openrouter_key(protocol, Some(&base_url)));
    notes_ai::config::completion_config(protocol, Some(base_url), api_key, model)
}

fn set_or_remove(values: &mut BTreeMap<String, String>, key: &str, value: String) {
    let value = value.trim();
    if value.is_empty() {
        values.remove(key);
    } else {
        values.insert(key.into(), value.into());
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn read_env(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    dotenvy::from_path_iter(path)
        .with_context(|| format!("reading {}", path.display()))?
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()
        .with_context(|| format!("parsing {}", path.display()))
}

fn write_env(path: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = values
        .iter()
        .map(|(key, value)| format!("{key}={}\n", quote(value)))
        .collect::<String>();
    let temporary = path.with_extension("env.tmp");
    std::fs::write(&temporary, body).with_context(|| format!("writing {}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_file_round_trips_escaped_values() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(".env");
        let mut values = BTreeMap::new();
        values.insert("OPENROUTER_API_KEY".into(), "secret with \"quotes\"".into());
        write_env(&path, &values).expect("write environment");
        assert_eq!(read_env(&path).expect("read environment"), values);
    }
}

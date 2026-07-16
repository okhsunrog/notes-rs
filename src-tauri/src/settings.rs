use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tauri::{AppHandle, Manager};

pub use notes_ai::config::DEFAULT_OPENROUTER_BASE_URL;
pub use notes_ai::config::{
    CompletionProtocol, EmbeddingProvider, ExtractionProtocol, RerankProvider,
};

const SECRET_KEYS: &[&str] = &[
    "CHAT_API_KEY",
    "EXTRACT_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "COHERE_API_KEY",
    "VOYAGE_API_KEY",
    "GEMINI_API_KEY",
];

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
    Kde => "kde",
});
settings_enum!(ProviderKeyScope {
    Chat => "chat",
    Extraction => "extraction",
});

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub local_only: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
    pub chat_model: String,
    pub chat_protocol: CompletionProtocol,
    pub chat_base_url: String,
    pub extraction_model: String,
    pub extraction_protocol: ExtractionProtocol,
    pub extraction_base_url: String,
    pub embedding_provider: EmbeddingProvider,
    pub embedding_model: String,
    pub embedding_ndims: String,
    pub rerank_provider: RerankProvider,
    pub rerank_model: String,
    pub openrouter_base_url: String,
    pub openai_base_url: String,
    pub window_decoration_mode: WindowDecorationMode,
    pub sync_directory: String,
    pub kde_decorations_available: bool,
    pub configured_keys: Vec<String>,
    pub local_models_available: bool,
    pub config_path: String,
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub local_only: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
    pub chat_model: String,
    pub chat_protocol: CompletionProtocol,
    pub chat_base_url: String,
    pub extraction_model: String,
    pub extraction_protocol: ExtractionProtocol,
    pub extraction_base_url: String,
    pub embedding_provider: EmbeddingProvider,
    pub embedding_model: String,
    pub embedding_ndims: String,
    pub rerank_provider: RerankProvider,
    pub rerank_model: String,
    pub openrouter_base_url: String,
    pub openai_base_url: String,
    pub window_decoration_mode: WindowDecorationMode,
    pub sync_directory: String,
    #[serde(default)]
    pub api_keys: BTreeMap<String, String>,
    #[serde(default)]
    pub clear_keys: Vec<String>,
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
    let configured_keys = SECRET_KEYS
        .iter()
        .filter(|key| {
            values
                .get(**key)
                .is_some_and(|secret| !secret.trim().is_empty())
                || std::env::var(key).is_ok_and(|secret| !secret.trim().is_empty())
        })
        .map(|key| (*key).to_string())
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
        ),
        extraction_model: value("EXTRACT_MODEL", "deepseek/deepseek-v4-flash"),
        extraction_protocol: value("EXTRACT_PROTOCOL", "inherit").parse()?,
        extraction_base_url: value("EXTRACT_BASE_URL", ""),
        embedding_provider: value("EMBED_PROVIDER", "openrouter").parse()?,
        embedding_model: value("EMBED_MODEL", "qwen/qwen3-embedding-8b"),
        embedding_ndims: value("EMBED_NDIMS", "4096"),
        rerank_provider: value("RERANK_PROVIDER", "openrouter").parse()?,
        rerank_model: value("RERANK_MODEL", "cohere/rerank-v3.5"),
        openrouter_base_url: value("OPENROUTER_BASE_URL", DEFAULT_OPENROUTER_BASE_URL),
        openai_base_url: value("OPENAI_BASE_URL", "https://api.openai.com/v1"),
        window_decoration_mode: values
            .get("WINDOW_DECORATION_MODE")
            .cloned()
            .unwrap_or_else(|| {
                if value("WINDOW_DECORATIONS", "true") == "false" {
                    "borderless".into()
                } else {
                    "native".into()
                }
            })
            .parse()?,
        sync_directory: value("SYNC_DIRECTORY", ""),
        kde_decorations_available: cfg!(target_os = "linux")
            && std::env::var_os("WAYLAND_DISPLAY").is_some(),
        configured_keys,
        local_models_available: cfg!(feature = "local-models"),
        config_path: path.display().to_string(),
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
    set_or_remove(&mut values, "CHAT_BASE_URL", update.chat_base_url);
    set_or_remove(&mut values, "EXTRACT_MODEL", update.extraction_model);
    set_or_remove(
        &mut values,
        "EXTRACT_PROTOCOL",
        update.extraction_protocol.to_string(),
    );
    set_or_remove(&mut values, "EXTRACT_BASE_URL", update.extraction_base_url);
    set_or_remove(
        &mut values,
        "EMBED_PROVIDER",
        update.embedding_provider.to_string(),
    );
    set_or_remove(&mut values, "EMBED_MODEL", update.embedding_model);
    set_or_remove(&mut values, "EMBED_NDIMS", update.embedding_ndims);
    set_or_remove(
        &mut values,
        "RERANK_PROVIDER",
        update.rerank_provider.to_string(),
    );
    set_or_remove(&mut values, "RERANK_MODEL", update.rerank_model);
    set_or_remove(
        &mut values,
        "OPENROUTER_BASE_URL",
        update.openrouter_base_url,
    );
    set_or_remove(&mut values, "OPENAI_BASE_URL", update.openai_base_url);
    values.remove("WINDOW_DECORATIONS");
    values.insert(
        "WINDOW_DECORATION_MODE".into(),
        update.window_decoration_mode.to_string(),
    );
    set_or_remove(&mut values, "SYNC_DIRECTORY", update.sync_directory);

    let allowed: BTreeSet<&str> = SECRET_KEYS.iter().copied().collect();
    for key in update.clear_keys {
        if allowed.contains(key.as_str()) {
            values.remove(&key);
        }
    }
    for (key, secret) in update.api_keys {
        if allowed.contains(key.as_str()) && !secret.trim().is_empty() {
            values.insert(key, secret.trim().to_string());
        }
    }
    write_env(&path, &values)?;
    apply_window_decorations(app, &update.window_decoration_mode)?;
    load(app)
}

pub fn apply_saved_window_preferences(app: &AppHandle) -> Result<()> {
    let settings = load(app)?;
    #[cfg(target_os = "linux")]
    if settings.window_decoration_mode == WindowDecorationMode::Kde {
        use gtk::prelude::GtkWindowExt;
        let window = app
            .get_webview_window("main")
            .context("main window is unavailable")?
            .gtk_window()
            .context("accessing the GTK window")?;
        // Tao installs a custom GtkHeaderBar on every Wayland window. GTK
        // always treats a custom titlebar as CSD, so remove it and let GTK's
        // Wayland backend negotiate server decorations with KWin.
        window.set_titlebar(None::<&gtk::Widget>);
    }
    apply_window_decorations(app, &settings.window_decoration_mode)
}

fn apply_window_decorations(app: &AppHandle, mode: &WindowDecorationMode) -> Result<()> {
    app.get_webview_window("main")
        .context("main window is unavailable")?
        .set_decorations(*mode != WindowDecorationMode::Borderless)
        .context("applying window decorations")
}

/// Must run before Tauri initializes GTK. `GTK_CSD=0` makes undecorated GTK3
/// windows ask the Wayland compositor to provide decorations. The custom
/// titlebar installed by Tao is removed later in `apply_saved_window_preferences`.
#[cfg(target_os = "linux")]
pub fn prepare_linux_window_backend() -> Result<()> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .context("HOME and XDG_DATA_HOME are both unavailable")?;
    let path = data_home.join("dev.okhsunrog.notes-rs/.env");
    let values = read_env(&path)?;
    if values.get("WINDOW_DECORATION_MODE").is_some_and(|mode| {
        mode.parse::<WindowDecorationMode>()
            .is_ok_and(|mode| mode == WindowDecorationMode::Kde)
    }) {
        // SAFETY: main calls this before Tauri starts GTK or any threads.
        unsafe {
            std::env::set_var("GDK_BACKEND", "wayland");
            std::env::set_var("GTK_CSD", "0");
        }
    }
    Ok(())
}

fn validate(update: &SettingsUpdate) -> Result<()> {
    if update.window_decoration_mode == WindowDecorationMode::Kde
        && (!cfg!(target_os = "linux") || std::env::var_os("WAYLAND_DISPLAY").is_none())
    {
        bail!("KDE decorations require Linux and a native Wayland session");
    }
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
    notes_ai::config::validate_http_base_url(&update.chat_base_url)?;
    if update.extraction_protocol != ExtractionProtocol::Inherit {
        notes_ai::config::validate_http_base_url(&update.extraction_base_url)?;
    }
    if !update.embedding_ndims.trim().is_empty() {
        let dimensions = update
            .embedding_ndims
            .trim()
            .parse::<usize>()
            .context("embedding dimensions must be a positive integer")?;
        if dimensions == 0 || dimensions > 65_536 {
            bail!("embedding dimensions must be between 1 and 65536");
        }
    }
    notes_ai::config::validate_http_base_url(&update.openrouter_base_url)?;
    notes_ai::config::validate_http_base_url(&update.openai_base_url)?;
    Ok(())
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

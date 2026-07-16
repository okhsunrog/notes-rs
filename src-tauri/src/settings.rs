use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

const SECRET_KEYS: &[&str] = &[
    "CHAT_API_KEY",
    "EXTRACT_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "COHERE_API_KEY",
    "VOYAGE_API_KEY",
    "GEMINI_API_KEY",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub local_only: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
    pub chat_model: String,
    pub chat_protocol: String,
    pub chat_base_url: String,
    pub extraction_model: String,
    pub extraction_protocol: String,
    pub extraction_base_url: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_ndims: String,
    pub rerank_provider: String,
    pub rerank_model: String,
    pub openrouter_base_url: String,
    pub openai_base_url: String,
    pub window_decoration_mode: String,
    pub sync_directory: String,
    pub kde_decorations_available: bool,
    pub configured_keys: Vec<String>,
    pub local_models_available: bool,
    pub config_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub local_only: bool,
    pub entity_extraction_enabled: bool,
    pub query_rewriting_enabled: bool,
    pub chat_model: String,
    pub chat_protocol: String,
    pub chat_base_url: String,
    pub extraction_model: String,
    pub extraction_protocol: String,
    pub extraction_base_url: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_ndims: String,
    pub rerank_provider: String,
    pub rerank_model: String,
    pub openrouter_base_url: String,
    pub openai_base_url: String,
    pub window_decoration_mode: String,
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
        chat_protocol: value("CHAT_PROTOCOL", "openai"),
        chat_base_url: value(
            "CHAT_BASE_URL",
            &value("OPENROUTER_BASE_URL", DEFAULT_OPENROUTER_BASE_URL),
        ),
        extraction_model: value("EXTRACT_MODEL", "deepseek/deepseek-v4-flash"),
        extraction_protocol: value("EXTRACT_PROTOCOL", "inherit"),
        extraction_base_url: value("EXTRACT_BASE_URL", ""),
        embedding_provider: value("EMBED_PROVIDER", "openrouter"),
        embedding_model: value("EMBED_MODEL", "qwen/qwen3-embedding-8b"),
        embedding_ndims: value("EMBED_NDIMS", "4096"),
        rerank_provider: value("RERANK_PROVIDER", "openrouter"),
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
            }),
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
    set_or_remove(&mut values, "CHAT_PROTOCOL", update.chat_protocol);
    set_or_remove(&mut values, "CHAT_BASE_URL", update.chat_base_url);
    set_or_remove(&mut values, "EXTRACT_MODEL", update.extraction_model);
    set_or_remove(&mut values, "EXTRACT_PROTOCOL", update.extraction_protocol);
    set_or_remove(&mut values, "EXTRACT_BASE_URL", update.extraction_base_url);
    set_or_remove(&mut values, "EMBED_PROVIDER", update.embedding_provider);
    set_or_remove(&mut values, "EMBED_MODEL", update.embedding_model);
    set_or_remove(&mut values, "EMBED_NDIMS", update.embedding_ndims);
    set_or_remove(&mut values, "RERANK_PROVIDER", update.rerank_provider);
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
        update.window_decoration_mode.clone(),
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
    if settings.window_decoration_mode == "kde" {
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

fn apply_window_decorations(app: &AppHandle, mode: &str) -> Result<()> {
    app.get_webview_window("main")
        .context("main window is unavailable")?
        .set_decorations(mode != "borderless")
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
    if values
        .get("WINDOW_DECORATION_MODE")
        .is_some_and(|mode| mode == "kde")
    {
        // SAFETY: main calls this before Tauri starts GTK or any threads.
        unsafe {
            std::env::set_var("GDK_BACKEND", "wayland");
            std::env::set_var("GTK_CSD", "0");
        }
    }
    Ok(())
}

fn validate(update: &SettingsUpdate) -> Result<()> {
    const EMBED_PROVIDERS: &[&str] = &[
        "openrouter",
        "openai",
        "cohere",
        "voyageai",
        "gemini",
        "local",
    ];
    if !EMBED_PROVIDERS.contains(&update.embedding_provider.as_str()) {
        bail!("unsupported embedding provider");
    }
    if !matches!(update.rerank_provider.as_str(), "openrouter" | "local") {
        bail!("unsupported rerank provider");
    }
    if !matches!(
        update.window_decoration_mode.as_str(),
        "native" | "borderless" | "kde"
    ) {
        bail!("unsupported window decoration mode");
    }
    if update.window_decoration_mode == "kde"
        && (!cfg!(target_os = "linux") || std::env::var_os("WAYLAND_DISPLAY").is_none())
    {
        bail!("KDE decorations require Linux and a native Wayland session");
    }
    if (update.embedding_provider == "local" || update.rerank_provider == "local")
        && !cfg!(feature = "local-models")
    {
        bail!("this build does not include the local-models feature");
    }
    if update.local_only {
        if !cfg!(feature = "local-models") {
            bail!("local-only mode requires a build with the local-models feature");
        }
        if update.embedding_provider != "local" || update.rerank_provider != "local" {
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
    if !matches!(update.chat_protocol.as_str(), "openai" | "anthropic") {
        bail!("chat protocol must be openai or anthropic");
    }
    if !matches!(
        update.extraction_protocol.as_str(),
        "inherit" | "openai" | "anthropic"
    ) {
        bail!("extraction protocol must inherit chat, use openai, or use anthropic");
    }
    validate_http_base_url(&update.chat_base_url)?;
    if update.extraction_protocol != "inherit" {
        validate_http_base_url(&update.extraction_base_url)?;
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
    validate_http_base_url(&update.openrouter_base_url)?;
    validate_http_base_url(&update.openai_base_url)?;
    Ok(())
}

fn validate_http_base_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value.trim()).context("base URL must be a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("base URL must use http or https and include a host");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("base URL cannot contain a query string or fragment");
    }
    Ok(())
}

pub fn openrouter_client() -> Result<rig::providers::openrouter::Client> {
    let api_key = std::env::var("OPENROUTER_API_KEY").context("OPENROUTER_API_KEY not set")?;
    let base_url =
        std::env::var("OPENROUTER_BASE_URL").unwrap_or_else(|_| DEFAULT_OPENROUTER_BASE_URL.into());
    openrouter_client_from(api_key, &base_url)
}

fn openrouter_client_from(
    api_key: String,
    base_url: &str,
) -> Result<rig::providers::openrouter::Client> {
    validate_http_base_url(base_url)?;
    rig::providers::openrouter::Client::builder()
        .base_url(base_url.trim_end_matches('/'))
        .api_key(api_key)
        .build()
        .context("building OpenRouter client")
}

fn env_flag(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
        .unwrap_or(default)
}

pub fn local_only() -> bool {
    env_flag("AI_LOCAL_ONLY", false)
}

pub fn entity_extraction_enabled() -> bool {
    env_flag("ENTITY_EXTRACTION_ENABLED", true) && !local_only()
}

pub fn query_rewriting_enabled() -> bool {
    env_flag("QUERY_REWRITING_ENABLED", true) && !local_only()
}

pub fn chat_model() -> String {
    std::env::var("CHAT_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4-flash".into())
}

pub fn extraction_model() -> String {
    std::env::var("EXTRACT_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4-flash".into())
}

pub fn chat_completion_config() -> Result<llm_relay::ClientConfig> {
    let protocol = std::env::var("CHAT_PROTOCOL").unwrap_or_else(|_| "openai".into());
    let base_url = std::env::var("CHAT_BASE_URL")
        .ok()
        .or_else(|| std::env::var("OPENROUTER_BASE_URL").ok());
    let api_key = std::env::var("CHAT_API_KEY")
        .ok()
        .or_else(|| legacy_openrouter_key(&protocol, base_url.as_deref()));
    completion_config(&protocol, base_url, api_key, chat_model())
}

pub fn extraction_completion_config() -> Result<llm_relay::ClientConfig> {
    let protocol = std::env::var("EXTRACT_PROTOCOL").unwrap_or_else(|_| "inherit".into());
    if protocol == "inherit" {
        let mut config = chat_completion_config()?;
        config.model = extraction_model();
        return Ok(config);
    }
    let base_url = std::env::var("EXTRACT_BASE_URL").ok();
    let api_key = std::env::var("EXTRACT_API_KEY")
        .ok()
        .or_else(|| std::env::var("CHAT_API_KEY").ok())
        .or_else(|| legacy_openrouter_key(&protocol, base_url.as_deref()));
    completion_config(&protocol, base_url, api_key, extraction_model())
}

fn legacy_openrouter_key(protocol: &str, base_url: Option<&str>) -> Option<String> {
    if protocol != "openai" {
        return None;
    }
    let host = base_url
        .and_then(|value| reqwest::Url::parse(value).ok())
        .and_then(|url| url.host_str().map(str::to_owned));
    if host.as_deref() == Some("openrouter.ai") {
        std::env::var("OPENROUTER_API_KEY").ok()
    } else {
        None
    }
}

pub(crate) fn completion_config(
    protocol: &str,
    base_url: Option<String>,
    api_key: Option<String>,
    model: String,
) -> Result<llm_relay::ClientConfig> {
    let api_key = api_key.unwrap_or_default();
    match protocol {
        "openai" => {
            let base_url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".into());
            let config = llm_relay::ClientConfig::openai_compatible(base_url, api_key, model);
            if config.api_key.is_empty() {
                Ok(config.without_auth())
            } else {
                Ok(config)
            }
        }
        "anthropic" => Ok(llm_relay::ClientConfig::anthropic(api_key, model)
            .base_url(base_url.unwrap_or_else(|| "https://api.anthropic.com".into()))),
        other => bail!("unsupported completion protocol: {other}"),
    }
}

pub(crate) fn completion_config_for_probe(
    protocol: &str,
    base_url: String,
    model: String,
    api_key: Option<String>,
) -> Result<llm_relay::ClientConfig> {
    let api_key = api_key
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("CHAT_API_KEY").ok())
        .or_else(|| legacy_openrouter_key(protocol, Some(&base_url)));
    completion_config(protocol, Some(base_url), api_key, model)
}

pub fn ensure_cloud_ai_allowed(feature: &str) -> Result<()> {
    if local_only() {
        bail!("{feature} is disabled in local-only mode because no local chat model is configured")
    }
    Ok(())
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

    #[test]
    fn validates_provider_base_urls() {
        assert!(validate_http_base_url("https://proxy.example/v1/").is_ok());
        assert!(validate_http_base_url("http://127.0.0.1:11434/v1").is_ok());
        assert!(validate_http_base_url("file:///tmp/api").is_err());
        assert!(validate_http_base_url("https://proxy.example/v1?token=secret").is_err());
    }

    #[test]
    fn openrouter_client_honors_custom_base_url() {
        let client = openrouter_client_from("test-key".into(), "http://127.0.0.1:11434/custom/v1/")
            .expect("build custom client");
        assert_eq!(client.base_url(), "http://127.0.0.1:11434/custom/v1");
    }

    #[test]
    fn builds_protocol_neutral_completion_configs() {
        let openai = completion_config(
            "openai",
            Some("http://localhost:11434/v1".into()),
            None,
            "qwen3".into(),
        )
        .expect("local OpenAI-compatible config");
        assert_eq!(openai.provider, llm_relay::Provider::OpenAiCompatible);
        assert_eq!(openai.auth_scheme, llm_relay::AuthScheme::None);

        let anthropic = completion_config(
            "anthropic",
            Some("https://proxy.example/anthropic".into()),
            Some("secret".into()),
            "custom-claude".into(),
        )
        .expect("Anthropic-compatible config");
        assert_eq!(anthropic.provider, llm_relay::Provider::Anthropic);
        assert_eq!(
            anthropic.auth_scheme,
            llm_relay::AuthScheme::Header("x-api-key".into())
        );
    }
}

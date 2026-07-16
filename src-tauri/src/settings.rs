use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const SECRET_KEYS: &[&str] = &[
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "COHERE_API_KEY",
    "VOYAGE_API_KEY",
    "GEMINI_API_KEY",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_ndims: String,
    pub rerank_provider: String,
    pub rerank_model: String,
    pub openrouter_base_url: String,
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
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_ndims: String,
    pub rerank_provider: String,
    pub rerank_model: String,
    pub openrouter_base_url: String,
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
        embedding_provider: value("EMBED_PROVIDER", "openrouter"),
        embedding_model: value("EMBED_MODEL", "qwen/qwen3-embedding-8b"),
        embedding_ndims: value("EMBED_NDIMS", "4096"),
        rerank_provider: value("RERANK_PROVIDER", "openrouter"),
        rerank_model: value("RERANK_MODEL", "cohere/rerank-v3.5"),
        openrouter_base_url: value("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1"),
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
}

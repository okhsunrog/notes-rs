use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tauri::{AppHandle, Manager};

const SETTINGS_VERSION: u32 = 3;

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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub enum SecretKey {
    #[serde(rename = "SYNC_TOKEN")]
    SyncToken,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub window_decoration_mode: WindowDecorationMode,
    pub sync_server_url: Option<url::Url>,
    pub configured_keys: Vec<SecretKey>,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub window_decoration_mode: WindowDecorationMode,
    pub sync_server_url: Option<url::Url>,
    #[serde(default)]
    pub api_keys: BTreeMap<SecretKey, String>,
    #[serde(default)]
    pub clear_keys: Vec<SecretKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredSettings {
    version: u32,
    window_decoration_mode: WindowDecorationMode,
    sync_server_url: Option<url::Url>,
    secrets: BTreeMap<SecretKey, String>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            window_decoration_mode: WindowDecorationMode::Native,
            sync_server_url: None,
            secrets: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeSettings {
    stored: StoredSettings,
}

impl RuntimeSettings {
    pub fn sync_credentials(&self) -> Result<Option<(url::Url, String)>> {
        let Some(server_url) = self.stored.sync_server_url.clone() else {
            return Ok(None);
        };
        let token = self
            .stored
            .secrets
            .get(&SecretKey::SyncToken)
            .filter(|token| !token.trim().is_empty())
            .cloned()
            .context("a sync token is required when a sync server is configured")?;
        Ok(Some((server_url, token)))
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
    let configured_keys = stored
        .secrets
        .get(&SecretKey::SyncToken)
        .is_some_and(|token| !token.trim().is_empty())
        .then_some(SecretKey::SyncToken)
        .into_iter()
        .collect();
    Ok(SettingsSnapshot {
        window_decoration_mode: stored.window_decoration_mode,
        sync_server_url: stored.sync_server_url,
        configured_keys,
        config_path: path,
    })
}

pub fn save(app: &AppHandle, update: SettingsUpdate) -> Result<SettingsSnapshot> {
    validate_update(&update)?;
    let mut stored = load_stored(app)?;
    stored.window_decoration_mode = update.window_decoration_mode;
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

pub fn reset(app: &AppHandle) -> Result<SettingsSnapshot> {
    let stored = StoredSettings::default();
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
    if let Some(server_url) = &update.sync_server_url {
        validate_http_url(server_url)?;
    }
    Ok(())
}

fn validate_stored(stored: &StoredSettings) -> Result<()> {
    if stored.version != SETTINGS_VERSION {
        bail!(
            "unsupported settings format version {}; remove settings.json and configure this development build again",
            stored.version
        );
    }
    if let Some(server_url) = &stored.sync_server_url {
        validate_http_url(server_url)?;
        if stored
            .secrets
            .get(&SecretKey::SyncToken)
            .is_none_or(|token| token.trim().is_empty())
        {
            bail!("a sync token is required when a sync server is configured");
        }
    }
    Ok(())
}

fn validate_http_url(value: &url::Url) -> Result<()> {
    if !matches!(value.scheme(), "http" | "https") {
        bail!("server URL must use HTTP or HTTPS");
    }
    if value.host_str().is_none() {
        bail!("server URL must include a host");
    }
    if !value.username().is_empty() || value.password().is_some() {
        bail!("server URL cannot include credentials");
    }
    if value.query().is_some() || value.fragment().is_some() {
        bail!("server URL cannot include a query or fragment");
    }
    Ok(())
}

fn load_stored(app: &AppHandle) -> Result<StoredSettings> {
    let path = config_path(app)?;
    if path.exists() {
        return read_settings(&path);
    }
    let stored = StoredSettings::default();
    write_settings(&path, &stored)?;
    Ok(stored)
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
    fn stored_settings_contain_only_device_and_server_connection_state() {
        let value = serde_json::to_value(StoredSettings::default()).expect("serialize settings");
        let object = value.as_object().expect("settings object");
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "secrets",
                "syncServerUrl",
                "version",
                "windowDecorationMode",
            ]
        );
    }
}

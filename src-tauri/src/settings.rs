use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tauri::{AppHandle, Manager};

const SETTINGS_VERSION: u32 = 3;
const fn default_window_corner_radius() -> u8 {
    10
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AiSearchTrigger {
    #[default]
    AsYouType,
    EnterOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum StartupView {
    #[default]
    Dashboard,
    LastSession,
    Today,
    SpecificPage,
}

const fn default_true() -> bool {
    true
}

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
    pub window_corner_radius: u8,
    pub window_corner_rounding_supported: bool,
    pub window_decoration_mode: WindowDecorationMode,
    pub active_window_decoration_mode: WindowDecorationMode,
    pub window_decorations_require_restart: bool,
    pub startup_view: StartupView,
    pub startup_page_uuid: Option<uuid::Uuid>,
    pub sync_server_url: Option<url::Url>,
    pub ai_search_enabled: bool,
    pub ai_search_trigger: AiSearchTrigger,
    pub ai_search_rerank: bool,
    pub search_debug_sources: bool,
    pub configured_keys: Vec<SecretKey>,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub window_corner_radius: u8,
    pub window_decoration_mode: WindowDecorationMode,
    pub startup_view: StartupView,
    pub startup_page_uuid: Option<uuid::Uuid>,
    pub sync_server_url: Option<url::Url>,
    pub ai_search_enabled: bool,
    pub ai_search_trigger: AiSearchTrigger,
    pub ai_search_rerank: bool,
    pub search_debug_sources: bool,
    #[serde(default)]
    pub api_keys: BTreeMap<SecretKey, String>,
    #[serde(default)]
    pub clear_keys: Vec<SecretKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredSettings {
    #[serde(default = "default_window_corner_radius")]
    window_corner_radius: u8,
    version: u32,
    window_decoration_mode: WindowDecorationMode,
    #[serde(default)]
    startup_view: StartupView,
    #[serde(default)]
    startup_page_uuid: Option<uuid::Uuid>,
    sync_server_url: Option<url::Url>,
    #[serde(default = "default_true")]
    ai_search_enabled: bool,
    #[serde(default)]
    ai_search_trigger: AiSearchTrigger,
    #[serde(default = "default_true")]
    ai_search_rerank: bool,
    #[serde(default)]
    search_debug_sources: bool,
    secrets: BTreeMap<SecretKey, String>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            window_corner_radius: default_window_corner_radius(),
            version: SETTINGS_VERSION,
            window_decoration_mode: WindowDecorationMode::Native,
            startup_view: StartupView::Dashboard,
            startup_page_uuid: None,
            sync_server_url: None,
            ai_search_enabled: true,
            ai_search_trigger: AiSearchTrigger::AsYouType,
            ai_search_rerank: true,
            search_debug_sources: false,
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
    live_snapshot(app, load_stored(app)?, config_path(app)?)
}

#[cfg(not(mobile))]
struct WindowDecorations {
    active: std::sync::Mutex<WindowDecorationMode>,
    requires_restart: bool,
}

#[cfg(any(not(mobile), test))]
fn should_apply_decorations(
    requires_restart: bool,
    active: WindowDecorationMode,
    requested: WindowDecorationMode,
) -> bool {
    !requires_restart && active != requested
}

fn live_snapshot(
    app: &AppHandle,
    stored: StoredSettings,
    path: PathBuf,
) -> Result<SettingsSnapshot> {
    let mut result = snapshot(stored, path)?;
    #[cfg(not(mobile))]
    {
        let state = app.state::<WindowDecorations>();
        result.active_window_decoration_mode =
            *state.active.lock().unwrap_or_else(|e| e.into_inner());
        result.window_decorations_require_restart = state.requires_restart;
    }
    #[cfg(mobile)]
    {
        let _ = app;
        result.active_window_decoration_mode = WindowDecorationMode::Native;
    }
    Ok(result)
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
        window_corner_radius: stored.window_corner_radius,
        window_corner_rounding_supported: cfg!(any(target_os = "linux", target_os = "windows")),
        window_decoration_mode: stored.window_decoration_mode,
        active_window_decoration_mode: stored.window_decoration_mode,
        window_decorations_require_restart: false,
        startup_view: stored.startup_view,
        startup_page_uuid: stored.startup_page_uuid,
        sync_server_url: stored.sync_server_url,
        ai_search_enabled: stored.ai_search_enabled,
        ai_search_trigger: stored.ai_search_trigger,
        ai_search_rerank: stored.ai_search_rerank,
        search_debug_sources: stored.search_debug_sources,
        configured_keys,
        config_path: path,
    })
}

pub fn save(app: &AppHandle, update: SettingsUpdate) -> Result<SettingsSnapshot> {
    validate_update(&update)?;
    let mut stored = load_stored(app)?;
    stored.window_corner_radius = update.window_corner_radius;
    stored.window_decoration_mode = update.window_decoration_mode;
    stored.startup_view = update.startup_view;
    stored.startup_page_uuid = update.startup_page_uuid;
    stored.sync_server_url = update.sync_server_url;
    stored.ai_search_enabled = update.ai_search_enabled;
    stored.ai_search_trigger = update.ai_search_trigger;
    stored.ai_search_rerank = update.ai_search_rerank;
    stored.search_debug_sources = update.search_debug_sources;
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
    live_snapshot(app, stored, path)
}

pub fn reset(app: &AppHandle) -> Result<SettingsSnapshot> {
    let stored = StoredSettings::default();
    let path = config_path(app)?;
    write_settings(&path, &stored)?;
    apply_window_decorations(app, &stored.window_decoration_mode)?;
    live_snapshot(app, stored, path)
}

#[cfg(not(mobile))]
pub fn create_main_window(
    app: &tauri::App,
    config: &tauri::utils::config::WindowConfig,
) -> Result<()> {
    let mode = match load_stored(app.handle()) {
        Ok(settings) => settings.window_decoration_mode,
        Err(error) => {
            // Keep the startup-error UI available if settings are unreadable.
            tracing::warn!(%error, "using native decorations because settings could not be loaded");
            WindowDecorationMode::Native
        }
    };
    let builder = tauri::WebviewWindowBuilder::from_config(app, config)?
        .decorations(mode == WindowDecorationMode::Native);
    // Transparency is creation-time only; CSS keeps native mode opaque.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let builder = builder.transparent(true);
    let window = builder.build()?;
    #[cfg(target_os = "linux")]
    let requires_restart = {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        matches!(
            window.window_handle()?.as_raw(),
            RawWindowHandle::Wayland(_)
        )
    };
    #[cfg(not(target_os = "linux"))]
    let requires_restart = false;
    let _ = window;
    app.manage(WindowDecorations {
        active: std::sync::Mutex::new(mode),
        requires_restart,
    });
    Ok(())
}

fn apply_window_decorations(app: &AppHandle, mode: &WindowDecorationMode) -> Result<()> {
    #[cfg(not(mobile))]
    {
        let state = app.state::<WindowDecorations>();
        let mut active = state.active.lock().unwrap_or_else(|e| e.into_inner());
        if !should_apply_decorations(state.requires_restart, *active, *mode) {
            return Ok(());
        }
        app.get_webview_window("main")
            .context("main window is unavailable")?
            .set_decorations(*mode != WindowDecorationMode::Borderless)
            .context("applying window decorations")?;
        *active = *mode;
        Ok(())
    }

    #[cfg(mobile)]
    {
        let _ = (app, mode);
        Ok(())
    }
}

fn validate_update(update: &SettingsUpdate) -> Result<()> {
    if update.window_corner_radius > 24 {
        bail!("window corner radius must be between 0 and 24 px");
    }
    if let Some(server_url) = &update.sync_server_url {
        validate_http_url(server_url)?;
    }
    validate_startup(update.startup_view, update.startup_page_uuid)?;
    Ok(())
}

fn validate_stored(stored: &StoredSettings) -> Result<()> {
    if stored.window_corner_radius > 24 {
        bail!("window corner radius must be between 0 and 24 px");
    }
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
    validate_startup(stored.startup_view, stored.startup_page_uuid)?;
    Ok(())
}

fn validate_startup(view: StartupView, page_uuid: Option<uuid::Uuid>) -> Result<()> {
    if view == StartupView::SpecificPage && page_uuid.is_none() {
        bail!("a startup page is required when startup view is specific_page");
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
    #[test]
    fn corner_radius_defaults_round_trips_and_rejects_excessive_values() {
        let mut stored = StoredSettings::default();
        assert_eq!(stored.window_corner_radius, 10);
        for radius in [0, 6, 10, 16, 24] {
            stored.window_corner_radius = radius;
            let restored: StoredSettings =
                serde_json::from_slice(&serde_json::to_vec(&stored).unwrap()).unwrap();
            assert_eq!(restored.window_corner_radius, radius);
            assert!(validate_stored(&restored).is_ok());
        }
        stored.window_corner_radius = 25;
        assert!(validate_stored(&stored).is_err());
    }
    #[test]
    fn decoration_changes_are_deferred_only_on_wayland() {
        use super::{
            WindowDecorationMode::{Borderless, Native},
            should_apply_decorations,
        };
        for active in [Native, Borderless] {
            for requested in [Native, Borderless] {
                assert!(!should_apply_decorations(true, active, requested));
                assert_eq!(
                    should_apply_decorations(false, active, requested),
                    active != requested
                );
            }
        }
    }
    use super::*;

    #[test]
    fn stored_settings_contain_only_device_and_server_connection_state() {
        let value = serde_json::to_value(StoredSettings::default()).expect("serialize settings");
        let object = value.as_object().expect("settings object");
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "aiSearchEnabled",
                "aiSearchRerank",
                "aiSearchTrigger",
                "searchDebugSources",
                "secrets",
                "startupPageUuid",
                "startupView",
                "syncServerUrl",
                "version",
                "windowCornerRadius",
                "windowDecorationMode",
            ]
        );
    }

    #[test]
    fn legacy_settings_default_new_search_preferences() {
        let stored: StoredSettings = serde_json::from_value(serde_json::json!({
            "version": SETTINGS_VERSION,
            "windowDecorationMode": "native",
            "syncServerUrl": null,
            "secrets": {}
        }))
        .expect("read legacy settings");
        assert_eq!(stored.window_corner_radius, 10);
        assert!(stored.ai_search_enabled);
        assert_eq!(stored.ai_search_trigger, AiSearchTrigger::AsYouType);
        assert!(stored.ai_search_rerank);
        assert!(!stored.search_debug_sources);
        assert_eq!(stored.startup_view, StartupView::Dashboard);
        assert_eq!(stored.startup_page_uuid, None);
    }

    #[test]
    fn search_preferences_round_trip_through_storage_and_snapshot() {
        let stored = StoredSettings {
            ai_search_enabled: false,
            ai_search_trigger: AiSearchTrigger::EnterOnly,
            ai_search_rerank: false,
            search_debug_sources: true,
            ..StoredSettings::default()
        };
        let body = serde_json::to_vec(&stored).expect("serialize settings");
        let restored: StoredSettings = serde_json::from_slice(&body).expect("restore settings");
        let snapshot = snapshot(restored, PathBuf::from("settings.json")).expect("snapshot");

        assert!(!snapshot.ai_search_enabled);
        assert_eq!(snapshot.ai_search_trigger, AiSearchTrigger::EnterOnly);
        assert!(!snapshot.ai_search_rerank);
        assert!(snapshot.search_debug_sources);
    }

    #[test]
    fn startup_preferences_round_trip_and_require_a_specific_page() {
        let page_uuid = uuid::Uuid::new_v4();
        let stored = StoredSettings {
            startup_view: StartupView::SpecificPage,
            startup_page_uuid: Some(page_uuid),
            ..StoredSettings::default()
        };
        let body = serde_json::to_vec(&stored).expect("serialize settings");
        let restored: StoredSettings = serde_json::from_slice(&body).expect("restore settings");
        let snapshot = snapshot(restored, PathBuf::from("settings.json")).expect("snapshot");

        assert_eq!(snapshot.startup_view, StartupView::SpecificPage);
        assert_eq!(snapshot.startup_page_uuid, Some(page_uuid));
        assert!(validate_startup(StartupView::SpecificPage, None).is_err());
    }
}

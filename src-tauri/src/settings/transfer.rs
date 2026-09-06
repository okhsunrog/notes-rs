use super::*;
use crate::commands::{CommandError, CommandErrorCode, CommandResult, DomainEvent, emit_domain};
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::State;

const MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationTheme {
    System,
    Light,
    Dark,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationPalette {
    Iris,
    Tidal,
    Ember,
    Sakura,
    Nordic,
    Moss,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationDisplayProfile {
    #[default]
    Auto,
    Standard,
    Eink,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationInkColor {
    #[default]
    Auto,
    Color,
    Mono,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationAppearance {
    pub theme: ConfigurationTheme,
    pub palette: ConfigurationPalette,
    // Files written before the display profile existed stay importable.
    #[serde(default)]
    pub display_profile: ConfigurationDisplayProfile,
    #[serde(default)]
    pub ink_color: ConfigurationInkColor,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationSearch {
    pub enabled: bool,
    pub trigger: AiSearchTrigger,
    pub rerank: bool,
}

// No Debug implementation: portable files can contain credentials.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    format: String,
    version: u32,
    server_url: Option<url::Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync_token: Option<String>,
    search: ConfigurationSearch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    appearance: Option<ConfigurationAppearance>,
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationPreview {
    pub id: uuid::Uuid,
    pub server_url: Option<url::Url>,
    pub token_included: bool,
    pub token_required: bool,
    pub search: ConfigurationSearch,
    pub appearance: Option<ConfigurationAppearance>,
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationImportResult {
    pub settings: SettingsSnapshot,
    pub appearance: Option<ConfigurationAppearance>,
    pub connection_verified: bool,
    pub restart_required: bool,
}

#[derive(Default)]
pub struct PendingConfiguration(Mutex<Option<(uuid::Uuid, Configuration)>>);

fn failure(message: &str) -> CommandError {
    CommandError {
        code: CommandErrorCode::InvalidInput,
        message: message.into(),
    }
}

fn parse_configuration(reader: impl Read) -> Result<Configuration> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        bail!("Configuration files must be smaller than 64 KiB.");
    }
    // serde errors can echo a malicious field name or secret value. Never surface them.
    let config: Configuration = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid configuration JSON or unsupported fields."))?;
    if config.format != "tangleaf-configuration" || config.version != 1 {
        bail!("Unsupported configuration format or version.");
    }
    if let Some(url) = &config.server_url {
        validate_http_url(url)?;
    }
    if let Some(token) = &config.sync_token
        && (token.trim().is_empty()
            || token.chars().any(char::is_control)
            || config.server_url.is_none())
    {
        bail!("A non-empty token requires a server URL and cannot contain control characters.");
    }
    Ok(config)
}

fn export_configuration(
    stored: &StoredSettings,
    include_token: bool,
    appearance: Option<ConfigurationAppearance>,
) -> Configuration {
    Configuration {
        format: "tangleaf-configuration".into(),
        version: 1,
        server_url: stored.sync_server_url.clone(),
        sync_token: include_token
            .then(|| stored.secrets.get(&SecretKey::SyncToken).cloned())
            .flatten()
            .filter(|_| stored.sync_server_url.is_some()),
        search: ConfigurationSearch {
            enabled: stored.ai_search_enabled,
            trigger: stored.ai_search_trigger,
            rerank: stored.ai_search_rerank,
        },
        appearance,
    }
}

fn merge_configuration(
    mut stored: StoredSettings,
    config: &Configuration,
    token: Option<String>,
) -> Result<StoredSettings> {
    // Never send a token saved for another server to the imported destination.
    if stored.sync_server_url != config.server_url || config.server_url.is_none() {
        stored.secrets.remove(&SecretKey::SyncToken);
    }
    stored.sync_server_url = config.server_url.clone();
    if let Some(token) = config.sync_token.clone().or(token) {
        let token = token.trim();
        if token.is_empty() || token.chars().any(char::is_control) || config.server_url.is_none() {
            bail!("Enter a valid device token for this server.");
        }
        stored.secrets.insert(SecretKey::SyncToken, token.into());
    }
    stored.ai_search_enabled = config.search.enabled;
    stored.ai_search_trigger = config.search.trigger;
    stored.ai_search_rerank = config.search.rerank;
    validate_stored(&stored)?;
    Ok(stored)
}

#[tauri::command]
#[specta::specta]
pub async fn export_device_configuration(
    app: AppHandle,
    include_token: bool,
    appearance: Option<ConfigurationAppearance>,
) -> CommandResult<bool> {
    let stored = load_stored(&app).map_err(|_| failure("Could not read saved device settings."))?;
    let body = serde_json::to_vec_pretty(&export_configuration(&stored, include_token, appearance))
        .map_err(|_| failure("Could not serialize configuration."))?;
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::DialogExt;
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("Tangleaf configuration", &["json"])
            .set_file_name("tangleaf-configuration.json")
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = path
            .into_path()
            .map_err(|_| failure("Choose a local configuration file."))?;
        let result = (|| -> Result<()> {
            let mut file = tempfile::NamedTempFile::new_in(
                path.parent().context("missing parent directory")?,
            )?;
            file.write_all(&body)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        })();
        result.map_err(|_| failure("Could not save configuration file."))?;
    }
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_android_fs::AndroidFsExt;
        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .save_file(
                None,
                "tangleaf-configuration.json",
                Some("application/json"),
                false,
            )
            .await
            .map_err(|_| failure("Could not open the document picker."))?
        else {
            return Ok(false);
        };
        let result = async {
            let mut file = api.open_file_writable(&uri).await.map_err(|_| ())?;
            file.write_all(&body).map_err(|_| ())?;
            file.flush().map_err(|_| ())
        }
        .await;
        if result.is_err() {
            let _ = api.remove_file(&uri).await;
            return Err(failure("Could not save configuration file."));
        }
    }
    Ok(true)
}

#[tauri::command]
#[specta::specta]
pub async fn preview_configuration_import(
    app: AppHandle,
    pending: State<'_, PendingConfiguration>,
) -> CommandResult<Option<ConfigurationPreview>> {
    *pending.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    #[cfg(not(target_os = "android"))]
    let file = {
        use tauri_plugin_dialog::DialogExt;
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("Tangleaf configuration", &["json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path
            .into_path()
            .map_err(|_| failure("Choose a local configuration file."))?;
        std::fs::File::open(path).map_err(|_| failure("Could not read configuration file."))?
    };
    #[cfg(target_os = "android")]
    let file = {
        use tauri_plugin_android_fs::AndroidFsExt;
        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .pick_file(
                None,
                &["application/json", "text/plain", "application/octet-stream"],
                false,
            )
            .await
            .map_err(|_| failure("Could not open the document picker."))?
        else {
            return Ok(None);
        };
        api.open_file_readable(&uri)
            .await
            .map_err(|_| failure("Could not read configuration file."))?
    };
    let config = parse_configuration(file).map_err(|e| failure(&e.to_string()))?;
    let stored = load_stored(&app).map_err(|_| failure("Could not read saved device settings."))?;
    let id = uuid::Uuid::now_v7();
    let preview = ConfigurationPreview {
        id,
        server_url: config.server_url.clone(),
        token_included: config.sync_token.is_some(),
        token_required: merge_configuration(stored, &config, None).is_err(),
        search: config.search.clone(),
        appearance: config.appearance.clone(),
    };
    *pending.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, config));
    Ok(Some(preview))
}

#[tauri::command]
#[specta::specta]
pub fn cancel_configuration_import(pending: State<'_, PendingConfiguration>, id: uuid::Uuid) {
    let mut guard = pending.0.lock().unwrap_or_else(|e| e.into_inner());
    if guard
        .as_ref()
        .is_some_and(|(pending_id, _)| *pending_id == id)
    {
        *guard = None;
    }
}

#[tauri::command]
#[specta::specta]
pub async fn apply_configuration_import(
    app: AppHandle,
    pending: State<'_, PendingConfiguration>,
    sync: State<'_, crate::sync::SyncRuntime>,
    id: uuid::Uuid,
    token: Option<String>,
) -> CommandResult<ConfigurationImportResult> {
    let config = {
        let guard = pending.0.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .filter(|(pending_id, _)| *pending_id == id)
            .map(|(_, config)| config.clone())
            .ok_or_else(|| failure("This import preview has expired. Choose the file again."))?
    };
    let previous =
        load_stored(&app).map_err(|_| failure("Could not read saved device settings."))?;
    let stored = merge_configuration(previous.clone(), &config, token)
        .map_err(|e| failure(&e.to_string()))?;
    let restart_required =
        previous.sync_server_url != stored.sync_server_url || previous.secrets != stored.secrets;
    write_settings(
        &config_path(&app).map_err(|_| failure("Could not locate settings."))?,
        &stored,
    )
    .map_err(|_| failure("Could not save imported settings."))?;
    cancel_configuration_import(pending, id);
    if restart_required {
        sync.sync_settings_changed();
    }
    emit_domain(&app, DomainEvent::SettingsChanged);
    let connection_verified = if let Some(url) = stored.sync_server_url.clone() {
        match notes_sync::HttpTransport::new(url, stored.secrets[&SecretKey::SyncToken].clone()) {
            Ok(transport) => transport.info().await.is_ok(),
            Err(_) => false,
        }
    } else {
        false
    };
    Ok(ConfigurationImportResult {
        settings: live_snapshot(
            &app,
            stored,
            config_path(&app).map_err(|_| failure("Could not locate settings."))?,
        )
        .map_err(|_| failure("Could not reload imported settings."))?,
        appearance: config.appearance,
        connection_verified,
        restart_required,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored() -> StoredSettings {
        let mut stored = StoredSettings {
            sync_server_url: Some("https://notes.example/".parse().unwrap()),
            ..StoredSettings::default()
        };
        stored
            .secrets
            .insert(SecretKey::SyncToken, "private-token".into());
        stored.window_corner_radius = 17;
        stored
    }

    #[test]
    fn exports_secrets_only_when_requested_and_round_trips() {
        let config = export_configuration(&stored(), false, None);
        let body = serde_json::to_vec(&config).unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("private-token"));
        assert!(!String::from_utf8_lossy(&body).contains("syncToken"));
        let config = export_configuration(&stored(), true, None);
        let body = serde_json::to_vec(&config).unwrap();
        let parsed = parse_configuration(body.as_slice()).unwrap();
        let merged = merge_configuration(StoredSettings::default(), &parsed, None).unwrap();
        assert_eq!(merged.secrets[&SecretKey::SyncToken], "private-token");
        assert_eq!(merged.window_corner_radius, 10);
    }

    #[test]
    fn preserves_local_settings_and_never_reuses_tokens_across_servers() {
        let mut config = export_configuration(&stored(), false, None);
        assert_eq!(
            merge_configuration(stored(), &config, None)
                .unwrap()
                .window_corner_radius,
            17
        );
        config.server_url = Some("https://other.example/".parse().unwrap());
        assert!(merge_configuration(stored(), &config, None).is_err());
        let merged = merge_configuration(stored(), &config, Some("new-token".into())).unwrap();
        assert_eq!(merged.secrets[&SecretKey::SyncToken], "new-token");
        config.server_url = None;
        assert!(
            merge_configuration(stored(), &config, None)
                .unwrap()
                .secrets
                .is_empty()
        );
    }

    #[test]
    fn appearance_written_before_the_display_profile_still_imports() {
        let mut body = serde_json::to_value(export_configuration(&stored(), false, None)).unwrap();
        body["appearance"] = serde_json::json!({"theme": "dark", "palette": "iris"});
        let bytes = serde_json::to_vec(&body).unwrap();
        let appearance = parse_configuration(bytes.as_slice())
            .unwrap()
            .appearance
            .expect("appearance");
        assert!(matches!(
            appearance.display_profile,
            ConfigurationDisplayProfile::Auto
        ));
        assert!(matches!(appearance.ink_color, ConfigurationInkColor::Auto));
    }

    #[test]
    fn rejects_invalid_files_without_echoing_secret_values() {
        let body = serde_json::to_value(export_configuration(&stored(), true, None)).unwrap();
        for (field, value) in [
            ("version", serde_json::json!(99)),
            (
                "serverUrl",
                serde_json::json!("https://private-token@notes.example/"),
            ),
            ("syncToken", serde_json::json!("")),
            (
                "appearance",
                serde_json::json!({"theme":"private-token", "palette":"iris"}),
            ),
            ("private-token", serde_json::json!(true)),
        ] {
            let mut malformed = body.clone();
            malformed[field] = value;
            let bytes = serde_json::to_vec(&malformed).unwrap();
            let error = parse_configuration(bytes.as_slice())
                .err()
                .expect("invalid configuration");
            assert!(!error.to_string().contains("private-token"));
        }
        assert!(parse_configuration(&vec![b' '; MAX_BYTES as usize + 1][..]).is_err());
    }
}

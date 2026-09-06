use super::*;

#[tauri::command]
#[specta::specta]
pub fn is_ready(state: State<'_, Startup>) -> bool {
    matches!(
        *state.status.read().unwrap_or_else(|e| e.into_inner()),
        StartupStatus::Ready
    )
}

#[tauri::command]
#[specta::specta]
pub fn startup_status(state: State<'_, Startup>) -> StartupStatus {
    state
        .status
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[tauri::command]
#[specta::specta]
pub fn sync_status(state: State<'_, crate::sync::SyncRuntime>) -> crate::sync::SyncStatus {
    state
        .status
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn remote_ai(state: &AppState) -> CommandResult<&notes_sync::HttpTransport> {
    state.remote_ai.as_ref().ok_or_else(|| CommandError {
        code: CommandErrorCode::Unavailable,
        message: "server AI requires a configured tangleaf server".into(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn server_ai_status(state: State<'_, AppState>) -> CommandResult<AiIndexStatus> {
    remote_ai(&state)?.ai_status().await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn save_server_ai_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AiRuntimeSettings,
) -> CommandResult<AiIndexStatus> {
    let status = remote_ai(&state)?
        .update_ai_settings(settings)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::ServerAiChanged);
    Ok(status)
}

#[tauri::command]
#[specta::specta]
pub async fn save_server_ai_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AiProviderSettingsUpdate,
) -> CommandResult<AiIndexStatus> {
    let status = remote_ai(&state)?
        .update_ai_provider(settings)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::ServerAiChanged);
    Ok(status)
}

#[tauri::command]
#[specta::specta]
pub async fn probe_server_ai_provider(
    state: State<'_, AppState>,
    settings: AiProviderSettingsUpdate,
) -> CommandResult<AiProviderProbeResult> {
    remote_ai(&state)?
        .probe_ai_provider(settings)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn reindex_server_ai(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<AiIndexStatus> {
    let status = remote_ai(&state)?.reindex_ai().await.map_err(err)?;
    emit_domain(&app, DomainEvent::ServerAiChanged);
    Ok(status)
}

#[tauri::command]
#[specta::specta]
pub fn load_settings(app: AppHandle) -> CommandResult<crate::settings::SettingsSnapshot> {
    crate::settings::load(&app).map_err(err)
}

#[tauri::command]
#[specta::specta]
pub fn save_settings(
    app: AppHandle,
    sync: State<'_, crate::sync::SyncRuntime>,
    update: crate::settings::SettingsUpdate,
) -> CommandResult<crate::settings::SettingsSnapshot> {
    let previous_sync = crate::settings::runtime(&app)
        .and_then(|settings| settings.sync_credentials())
        .map_err(err)?;
    let settings = crate::settings::save(&app, update).map_err(err)?;
    let current_sync = crate::settings::runtime(&app)
        .and_then(|settings| settings.sync_credentials())
        .map_err(err)?;
    if previous_sync != current_sync {
        sync.sync_settings_changed();
    }
    emit_domain(&app, DomainEvent::SettingsChanged);
    Ok(settings)
}

#[tauri::command]
#[specta::specta]
pub fn reset_settings(
    app: AppHandle,
    sync: State<'_, crate::sync::SyncRuntime>,
) -> CommandResult<crate::settings::SettingsSnapshot> {
    let previous_sync = crate::settings::runtime(&app)
        .and_then(|settings| settings.sync_credentials())
        .ok()
        .flatten();
    let settings = crate::settings::reset(&app).map_err(err)?;
    if previous_sync.is_some() {
        sync.sync_settings_changed();
    }
    emit_domain(&app, DomainEvent::SettingsChanged);
    Ok(settings)
}

#[tauri::command]
#[specta::specta]
pub fn retry_sync(sync: State<'_, crate::sync::SyncRuntime>) {
    sync.request_retry();
}

/// Puts every change the server refused back in the send queue and reconnects.
/// Nothing was deleted while it was held back, so this is a plain retry.
#[tauri::command]
#[specta::specta]
pub async fn retry_rejected_changes(
    state: State<'_, AppState>,
    sync: State<'_, crate::sync::SyncRuntime>,
) -> CommandResult<u32> {
    let released = notes_core::release_quarantined_outbox(&state.conn)
        .await
        .map_err(err)?;
    sync.request_retry();
    Ok(released.try_into().unwrap_or(u32::MAX))
}

#[tauri::command]
#[specta::specta]
pub fn restart_app(app: AppHandle) {
    app.restart()
}

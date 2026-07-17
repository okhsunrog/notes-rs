use super::*;

#[tauri::command]
#[specta::specta]
pub async fn export_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    let filename = format!(
        "notes-rs-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let mut archive = db::export_archive(&state.conn).await.map_err(err)?;
    add_archive_files(&app, &mut archive).map_err(err)?;
    let json = serde_json::to_string_pretty(&archive).map_err(err)?;

    #[cfg(not(target_os = "android"))]
    {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("notes-rs archive", &["json"])
            .set_file_name(filename)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        std::fs::write(&path, json).map_err(err)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    }

    #[cfg(target_os = "android")]
    {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .save_file(None, filename, Some("application/json"), false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        api.write(&uri, json.as_bytes()).await.map_err(err)?;
        Ok(Some(uri.uri))
    }
}

#[tauri::command]
#[specta::specta]
pub async fn import_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    #[cfg(not(target_os = "android"))]
    let (source, json) = {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("notes-rs archive", &["json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        let json = std::fs::read_to_string(&path).map_err(err)?;
        (path.to_string_lossy().into_owned(), json)
    };

    #[cfg(target_os = "android")]
    let (source, json) = {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .pick_file(None, &["application/json"], false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        let json = api.read_to_string(&uri).await.map_err(err)?;
        (uri.uri, json)
    };

    let archive: db::DataArchive = serde_json::from_str(&json).map_err(err)?;
    write_backup(&app, &state.conn, "before-import")
        .await
        .map_err(err)?;
    restore_archive(&app, &state.conn, archive)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::WorkspaceChanged);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(Some(source))
}

#[tauri::command]
#[specta::specta]
pub async fn create_backup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    write_backup(&app, &state.conn, "manual").await.map_err(err)
}

pub(super) async fn write_backup(
    app: &AppHandle,
    connection: &Connection,
    reason: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let directory = app.path().app_data_dir()?.join("backups");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!(
        "notes-rs-{reason}-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let mut archive = db::export_archive(connection).await?;
    add_archive_files(app, &mut archive)?;
    std::fs::write(&path, serde_json::to_string_pretty(&archive)?)?;
    Ok(path)
}

#[tauri::command]
#[specta::specta]
pub fn choose_sync_directory(app: AppHandle) -> CommandResult<Option<std::path::PathBuf>> {
    #[cfg(not(mobile))]
    {
        app.dialog()
            .file()
            .blocking_pick_folder()
            .map(|path| path.into_path())
            .transpose()
            .map_err(err)
    }

    #[cfg(mobile)]
    {
        let _ = app;
        Err(
            "directory-based sync is unavailable on mobile; configure the sync server instead"
                .into(),
        )
    }
}

#[tauri::command]
#[specta::specta]
pub async fn sync_push(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let directory = sync_directory(settings.sync_directory.as_deref()).map_err(err)?;
    std::fs::create_dir_all(&directory).map_err(err)?;
    let mut archive = db::export_archive(&state.conn).await.map_err(err)?;
    add_archive_files(&app, &mut archive).map_err(err)?;
    let path = directory.join("notes-rs-sync.json");
    let temporary = directory.join("notes-rs-sync.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_string_pretty(&archive).map_err(err)?,
    )
    .map_err(err)?;
    std::fs::rename(&temporary, &path).map_err(err)?;
    Ok(path)
}

#[tauri::command]
#[specta::specta]
pub async fn sync_pull(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let path = sync_directory(settings.sync_directory.as_deref())
        .map_err(err)?
        .join("notes-rs-sync.json");
    let archive =
        serde_json::from_str(&std::fs::read_to_string(&path).map_err(err)?).map_err(err)?;
    write_backup(&app, &state.conn, "before-sync-pull")
        .await
        .map_err(err)?;
    restore_archive(&app, &state.conn, archive)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::WorkspaceChanged);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(path)
}

fn sync_directory(value: Option<&std::path::Path>) -> anyhow::Result<std::path::PathBuf> {
    value
        .map(std::path::Path::to_path_buf)
        .context("choose a sync directory in Settings first")
}

fn add_archive_files(app: &AppHandle, archive: &mut db::DataArchive) -> anyhow::Result<()> {
    for node in archive
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Attachment)
    {
        let path = safe_app_data_path(app, &node.content)?;
        let bytes = std::fs::read(&path)
            .with_context(|| format!("reading attachment {}", path.display()))?;
        archive.files.insert(
            node.content.clone(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
    }
    Ok(())
}

async fn restore_archive(
    app: &AppHandle,
    connection: &Connection,
    archive: db::DataArchive,
) -> anyhow::Result<()> {
    let data_dir = app.path().app_data_dir()?;
    let staging = data_dir.join(format!("attachments-import-{}", uuid::Uuid::new_v4()));
    for (relative, encoded) in &archive.files {
        let relative = safe_relative_path(relative)?;
        let destination = staging.join(relative.strip_prefix("attachments").unwrap_or(&relative));
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            destination,
            base64::engine::general_purpose::STANDARD.decode(encoded)?,
        )?;
    }
    if let Err(error) = db::import_archive(connection, archive).await {
        let _ = std::fs::remove_dir_all(staging);
        return Err(error);
    }
    let attachments = data_dir.join("attachments");
    if attachments.exists() {
        std::fs::remove_dir_all(&attachments)?;
    }
    if staging.exists() {
        std::fs::rename(staging, attachments)?;
    }
    Ok(())
}

pub(super) fn safe_app_data_path(
    app: &AppHandle,
    relative: &str,
) -> anyhow::Result<std::path::PathBuf> {
    Ok(app
        .path()
        .app_data_dir()?
        .join(safe_relative_path(relative)?))
}

fn safe_relative_path(value: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = std::path::Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("archive contains an unsafe attachment path");
    }
    if !path.starts_with("attachments") {
        anyhow::bail!("attachment paths must be below the attachments directory");
    }
    Ok(path.to_path_buf())
}

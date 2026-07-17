use super::*;

const MAX_ATTACHMENT_SIZE: u64 = 100 * 1024 * 1024;

#[tauri::command]
#[specta::specta]
pub async fn attach_file(
    app: AppHandle,
    state: State<'_, AppState>,
    location: notes_core::AttachmentOwner,
) -> CommandResult<Option<db::Attachment>> {
    #[cfg(not(target_os = "android"))]
    let (filename, mime, bytes) = {
        let Some(source) = app.dialog().file().blocking_pick_file() else {
            return Ok(None);
        };
        let source = source.into_path().map_err(err)?;
        let metadata = source.metadata().map_err(err)?;
        if !metadata.is_file() {
            return Err(CommandError::invalid("attachments must be regular files"));
        }
        if metadata.len() > MAX_ATTACHMENT_SIZE {
            return Err(CommandError::invalid("attachments are limited to 100 MiB"));
        }
        let filename = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CommandError::invalid("attachment filename is not valid UTF-8"))?
            .to_owned();
        let bytes = std::fs::read(&source).map_err(err)?;
        (filename, "application/octet-stream".to_owned(), bytes)
    };

    #[cfg(target_os = "android")]
    let (filename, mime, bytes) = {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .pick_file(None, &[], false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        let size = api.get_len(&uri).await.map_err(err)?;
        if size > MAX_ATTACHMENT_SIZE {
            return Err(CommandError::invalid("attachments are limited to 100 MiB"));
        }
        let filename = api.get_name(&uri).await.map_err(err)?;
        let mime = api
            .get_mime_type(&uri)
            .await
            .unwrap_or_else(|_| "application/octet-stream".into());
        let bytes = api.read(&uri).await.map_err(err)?;
        (filename, mime, bytes)
    };

    validate_attachment_filename(&filename)?;
    let size = bytes.len() as u64;
    let blob_hash = format!("{:x}", Sha256::digest(&bytes));
    let relative = attachment_relative_path(&blob_hash, &filename);
    let destination = app.path().app_data_dir().map_err(err)?.join(&relative);
    let file_already_existed = destination.exists();
    std::fs::create_dir_all(destination.parent().expect("attachment has parent")).map_err(err)?;
    std::fs::write(&destination, bytes).map_err(err)?;

    match db::create_attachment(&state.conn, location, blob_hash, filename, mime, size).await {
        Ok(attachment) => {
            emit_domain(
                &app,
                DomainEvent::AttachmentsChanged {
                    owner_uuids: vec![attachment.owner.uuid()],
                },
            );
            Ok(Some(attachment))
        }
        Err(error) => {
            if !file_already_existed {
                let _ = std::fs::remove_file(destination);
            }
            Err(err(error))
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_attachments(
    state: State<'_, AppState>,
    location: notes_core::AttachmentOwner,
) -> CommandResult<Vec<db::Attachment>> {
    db::list_attachments(&state.conn, location)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<()> {
    let attachment = db::get_attachment(&state.conn, uuid)
        .await
        .map_err(err)?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "attachment not found"))?;
    let relative = attachment_relative_path(&attachment.blob_hash, &attachment.filename);
    let path = super::data::safe_app_data_path(&app, &relative).map_err(err)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<bool> {
    super::data::write_backup(&app, &state.conn, "before-attachment-delete")
        .await
        .map_err(err)?;
    let Some(attachment) = db::delete_attachment(&state.conn, uuid)
        .await
        .map_err(err)?
    else {
        return Ok(false);
    };
    remove_file_if_unreferenced(&app, &state.conn, &attachment)
        .await
        .map_err(err)?;
    emit_domain(
        &app,
        DomainEvent::AttachmentsChanged {
            owner_uuids: vec![attachment.owner.uuid()],
        },
    );
    Ok(true)
}

pub(super) async fn remove_file_if_unreferenced(
    app: &AppHandle,
    connection: &Connection,
    attachment: &db::Attachment,
) -> anyhow::Result<()> {
    let relative = attachment_relative_path(&attachment.blob_hash, &attachment.filename);
    let path = super::data::safe_app_data_path(app, &relative)?;
    let still_referenced = db::attachment_path_ref_count(
        connection,
        attachment.blob_hash.clone(),
        attachment.filename.clone(),
    )
    .await?
        > 0;
    if !still_referenced && path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub(super) fn attachment_relative_path(blob_hash: &str, filename: &str) -> std::path::PathBuf {
    std::path::PathBuf::from("attachments")
        .join(blob_hash)
        .join(filename)
}

fn validate_attachment_filename(filename: &str) -> CommandResult<()> {
    notes_core::validate_attachment_filename(filename).map_err(err)
}

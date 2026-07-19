use super::*;
use notes_blob::BlobHash;
use std::io::Read;

pub(super) const MAX_ATTACHMENT_SIZE: u64 = 100 * 1024 * 1024;
const HASH_BUFFER_SIZE: usize = 64 * 1024;

#[tauri::command]
#[specta::specta]
pub async fn attach_file(
    app: AppHandle,
    state: State<'_, AppState>,
    location: notes_core::AttachmentOwner,
) -> CommandResult<Option<db::Attachment>> {
    #[cfg(not(target_os = "android"))]
    let (filename, mime, installed) = {
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
        validate_attachment_filename(&filename)?;
        let source_file = std::fs::File::open(&source).map_err(err)?;
        let (blob_hash, _) = hash_reader_limited(source_file, MAX_ATTACHMENT_SIZE).map_err(err)?;
        let installed = state
            .blob_store
            .install_file(&source, blob_hash, MAX_ATTACHMENT_SIZE)
            .map_err(err)?;
        (filename, "application/octet-stream".to_owned(), installed)
    };

    #[cfg(target_os = "android")]
    let (filename, mime, installed) = {
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
        validate_attachment_filename(&filename)?;
        let source = api.open_file_readable(&uri).await.map_err(err)?;
        let (blob_hash, _) = hash_reader_limited(source, MAX_ATTACHMENT_SIZE).map_err(err)?;
        let source = api.open_file_readable(&uri).await.map_err(err)?;
        let installed = state
            .blob_store
            .install_reader(source, blob_hash, MAX_ATTACHMENT_SIZE)
            .map_err(err)?;
        (filename, mime, installed)
    };

    let applied = db::create_attachment_with_ops(
        &state.conn,
        location,
        installed.blob.hash,
        filename,
        mime,
        installed.blob.size,
    )
    .await
    .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations).await;
    Ok(Some(applied.value))
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
pub async fn resolve_attachment_images(
    state: State<'_, AppState>,
    attachment_uuids: Vec<uuid::Uuid>,
) -> CommandResult<Vec<crate::attachment_protocol::AttachmentImageDescriptor>> {
    if attachment_uuids.len() > crate::attachment_protocol::MAX_DESCRIPTOR_BATCH {
        return Err(CommandError::invalid(format!(
            "at most {} attachment images can be resolved at once",
            crate::attachment_protocol::MAX_DESCRIPTOR_BATCH
        )));
    }
    crate::attachment_protocol::resolve_descriptors(&state, attachment_uuids)
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
    let verified = state
        .blob_store
        .open_verified(attachment.blob_hash, MAX_ATTACHMENT_SIZE)
        .map_err(err)?;
    if verified.blob.size != attachment.size {
        return Err(CommandError::new(
            CommandErrorCode::Conflict,
            format!(
                "attachment size does not match its stored blob: expected {}, got {}",
                attachment.size, verified.blob.size
            ),
        ));
    }
    app.opener()
        .open_path(verified.blob.path.to_string_lossy(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<bool> {
    super::data::write_backup(
        &app,
        &state.conn,
        &state.blob_store,
        "before-attachment-delete",
    )
    .await
    .map_err(err)?;
    let applied = db::delete_attachment_with_ops(&state.conn, uuid)
        .await
        .map_err(err)?;
    let Some(_attachment) = applied.value else {
        return Ok(false);
    };
    emit_events_for_ops(&app, &state.conn, &applied.operations).await;
    Ok(true)
}

fn validate_attachment_filename(filename: &str) -> CommandResult<()> {
    notes_core::validate_attachment_filename(filename).map_err(err)
}

fn hash_reader_limited(mut reader: impl Read, maximum: u64) -> anyhow::Result<(BlobHash, u64)> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; HASH_BUFFER_SIZE];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .context("attachment size overflow")?;
        anyhow::ensure!(size <= maximum, "attachments are limited to 100 MiB");
        hasher.update(&buffer[..count]);
    }
    Ok((BlobHash::from_bytes(hasher.finalize().into()), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_hash_enforces_limit() {
        let payload = vec![7_u8; 65];
        let (hash, size) = hash_reader_limited(payload.as_slice(), 65).unwrap();
        assert_eq!(hash, BlobHash::digest(&payload));
        assert_eq!(size, 65);
        assert!(hash_reader_limited(payload.as_slice(), 64).is_err());
    }
}

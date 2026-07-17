use super::*;
use notes_blob::{BlobHash, BlobStore};
use std::collections::BTreeMap;
use std::io::Read;

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
    add_archive_files(&state.blob_store, &mut archive).map_err(err)?;
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
    write_backup(&app, &state.conn, &state.blob_store, "before-import")
        .await
        .map_err(err)?;
    restore_archive(&state.blob_store, &state.conn, archive)
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
    write_backup(&app, &state.conn, &state.blob_store, "manual")
        .await
        .map_err(err)
}

pub(super) async fn write_backup(
    app: &AppHandle,
    connection: &Connection,
    blob_store: &BlobStore,
    reason: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let directory = app.path().app_data_dir()?.join("backups");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!(
        "notes-rs-{reason}-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let mut archive = db::export_archive(connection).await?;
    add_archive_files(blob_store, &mut archive)?;
    std::fs::write(&path, serde_json::to_string_pretty(&archive)?)?;
    Ok(path)
}

fn add_archive_files(blob_store: &BlobStore, archive: &mut db::DataArchive) -> anyhow::Result<()> {
    let expected = archive_blob_sizes(archive)?;
    archive.files.clear();
    for (blob_hash, expected_size) in expected {
        let verified =
            blob_store.open_verified(blob_hash, super::attachments::MAX_ATTACHMENT_SIZE)?;
        anyhow::ensure!(
            verified.blob.size == expected_size,
            "attachment blob {blob_hash} has size {}, expected {expected_size}",
            verified.blob.size
        );
        let mut bytes = Vec::with_capacity(usize::try_from(verified.blob.size)?);
        verified.into_file().read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 == expected_size && BlobHash::digest(&bytes) == blob_hash,
            "attachment blob {blob_hash} changed while it was being exported"
        );
        archive.files.insert(
            archive_blob_path(blob_hash),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
    }
    Ok(())
}

fn archive_blob_sizes(archive: &db::DataArchive) -> anyhow::Result<BTreeMap<BlobHash, u64>> {
    let mut expected = BTreeMap::new();
    for attachment in &archive.attachments {
        if let Some(existing) = expected.insert(attachment.blob_hash, attachment.size) {
            anyhow::ensure!(
                existing == attachment.size,
                "attachment metadata disagrees about the size of blob {}",
                attachment.blob_hash
            );
        }
    }
    Ok(expected)
}

async fn restore_archive(
    blob_store: &BlobStore,
    connection: &Connection,
    archive: db::DataArchive,
) -> anyhow::Result<()> {
    let expected = archive_blob_sizes(&archive)?;
    let expected_files = expected
        .keys()
        .copied()
        .map(archive_blob_path)
        .collect::<std::collections::BTreeSet<_>>();
    let actual_files = archive
        .files
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    anyhow::ensure!(
        actual_files == expected_files,
        "archive attachment metadata and file payloads do not match"
    );
    for (blob_hash, expected_size) in expected {
        let path = archive_blob_path(blob_hash);
        let encoded = archive
            .files
            .get(&path)
            .with_context(|| format!("archive is missing blob {blob_hash}"))?;
        let decoder = base64::read::DecoderReader::new(
            encoded.as_bytes(),
            &base64::engine::general_purpose::STANDARD,
        );
        let installed = blob_store.install_reader(
            decoder,
            blob_hash,
            super::attachments::MAX_ATTACHMENT_SIZE,
        )?;
        anyhow::ensure!(
            installed.blob.size == expected_size,
            "archive blob {blob_hash} has size {}, expected {expected_size}",
            installed.blob.size
        );
    }
    db::import_archive(connection, archive).await?;
    Ok(())
}

fn archive_blob_path(blob_hash: BlobHash) -> String {
    let encoded = blob_hash.to_string();
    format!("blobs/{}/{}", &encoded[..2], encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_blob_paths_are_canonical_and_filename_free() {
        let hash = BlobHash::digest(b"portable payload");
        let encoded = hash.to_string();
        assert_eq!(
            archive_blob_path(hash),
            format!("blobs/{}/{}", &encoded[..2], encoded)
        );
    }

    #[test]
    fn archive_stores_one_payload_per_hash() {
        let temporary = tempfile::tempdir().unwrap();
        let blob_store = BlobStore::new(temporary.path());
        let payload = b"shared attachment bytes";
        let hash = BlobHash::digest(payload);
        blob_store
            .install_reader(payload.as_slice(), hash, payload.len() as u64)
            .unwrap();
        let owner = notes_core::AttachmentOwner::Page(uuid::Uuid::now_v7());
        let attachment = |filename: &str| db::Attachment {
            uuid: uuid::Uuid::now_v7(),
            owner,
            blob_hash: hash,
            filename: filename.into(),
            mime: "application/octet-stream".into(),
            size: payload.len() as u64,
            created_at: 0,
        };
        let mut archive = db::DataArchive {
            format: "notes-rs".into(),
            version: 1,
            workspace_uuid: uuid::Uuid::now_v7(),
            exported_at: 0,
            page_identities: Vec::new(),
            page_aliases: Vec::new(),
            pages: Vec::new(),
            blocks: Vec::new(),
            attachments: vec![attachment("one.bin"), attachment("two.bin")],
            external_import_receipts: Vec::new(),
            files: BTreeMap::new(),
        };

        add_archive_files(&blob_store, &mut archive).unwrap();

        assert_eq!(archive.files.len(), 1);
        assert!(archive.files.contains_key(&archive_blob_path(hash)));
    }
}

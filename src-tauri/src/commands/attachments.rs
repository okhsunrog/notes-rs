use super::*;

#[tauri::command]
#[specta::specta]
pub async fn attach_file(
    app: AppHandle,
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Option<Node>> {
    #[cfg(not(target_os = "android"))]
    let (title, mime_type, bytes) = {
        let Some(source) = app.dialog().file().blocking_pick_file() else {
            return Ok(None);
        };
        let source = source.into_path().map_err(err)?;
        let metadata = source.metadata().map_err(err)?;
        if !metadata.is_file() {
            return Err(CommandError::invalid("attachments must be regular files"));
        }
        if metadata.len() > 100 * 1024 * 1024 {
            return Err(CommandError::invalid("attachments are limited to 100 MiB"));
        }
        let title = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CommandError::invalid("attachment filename is not valid UTF-8"))?
            .to_string();
        let bytes = std::fs::read(&source).map_err(err)?;
        (title, "application/octet-stream".to_string(), bytes)
    };

    #[cfg(target_os = "android")]
    let (title, mime_type, bytes) = {
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
        if size > 100 * 1024 * 1024 {
            return Err(CommandError::invalid("attachments are limited to 100 MiB"));
        }
        let title = api.get_name(&uri).await.map_err(err)?;
        let mime_type = api
            .get_mime_type(&uri)
            .await
            .unwrap_or_else(|_| "application/octet-stream".into());
        let bytes = api.read(&uri).await.map_err(err)?;
        (title, mime_type, bytes)
    };

    let size = bytes.len() as u64;
    let blob_hash = format!("{:x}", Sha256::digest(&bytes));
    let relative = std::path::PathBuf::from("attachments")
        .join(&blob_hash)
        .join(&title);
    let destination = app.path().app_data_dir().map_err(err)?.join(&relative);
    std::fs::create_dir_all(destination.parent().expect("attachment has parent")).map_err(err)?;
    std::fs::write(&destination, bytes).map_err(err)?;
    match db::create_attachment(&state.conn, parent_id, blob_hash, title, mime_type, size).await {
        Ok(node) => {
            emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), [parent_id]).await;
            emit_domain(
                &app,
                DomainEvent::AttachmentsChanged {
                    parent_uuids: node_uuids_for_ids(&state.conn, [parent_id]).await,
                },
            );
            emit_domain(
                &app,
                DomainEvent::GraphChanged {
                    node_uuids: node_uuids_for_ids(&state.conn, [parent_id]).await,
                },
            );
            Ok(Some(node))
        }
        Err(error) => {
            let _ = std::fs::remove_file(destination);
            Err(err(error))
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_attachments(
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Vec<Node>> {
    db::list_attachments(&state.conn, parent_id)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<()> {
    let node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .filter(|node| node.kind == NodeKind::Attachment)
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "attachment not found"))?;
    let path = super::data::safe_app_data_path(&app, &node.content).map_err(err)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<bool> {
    super::data::write_backup(&app, &state.conn, "before-attachment-delete")
        .await
        .map_err(err)?;
    let Some((node, parent_uuid)) = db::delete_attachment(&state.conn, id).await.map_err(err)?
    else {
        return Ok(false);
    };
    let path = super::data::safe_app_data_path(&app, &node.content).map_err(err)?;
    let still_referenced = db::attachment_path_ref_count(&state.conn, node.content)
        .await
        .map_err(err)?
        > 0;
    if !still_referenced && path.exists() {
        std::fs::remove_file(&path).map_err(err)?;
    }
    emit_domain(
        &app,
        DomainEvent::NodeDeleted {
            node_uuids: vec![node.uuid],
            parent_uuids: Vec::new(),
        },
    );
    emit_domain(
        &app,
        DomainEvent::GraphChanged {
            node_uuids: vec![node.uuid],
        },
    );
    if let Some(parent_uuid) = parent_uuid {
        emit_domain(
            &app,
            DomainEvent::AttachmentsChanged {
                parent_uuids: vec![parent_uuid],
            },
        );
    }
    Ok(true)
}

use super::*;
use notes_blob::{BlobHash, BlobStore, InstallOutcome};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

const PORTABLE_ARCHIVE_MAGIC: &[u8; 16] = b"tangleaf-archive";
const PORTABLE_CONTAINER_VERSION: u32 = 1;
#[cfg(not(target_os = "android"))]
const MAX_ARCHIVE_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;
#[cfg(target_os = "android")]
const MAX_ARCHIVE_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ARCHIVE_BLOB_COUNT: u64 = 100_000;
#[cfg(not(target_os = "android"))]
const MAX_ARCHIVE_TOTAL_BLOB_BYTES: u64 = 8 * 1024 * 1024 * 1024;
#[cfg(target_os = "android")]
const MAX_ARCHIVE_TOTAL_BLOB_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_PAGES: usize = 100_000;
const MAX_ARCHIVE_BLOCKS: usize = 1_000_000;
const MAX_ARCHIVE_ATTACHMENTS: usize = 100_000;
const MAX_ARCHIVE_ALIASES: usize = 500_000;
const MAX_ARCHIVE_RECEIPTS: usize = 16;
const COPY_BUFFER_SIZE: usize = 64 * 1024;

struct StagedPortableArchive {
    archive: db::DataArchive,
    blob_store: BlobStore,
    expected_blobs: BTreeMap<BlobHash, u64>,
    _directory: tempfile::TempDir,
}

#[tauri::command]
#[specta::specta]
pub async fn export_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    let filename = format!(
        "tangleaf-{}.notes",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let archive = db::export_archive(&state.conn).await.map_err(err)?;

    #[cfg(not(target_os = "android"))]
    {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("tangleaf archive", &["notes"])
            .set_file_name(filename)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        let destination = path.to_string_lossy().into_owned();
        let blob_store = state.blob_store.clone();
        tauri::async_runtime::spawn_blocking(move || {
            write_archive_atomically(&path, archive, &blob_store, true)
        })
        .await
        .map_err(err)?
        .map_err(err)?;
        Ok(Some(destination))
    }

    #[cfg(target_os = "android")]
    {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .save_file(None, filename, Some("application/octet-stream"), false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        let file = match api.open_file_writable(&uri).await {
            Ok(file) => file,
            Err(open_error) => {
                let error = anyhow::Error::new(open_error)
                    .context("opening the newly created Android archive document");
                let error = match api.remove_file(&uri).await {
                    Ok(()) => error,
                    Err(cleanup_error) => error.context(format!(
                        "also failed to remove the empty Android archive document: {cleanup_error}"
                    )),
                };
                return Err(err(error));
            }
        };
        let destination = uri.uri.clone();
        let blob_store = state.blob_store.clone();
        let write_result = tauri::async_runtime::spawn_blocking(move || {
            let mut file = file;
            // A ContentProvider may expose a pipe or socket-backed descriptor.
            // Flushing and closing is its durability boundary; sync_all can
            // return EINVAL even after a successful write.
            write_portable_archive(&mut file, archive, &blob_store)
        })
        .await;
        match write_result {
            Ok(Ok(())) => Ok(Some(destination)),
            Ok(Err(error)) => {
                let error = match api.remove_file(&uri).await {
                    Ok(()) => error,
                    Err(cleanup_error) => error.context(format!(
                        "also failed to remove the partial Android archive document: {cleanup_error}"
                    )),
                };
                Err(err(error))
            }
            Err(error) => {
                let error = anyhow::Error::new(error)
                    .context("Android archive writer task did not complete");
                let error = match api.remove_file(&uri).await {
                    Ok(()) => error,
                    Err(cleanup_error) => error.context(format!(
                        "also failed to remove the partial Android archive document: {cleanup_error}"
                    )),
                };
                Err(err(error))
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn import_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    #[cfg(not(target_os = "android"))]
    let (source, file) = {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("tangleaf archive", &["notes"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        let file = std::fs::File::open(&path).map_err(err)?;
        (path.to_string_lossy().into_owned(), file)
    };

    #[cfg(target_os = "android")]
    let (source, file) = {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .pick_file(None, &["application/octet-stream"], false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        let file = api.open_file_readable(&uri).await.map_err(err)?;
        (uri.uri, file)
    };

    let staging_parent = app
        .path()
        .app_data_dir()
        .map_err(err)?
        .join("archive-staging");
    std::fs::create_dir_all(&staging_parent).map_err(err)?;
    let staged =
        tauri::async_runtime::spawn_blocking(move || read_portable_archive(file, &staging_parent))
            .await
            .map_err(err)?
            .map_err(err)?;
    write_backup(&app, &state.conn, &state.blob_store, "before-import")
        .await
        .map_err(err)?;
    let StagedPortableArchive {
        archive,
        blob_store: staged_blob_store,
        expected_blobs,
        _directory: staging_directory,
    } = staged;
    let destination_blob_store = state.blob_store.clone();
    let published_blobs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let callback_published_blobs = published_blobs.clone();
    let callback_destination = destination_blob_store.clone();
    let import_result = db::import_archive_with_precommit(&state.conn, archive, move || {
        publish_staged_blobs(
            &staged_blob_store,
            &callback_destination,
            &expected_blobs,
            &callback_published_blobs,
        )?;
        drop(staging_directory);
        Ok(())
    })
    .await;
    if let Err(import_error) = import_result {
        let candidates = published_blobs
            .lock()
            .map_err(|_| err("archive blob publication tracker was poisoned"))?
            .clone();
        let cleanup_store = destination_blob_store.clone();
        let cleanup_result =
            db::cleanup_unreferenced_attachment_blobs(&state.conn, candidates, move |blob_hash| {
                cleanup_store
                    .remove_verified(blob_hash, super::attachments::MAX_ATTACHMENT_SIZE)?;
                Ok(())
            })
            .await;
        let import_error = match cleanup_result {
            Ok(_) => import_error,
            Err(cleanup_error) => import_error.context(format!(
                "archive metadata rolled back, but orphan blob cleanup also failed: {cleanup_error:#}"
            )),
        };
        return Err(err(import_error));
    }
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
        "tangleaf-{reason}-{}-{}.notes",
        chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f"),
        uuid::Uuid::now_v7()
    ));
    let archive = db::export_archive(connection).await?;
    let blob_store = blob_store.clone();
    let published_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        write_archive_atomically(&published_path, archive, &blob_store, false)
    })
    .await??;
    Ok(path)
}

fn write_archive_atomically(
    destination: &Path,
    archive: db::DataArchive,
    blob_store: &BlobStore,
    replace: bool,
) -> anyhow::Result<()> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("archive destination has no parent directory")?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".tangleaf-export-")
        .tempfile_in(parent)?;
    write_portable_archive(&mut temporary, archive, blob_store)?;
    temporary.as_file_mut().sync_all()?;
    let published = if replace {
        temporary.persist(destination)?
    } else {
        temporary.persist_noclobber(destination)?
    };
    published.sync_all()?;
    sync_directory(parent)?;
    Ok(())
}

fn write_portable_archive(
    writer: &mut impl Write,
    archive: db::DataArchive,
    blob_store: &BlobStore,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        archive.format == "tangleaf" && archive.version == db::ARCHIVE_VERSION,
        "cannot export an unsupported tangleaf archive manifest"
    );
    validate_archive_shape(&archive)?;
    let expected = archive_blob_sizes(&archive)?;
    validate_archive_blob_budget(&expected)?;
    let manifest = serde_json::to_vec(&archive)?;
    anyhow::ensure!(
        manifest.len() as u64 <= MAX_ARCHIVE_MANIFEST_BYTES,
        "archive manifest exceeds the {MAX_ARCHIVE_MANIFEST_BYTES}-byte limit"
    );

    writer.write_all(PORTABLE_ARCHIVE_MAGIC)?;
    writer.write_all(&PORTABLE_CONTAINER_VERSION.to_be_bytes())?;
    write_u64(writer, manifest.len() as u64)?;
    writer.write_all(&manifest)?;
    write_u64(writer, expected.len() as u64)?;
    for (blob_hash, expected_size) in expected {
        writer.write_all(blob_hash.as_bytes())?;
        write_u64(writer, expected_size)?;
        let verified =
            blob_store.open_verified(blob_hash, super::attachments::MAX_ATTACHMENT_SIZE)?;
        anyhow::ensure!(
            verified.blob.size == expected_size,
            "attachment blob {blob_hash} has size {}, expected {expected_size}",
            verified.blob.size
        );
        copy_verified_blob(verified.into_file(), writer, blob_hash, expected_size)?;
    }
    writer.flush()?;
    Ok(())
}

fn read_portable_archive(
    mut reader: impl Read,
    staging_parent: &Path,
) -> anyhow::Result<StagedPortableArchive> {
    let mut magic = [0_u8; PORTABLE_ARCHIVE_MAGIC.len()];
    reader.read_exact(&mut magic)?;
    anyhow::ensure!(
        &magic == PORTABLE_ARCHIVE_MAGIC,
        "selected file is not a tangleaf portable archive"
    );
    let version = read_u32(&mut reader)?;
    anyhow::ensure!(
        version == PORTABLE_CONTAINER_VERSION,
        "unsupported tangleaf portable container version {version}"
    );
    let manifest_size = read_u64(&mut reader)?;
    anyhow::ensure!(
        manifest_size <= MAX_ARCHIVE_MANIFEST_BYTES,
        "archive manifest exceeds the {MAX_ARCHIVE_MANIFEST_BYTES}-byte limit"
    );
    let manifest_size = usize::try_from(manifest_size)?;
    let mut manifest = Vec::new();
    manifest
        .try_reserve_exact(manifest_size)
        .context("reserving archive manifest memory")?;
    manifest.resize(manifest_size, 0);
    reader.read_exact(&mut manifest)?;
    let archive: db::DataArchive = serde_json::from_slice(&manifest)?;
    anyhow::ensure!(
        archive.format == "tangleaf" && archive.version == db::ARCHIVE_VERSION,
        "unsupported archive format {} version {}",
        archive.format,
        archive.version
    );
    validate_archive_shape(&archive)?;
    let expected = archive_blob_sizes(&archive)?;
    validate_archive_blob_budget(&expected)?;
    let record_count = read_u64(&mut reader)?;
    anyhow::ensure!(
        record_count == expected.len() as u64,
        "archive attachment metadata and blob records do not match"
    );

    let staging_directory = tempfile::Builder::new()
        .prefix(".archive-stage-")
        .tempdir_in(staging_parent)?;
    let staging_blob_store = BlobStore::new(staging_directory.path());
    for (expected_hash, expected_size) in &expected {
        let mut raw_hash = [0_u8; 32];
        reader.read_exact(&mut raw_hash)?;
        let record_hash = BlobHash::from_bytes(raw_hash);
        let record_size = read_u64(&mut reader)?;
        anyhow::ensure!(
            record_hash == *expected_hash && record_size == *expected_size,
            "archive blob records are not canonical or do not match attachment metadata"
        );
        let mut record = reader.by_ref().take(record_size);
        let installed = staging_blob_store.install_reader(
            &mut record,
            record_hash,
            super::attachments::MAX_ATTACHMENT_SIZE,
        )?;
        anyhow::ensure!(
            record.limit() == 0 && installed.blob.size == *expected_size,
            "archive blob {record_hash} is truncated or has the wrong size"
        );
    }
    let mut trailing = [0_u8; 1];
    anyhow::ensure!(
        reader.read(&mut trailing)? == 0,
        "archive contains trailing data"
    );
    Ok(StagedPortableArchive {
        archive,
        blob_store: staging_blob_store,
        expected_blobs: expected,
        _directory: staging_directory,
    })
}

fn validate_archive_shape(archive: &db::DataArchive) -> anyhow::Result<()> {
    anyhow::ensure!(
        archive.pages.len() <= MAX_ARCHIVE_PAGES
            && archive.page_identities.len() <= MAX_ARCHIVE_PAGES,
        "archive contains more than {MAX_ARCHIVE_PAGES} pages"
    );
    anyhow::ensure!(
        archive.blocks.len() <= MAX_ARCHIVE_BLOCKS,
        "archive contains more than {MAX_ARCHIVE_BLOCKS} blocks"
    );
    anyhow::ensure!(
        archive.attachments.len() <= MAX_ARCHIVE_ATTACHMENTS,
        "archive contains more than {MAX_ARCHIVE_ATTACHMENTS} attachments"
    );
    anyhow::ensure!(
        archive.page_aliases.len() <= MAX_ARCHIVE_ALIASES,
        "archive contains more than {MAX_ARCHIVE_ALIASES} aliases"
    );
    anyhow::ensure!(
        archive.external_import_receipts.len() <= MAX_ARCHIVE_RECEIPTS,
        "archive contains more than {MAX_ARCHIVE_RECEIPTS} import receipts"
    );
    Ok(())
}

fn archive_blob_sizes(archive: &db::DataArchive) -> anyhow::Result<BTreeMap<BlobHash, u64>> {
    let mut expected = BTreeMap::new();
    for attachment in &archive.attachments {
        anyhow::ensure!(
            attachment.size <= super::attachments::MAX_ATTACHMENT_SIZE,
            "attachment {} exceeds the supported size limit",
            attachment.uuid
        );
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

fn validate_archive_blob_budget(expected: &BTreeMap<BlobHash, u64>) -> anyhow::Result<()> {
    anyhow::ensure!(
        expected.len() as u64 <= MAX_ARCHIVE_BLOB_COUNT,
        "archive contains more than {MAX_ARCHIVE_BLOB_COUNT} distinct blobs"
    );
    let total = expected.values().try_fold(0_u64, |total, size| {
        total
            .checked_add(*size)
            .context("archive blob size overflow")
    })?;
    anyhow::ensure!(
        total <= MAX_ARCHIVE_TOTAL_BLOB_BYTES,
        "archive blobs exceed the {MAX_ARCHIVE_TOTAL_BLOB_BYTES}-byte total limit"
    );
    Ok(())
}

fn publish_staged_blobs(
    staging: &BlobStore,
    destination: &BlobStore,
    expected: &BTreeMap<BlobHash, u64>,
    published: &std::sync::Mutex<Vec<BlobHash>>,
) -> anyhow::Result<()> {
    for (&blob_hash, &expected_size) in expected {
        let verified = staging.open_verified(blob_hash, expected_size)?;
        anyhow::ensure!(
            verified.blob.size == expected_size,
            "staged archive blob {blob_hash} has the wrong size"
        );
        let installed = destination.install_reader(
            verified.into_file(),
            blob_hash,
            super::attachments::MAX_ATTACHMENT_SIZE,
        )?;
        anyhow::ensure!(
            installed.blob.size == expected_size,
            "published archive blob {blob_hash} has the wrong size"
        );
        if installed.outcome == InstallOutcome::Installed {
            published
                .lock()
                .map_err(|_| anyhow::anyhow!("archive blob publication tracker was poisoned"))?
                .push(blob_hash);
        }
    }
    Ok(())
}

fn copy_verified_blob(
    mut source: std::fs::File,
    writer: &mut impl Write,
    expected_hash: BlobHash,
    expected_size: u64,
) -> anyhow::Result<()> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_SIZE];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .context("attachment size overflow while exporting")?;
        anyhow::ensure!(
            size <= expected_size,
            "attachment blob {expected_hash} changed while it was being exported"
        );
        hasher.update(&buffer[..count]);
        writer.write_all(&buffer[..count])?;
    }
    let actual_hash = BlobHash::from_bytes(hasher.finalize().into());
    anyhow::ensure!(
        size == expected_size && actual_hash == expected_hash,
        "attachment blob {expected_hash} changed while it was being exported"
    );
    Ok(())
}

fn write_u64(writer: &mut impl Write, value: u64) -> std::io::Result<()> {
    writer.write_all(&value.to_be_bytes())
}

fn read_u32(reader: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0_u8; size_of::<u32>()];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> std::io::Result<u64> {
    let mut bytes = [0_u8; size_of::<u64>()];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ_WRITE_DELETE: u32 = 0x0000_0007;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ_WRITE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?
        .sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "directory synchronization is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn archive_with_duplicate_blob(hash: BlobHash, size: u64) -> db::DataArchive {
        let owner = notes_core::AttachmentOwner::Page(uuid::Uuid::now_v7());
        let attachment = |filename: &str| db::Attachment {
            uuid: uuid::Uuid::now_v7(),
            owner,
            blob_hash: hash,
            filename: filename.into(),
            mime: "application/octet-stream".into(),
            size,
            created_at: 0,
        };
        db::DataArchive {
            format: "tangleaf".into(),
            version: db::ARCHIVE_VERSION,
            workspace_uuid: uuid::Uuid::now_v7(),
            exported_at: 0,
            page_identities: Vec::new(),
            page_aliases: Vec::new(),
            pages: Vec::new(),
            blocks: Vec::new(),
            attachments: vec![attachment("one.bin"), attachment("two.bin")],
            external_import_receipts: Vec::new(),
        }
    }

    #[test]
    fn portable_archive_streams_one_verified_record_per_hash() {
        let source_directory = tempfile::tempdir().unwrap();
        let source_store = BlobStore::new(source_directory.path());
        let payload = b"shared attachment bytes";
        let hash = BlobHash::digest(payload);
        source_store
            .install_reader(payload.as_slice(), hash, payload.len() as u64)
            .unwrap();
        let archive = archive_with_duplicate_blob(hash, payload.len() as u64);
        let mut encoded = Vec::new();

        write_portable_archive(&mut encoded, archive.clone(), &source_store).unwrap();

        let destination_directory = tempfile::tempdir().unwrap();
        let decoded =
            read_portable_archive(encoded.as_slice(), destination_directory.path()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded.archive).unwrap(),
            serde_json::to_value(archive).unwrap()
        );
        let verified = decoded
            .blob_store
            .open_verified(hash, payload.len() as u64)
            .unwrap();
        assert_eq!(verified.blob.size, payload.len() as u64);
    }

    #[test]
    fn portable_archive_rejects_corruption_and_trailing_data() {
        let source_directory = tempfile::tempdir().unwrap();
        let source_store = BlobStore::new(source_directory.path());
        let payload = b"portable payload";
        let hash = BlobHash::digest(payload);
        source_store
            .install_reader(payload.as_slice(), hash, payload.len() as u64)
            .unwrap();
        let archive = archive_with_duplicate_blob(hash, payload.len() as u64);
        let mut encoded = Vec::new();
        write_portable_archive(&mut encoded, archive, &source_store).unwrap();

        let destination = tempfile::tempdir().unwrap();
        let last = encoded.len() - 1;
        encoded[last] ^= 0x01;
        assert!(read_portable_archive(encoded.as_slice(), destination.path()).is_err());

        let mut clean = Vec::new();
        write_portable_archive(
            &mut clean,
            archive_with_duplicate_blob(hash, payload.len() as u64),
            &source_store,
        )
        .unwrap();
        clean.push(0);
        assert!(read_portable_archive(clean.as_slice(), destination.path()).is_err());
    }

    #[test]
    fn portable_archive_rejects_every_truncation_boundary() {
        let source_directory = tempfile::tempdir().unwrap();
        let source_store = BlobStore::new(source_directory.path());
        let payload = b"all framing boundaries must be exact";
        let hash = BlobHash::digest(payload);
        source_store
            .install_reader(payload.as_slice(), hash, payload.len() as u64)
            .unwrap();
        let mut encoded = Vec::new();
        write_portable_archive(
            &mut encoded,
            archive_with_duplicate_blob(hash, payload.len() as u64),
            &source_store,
        )
        .unwrap();

        let staging_parent = tempfile::tempdir().unwrap();
        for end in 0..encoded.len() {
            assert!(
                read_portable_archive(&encoded[..end], staging_parent.path()).is_err(),
                "truncation at byte {end} was accepted"
            );
        }
        read_portable_archive(encoded.as_slice(), staging_parent.path()).unwrap();
    }

    #[test]
    fn portable_archive_rejects_oversized_manifest_before_allocating() {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(PORTABLE_ARCHIVE_MAGIC);
        encoded.extend_from_slice(&PORTABLE_CONTAINER_VERSION.to_be_bytes());
        encoded.extend_from_slice(&(MAX_ARCHIVE_MANIFEST_BYTES + 1).to_be_bytes());
        let staging_parent = tempfile::tempdir().unwrap();

        let error = match read_portable_archive(encoded.as_slice(), staging_parent.path()) {
            Ok(_) => panic!("oversized manifest was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("manifest exceeds"));
    }

    #[test]
    fn failed_atomic_export_preserves_existing_destination() {
        let source_directory = tempfile::tempdir().unwrap();
        let source_store = BlobStore::new(source_directory.path());
        let missing_payload = b"missing from the blob store";
        let hash = BlobHash::digest(missing_payload);
        let archive = archive_with_duplicate_blob(hash, missing_payload.len() as u64);
        let export_directory = tempfile::tempdir().unwrap();
        let destination = export_directory.path().join("existing.notes");
        std::fs::write(&destination, b"previous valid export").unwrap();

        assert!(write_archive_atomically(&destination, archive, &source_store, true).is_err());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous valid export"
        );
    }

    struct FailAfter {
        remaining: usize,
    }

    impl Write for FailAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected writer failure"));
            }
            let count = bytes.len().min(self.remaining);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn portable_archive_propagates_mid_stream_writer_failures() {
        let source_directory = tempfile::tempdir().unwrap();
        let source_store = BlobStore::new(source_directory.path());
        let payload = b"writer failure payload";
        let hash = BlobHash::digest(payload);
        source_store
            .install_reader(payload.as_slice(), hash, payload.len() as u64)
            .unwrap();
        let archive = archive_with_duplicate_blob(hash, payload.len() as u64);
        let mut writer = FailAfter { remaining: 24 };

        let error = write_portable_archive(&mut writer, archive, &source_store).unwrap_err();
        assert!(error.to_string().contains("injected writer failure"));
    }

    #[test]
    fn partial_publication_tracks_only_new_blobs_for_rollback() {
        let staging_directory = tempfile::tempdir().unwrap();
        let staging_store = BlobStore::new(staging_directory.path());
        let mut payloads = [
            b"first staged payload".as_slice(),
            b"second staged bytes".as_slice(),
        ]
        .into_iter()
        .map(|bytes| (BlobHash::digest(bytes), bytes))
        .collect::<Vec<_>>();
        payloads.sort_by_key(|(hash, _)| *hash);
        let mut expected = BTreeMap::new();
        for (hash, bytes) in &payloads {
            staging_store
                .install_reader(*bytes, *hash, bytes.len() as u64)
                .unwrap();
            expected.insert(*hash, bytes.len() as u64);
        }

        let destination_directory = tempfile::tempdir().unwrap();
        let destination_store = BlobStore::new(destination_directory.path());
        let (failing_hash, failing_bytes) = payloads[1];
        destination_store
            .install_reader(failing_bytes, failing_hash, failing_bytes.len() as u64)
            .unwrap();
        std::fs::write(
            destination_store.path_for(failing_hash),
            vec![0_u8; failing_bytes.len()],
        )
        .unwrap();
        let published = std::sync::Mutex::new(Vec::new());

        assert!(
            publish_staged_blobs(&staging_store, &destination_store, &expected, &published,)
                .is_err()
        );
        let first_hash = payloads[0].0;
        assert_eq!(*published.lock().unwrap(), vec![first_hash]);
        assert!(
            destination_store
                .remove_verified(first_hash, 1_024)
                .unwrap()
        );
    }
}

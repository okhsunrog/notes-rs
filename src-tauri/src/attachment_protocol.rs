use crate::commands::AppState;
use anyhow::Context as _;
use image::{ImageFormat, ImageReader};
use notes_blob::{BlobHash, BlobStore};
use notes_core::{Connection, db};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
};
use tauri::{Manager as _, Runtime};

pub(crate) const ATTACHMENT_PROTOCOL: &str = "notes-attachment";
pub(crate) const MAX_MARKDOWN_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MARKDOWN_IMAGE_DIMENSION: u32 = 8_192;
const MAX_MARKDOWN_IMAGE_PIXELS: u64 = 25_000_000;
pub(crate) const MAX_DESCRIPTOR_BATCH: usize = 256;

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
pub enum AttachmentImageMime {
    #[serde(rename = "image/gif")]
    Gif,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/webp")]
    Webp,
}

impl AttachmentImageMime {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Gif => "image/gif",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentImageDescriptor {
    pub attachment_uuid: uuid::Uuid,
    pub byte_size: u64,
    pub height: u32,
    pub mime: AttachmentImageMime,
    pub width: u32,
}

struct AuthorizedImage {
    descriptor: AttachmentImageDescriptor,
    expected_hash: BlobHash,
    file: File,
}

pub(crate) async fn resolve_descriptors(
    state: &AppState,
    attachment_uuids: Vec<uuid::Uuid>,
) -> anyhow::Result<Vec<AttachmentImageDescriptor>> {
    anyhow::ensure!(
        attachment_uuids.len() <= MAX_DESCRIPTOR_BATCH,
        "at most {MAX_DESCRIPTOR_BATCH} attachment images can be resolved at once"
    );
    let attachment_uuids = attachment_uuids.into_iter().collect::<BTreeSet<_>>();
    let mut descriptors = Vec::with_capacity(attachment_uuids.len());
    for attachment_uuid in attachment_uuids {
        match open_authorized_image(&state.conn, &state.blob_store, attachment_uuid).await {
            Ok(Some(image)) => descriptors.push(image.descriptor),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%attachment_uuid, %error, "attachment image is unavailable");
            }
        }
    }
    Ok(descriptors)
}

async fn open_authorized_image(
    connection: &Connection,
    blob_store: &BlobStore,
    attachment_uuid: uuid::Uuid,
) -> anyhow::Result<Option<AuthorizedImage>> {
    let Some(attachment) = db::get_attachment(connection, attachment_uuid).await? else {
        return Ok(None);
    };
    if attachment.size == 0 || attachment.size > MAX_MARKDOWN_IMAGE_BYTES {
        return Ok(None);
    }
    let blob_store = blob_store.clone();
    tauri::async_runtime::spawn_blocking(move || inspect_attachment_image(&blob_store, attachment))
        .await
        .context("attachment image inspection task failed")?
}

fn inspect_attachment_image(
    blob_store: &BlobStore,
    attachment: db::Attachment,
) -> anyhow::Result<Option<AuthorizedImage>> {
    let verified = blob_store.open_verified(attachment.blob_hash, MAX_MARKDOWN_IMAGE_BYTES)?;
    anyhow::ensure!(
        verified.blob.size == attachment.size,
        "attachment metadata size does not match its verified blob"
    );
    let byte_size = verified.blob.size;
    let mut file = verified.into_file();
    let reader = ImageReader::new(BufReader::new(&mut file)).with_guessed_format()?;
    let Some(format) = reader.format() else {
        return Ok(None);
    };
    let mime = match format {
        ImageFormat::Gif => AttachmentImageMime::Gif,
        ImageFormat::Jpeg => AttachmentImageMime::Jpeg,
        ImageFormat::Png => AttachmentImageMime::Png,
        ImageFormat::WebP => AttachmentImageMime::Webp,
        _ => return Ok(None),
    };
    let (width, height) = match reader.into_dimensions() {
        Ok(dimensions) => dimensions,
        Err(_) => return Ok(None),
    };
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("attachment image pixel count overflow")?;
    if width == 0
        || height == 0
        || width > MAX_MARKDOWN_IMAGE_DIMENSION
        || height > MAX_MARKDOWN_IMAGE_DIMENSION
        || pixels > MAX_MARKDOWN_IMAGE_PIXELS
    {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(Some(AuthorizedImage {
        descriptor: AttachmentImageDescriptor {
            attachment_uuid: attachment.uuid,
            byte_size,
            height,
            mime,
            width,
        },
        expected_hash: attachment.blob_hash,
        file,
    }))
}

fn read_authorized_body(image: &mut AuthorizedImage) -> anyhow::Result<Vec<u8>> {
    let size = usize::try_from(image.descriptor.byte_size)?;
    let mut body = Vec::new();
    body.try_reserve_exact(size)?;
    body.resize(size, 0);
    image.file.read_exact(&mut body)?;
    let mut trailing = [0_u8; 1];
    anyhow::ensure!(
        image.file.read(&mut trailing)? == 0 && BlobHash::digest(&body) == image.expected_hash,
        "attachment image changed after verification"
    );
    Ok(body)
}

pub(crate) fn protocol<R: Runtime>(
    context: tauri::UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    if context.webview_label() != "main" {
        responder.respond(empty_response(tauri::http::StatusCode::FORBIDDEN));
        return;
    }
    let app = context.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        responder.respond(protocol_response(app, request).await);
    });
}

async fn protocol_response<R: Runtime>(
    app: tauri::AppHandle<R>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let head = match *request.method() {
        tauri::http::Method::GET => false,
        tauri::http::Method::HEAD => true,
        _ => {
            return tauri::http::Response::builder()
                .status(tauri::http::StatusCode::METHOD_NOT_ALLOWED)
                .header(tauri::http::header::ALLOW, "GET, HEAD")
                .header(tauri::http::header::CACHE_CONTROL, "no-store")
                .body(Vec::new())
                .expect("static attachment protocol response is valid");
        }
    };
    let Some(attachment_uuid) = parse_attachment_uri(request.uri()) else {
        return empty_response(tauri::http::StatusCode::BAD_REQUEST);
    };
    let (connection, blob_store) = {
        let Some(state) = app.try_state::<AppState>() else {
            return empty_response(tauri::http::StatusCode::SERVICE_UNAVAILABLE);
        };
        (state.conn.clone(), state.blob_store.clone())
    };
    let mut image = match open_authorized_image(&connection, &blob_store, attachment_uuid).await {
        Ok(Some(image)) => image,
        Ok(None) => return empty_response(tauri::http::StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::warn!(%attachment_uuid, %error, "failed to serve attachment image");
            return empty_response(tauri::http::StatusCode::NOT_FOUND);
        }
    };
    let descriptor = image.descriptor.clone();
    let body = if head {
        Vec::new()
    } else {
        match tauri::async_runtime::spawn_blocking(move || read_authorized_body(&mut image)).await {
            Ok(Ok(body)) => body,
            Ok(Err(error)) => {
                tracing::warn!(%attachment_uuid, %error, "verified attachment image changed while reading");
                return empty_response(tauri::http::StatusCode::NOT_FOUND);
            }
            Err(error) => {
                tracing::warn!(%attachment_uuid, %error, "attachment image read task failed");
                return empty_response(tauri::http::StatusCode::NOT_FOUND);
            }
        }
    };
    tauri::http::Response::builder()
        .status(tauri::http::StatusCode::OK)
        .header(tauri::http::header::CONTENT_TYPE, descriptor.mime.as_str())
        .header(
            tauri::http::header::CONTENT_LENGTH,
            descriptor.byte_size.to_string(),
        )
        .header(tauri::http::header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(body)
        .expect("validated attachment protocol response is valid")
}

fn parse_attachment_uri(uri: &tauri::http::Uri) -> Option<uuid::Uuid> {
    if uri.scheme_str()? != ATTACHMENT_PROTOCOL || uri.authority()?.as_str() != "localhost" {
        return None;
    }
    if uri.query().is_some() {
        return None;
    }
    let segment = uri.path().strip_prefix('/')?;
    if segment.len() != 36
        || segment.contains('/')
        || segment.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return None;
    }
    let uuid = segment.parse::<uuid::Uuid>().ok()?;
    (uuid.hyphenated().to_string() == segment).then_some(uuid)
}

fn empty_response(status: tauri::http::StatusCode) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header(tauri::http::header::CONTENT_LENGTH, "0")
        .header(tauri::http::header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Vec::new())
        .expect("static attachment protocol response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};
    use std::io::{Cursor, Write};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(width, height));
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Png).unwrap();
        encoded.into_inner()
    }

    #[test]
    fn route_accepts_only_a_canonical_attachment_uuid() {
        let uuid = uuid::Uuid::now_v7();
        let valid = format!("{ATTACHMENT_PROTOCOL}://localhost/{uuid}")
            .parse()
            .unwrap();
        assert_eq!(parse_attachment_uri(&valid), Some(uuid));
        for invalid in [
            format!("{ATTACHMENT_PROTOCOL}://localhost/{uuid}/extra"),
            format!("{ATTACHMENT_PROTOCOL}://localhost/{uuid}?download=1"),
            format!(
                "{ATTACHMENT_PROTOCOL}://localhost/{}",
                uuid.to_string().to_uppercase()
            ),
            format!("{ATTACHMENT_PROTOCOL}://evil/{uuid}"),
            format!("asset://localhost/{uuid}"),
        ] {
            assert!(
                parse_attachment_uri(&invalid.parse().unwrap()).is_none(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn inspection_uses_verified_raster_bytes_and_rejects_svg() {
        let directory = tempfile::tempdir().unwrap();
        let store = BlobStore::new(directory.path());
        let payload = png(4, 3);
        let hash = BlobHash::digest(&payload);
        store
            .install_reader(payload.as_slice(), hash, payload.len() as u64)
            .unwrap();
        let attachment = db::Attachment {
            uuid: uuid::Uuid::now_v7(),
            owner: notes_core::AttachmentOwner::Page(uuid::Uuid::now_v7()),
            blob_hash: hash,
            filename: "pixel.png".into(),
            mime: "text/html".into(),
            size: payload.len() as u64,
            created_at: 0,
        };
        let mut image = inspect_attachment_image(&store, attachment.clone())
            .unwrap()
            .unwrap();
        assert_eq!(image.descriptor.mime.as_str(), "image/png");
        assert_eq!((image.descriptor.width, image.descriptor.height), (4, 3));
        assert_eq!(read_authorized_body(&mut image).unwrap(), payload);

        let mut tampered = inspect_attachment_image(&store, attachment)
            .unwrap()
            .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(store.path_for(hash))
            .unwrap()
            .write_all(&vec![0; payload.len()])
            .unwrap();
        assert!(read_authorized_body(&mut tampered).is_err());

        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
        let svg_hash = BlobHash::digest(svg);
        store
            .install_reader(svg.as_slice(), svg_hash, svg.len() as u64)
            .unwrap();
        let svg_attachment = db::Attachment {
            uuid: uuid::Uuid::now_v7(),
            owner: notes_core::AttachmentOwner::Page(uuid::Uuid::now_v7()),
            blob_hash: svg_hash,
            filename: "unsafe.svg".into(),
            mime: "image/svg+xml".into(),
            size: svg.len() as u64,
            created_at: 0,
        };
        assert!(
            inspect_attachment_image(&store, svg_attachment)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn deleted_attachment_is_no_longer_authorized() {
        let directory = tempfile::tempdir().unwrap();
        let connection = db::open(&directory.path().join("notes.db")).await.unwrap();
        let store = BlobStore::new(directory.path());
        let payload = png(2, 2);
        let hash = BlobHash::digest(&payload);
        store
            .install_reader(payload.as_slice(), hash, payload.len() as u64)
            .unwrap();
        let page = db::create_page(&connection, "Images".into()).await.unwrap();
        let attachment = db::create_attachment(
            &connection,
            notes_core::AttachmentOwner::Page(page.uuid),
            hash,
            "pixel.png".into(),
            "application/octet-stream".into(),
            payload.len() as u64,
        )
        .await
        .unwrap();
        assert!(
            open_authorized_image(&connection, &store, attachment.uuid)
                .await
                .unwrap()
                .is_some()
        );
        db::delete_attachment(&connection, attachment.uuid)
            .await
            .unwrap();
        assert!(
            open_authorized_image(&connection, &store, attachment.uuid)
                .await
                .unwrap()
                .is_none()
        );
    }
}

use crate::commands::AppState;
use anyhow::Context as _;
use futures::{StreamExt as _, stream};
use image::{ImageFormat, ImageReader};
use notes_blob::{BlobHash, BlobStore};
use notes_core::{Connection, db};
use serde::Serialize;
use std::{
    collections::{BTreeSet, HashMap},
    io::Cursor,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};
use tauri::{Manager as _, Runtime};

pub(crate) const ATTACHMENT_PROTOCOL: &str = "notes-attachment";
pub(crate) const MAX_MARKDOWN_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MARKDOWN_IMAGE_DIMENSION: u32 = 8_192;
const MAX_MARKDOWN_IMAGE_PIXELS: u64 = 25_000_000;
pub(crate) const MAX_DESCRIPTOR_BATCH: usize = 256;
const DESCRIPTOR_CONCURRENCY: usize = 4;
const IMAGE_CACHE_FORMAT_VERSION: u32 = 2;
const PREVIEW_MAX_EDGE: u32 = 1_024;
/// Lossy WebP quality for inline previews. The `image` crate only encodes
/// lossless WebP (photo previews came out at hundreds of KB with slow decodes),
/// so previews go through `zenwebp` instead — a pure-Rust lossy encoder with
/// output within a few percent of libwebp's.
const PREVIEW_WEBP_QUALITY: f32 = 80.0;
const IMMUTABLE_CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

/// Shared by foreground requests and all background warm-ups in one workspace.
pub(crate) struct PreviewJobs {
    keys: Mutex<HashMap<BlobHash, Weak<tokio::sync::Mutex<()>>>>,
    slots: Arc<tokio::sync::Semaphore>,
    background: tokio::sync::Semaphore,
    #[cfg(test)]
    generations: std::sync::atomic::AtomicUsize,
}

impl Default for PreviewJobs {
    fn default() -> Self {
        Self {
            keys: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(2)),
            // Background work cannot occupy both generation slots.
            background: tokio::sync::Semaphore::new(1),
            #[cfg(test)]
            generations: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl PreviewJobs {
    fn key(&self, hash: BlobHash) -> Arc<tokio::sync::Mutex<()>> {
        let mut keys = self.keys.lock().expect("preview key registry poisoned");
        keys.retain(|_, value| value.strong_count() > 0);
        if let Some(key) = keys.get(&hash).and_then(Weak::upgrade) {
            return key;
        }
        let key = Arc::new(tokio::sync::Mutex::new(()));
        keys.insert(hash, Arc::downgrade(&key));
        key
    }
}

pub(crate) fn extract_attachment_uuids<'a>(
    markdown: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<uuid::Uuid> {
    const PREFIX: &str = "notes-attachment:";
    const UUID_LEN: usize = 36;
    let mut uuids = BTreeSet::new();
    for source in markdown {
        let mut remaining = source;
        while let Some(index) = remaining.find(PREFIX) {
            let candidate = &remaining[index + PREFIX.len()..];
            let mut consumed = 1;
            if candidate.len() >= UUID_LEN
                && let Ok(uuid) = candidate[..UUID_LEN].parse()
            {
                uuids.insert(uuid);
                consumed = UUID_LEN;
                if uuids.len() == MAX_DESCRIPTOR_BATCH {
                    return uuids;
                }
            }
            remaining = &candidate[candidate.len().min(consumed)..];
        }
    }
    uuids
}

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
    #[specta(type = String)]
    pub blob_hash: BlobHash,
    pub byte_size: u64,
    pub height: u32,
    pub mime: AttachmentImageMime,
    pub preview_height: u32,
    pub preview_width: u32,
    /// Versioned into the immutable resource URL. Bump the cache format when
    /// preview bytes or sizing rules change so WebView caches cannot retain an
    /// older derivative under the same URL.
    pub resource_version: u32,
    pub width: u32,
}

struct AuthorizedImage {
    descriptor: AttachmentImageDescriptor,
    cache: db::AttachmentImageCache,
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
    let connection = state.conn.clone();
    let blob_store = state.blob_store.clone();
    let images: Vec<AuthorizedImage> = stream::iter(attachment_uuids)
        .map(|attachment_uuid| {
            let connection = connection.clone();
            let blob_store = blob_store.clone();
            async move {
                match open_authorized_image(&connection, &blob_store, attachment_uuid).await {
                    Ok(image) => image,
                    Err(error) => {
                        tracing::warn!(%attachment_uuid, %error, "attachment image is unavailable");
                        None
                    }
                }
            }
        })
        .buffer_unordered(DESCRIPTOR_CONCURRENCY)
        .filter_map(|image| async move { image })
        .collect()
        .await;
    let descriptors = images
        .iter()
        .map(|image| image.descriptor.clone())
        .collect();
    warm_preview_cache(state, images);
    Ok(descriptors)
}

/// Generates missing previews in the background right after a page resolves its
/// descriptors, so the decode/resize/encode work happens off the first-scroll
/// path instead of on demand when the `<img>` enters the viewport mid-fling.
fn warm_preview_cache(state: &AppState, images: Vec<AuthorizedImage>) {
    let warm: Vec<AuthorizedImage> = images
        .into_iter()
        .filter(|image| image.cache.preview_hash.is_none())
        .collect();
    if warm.is_empty() {
        return;
    }
    let connection = state.conn.clone();
    let blob_store = state.blob_store.clone();
    let image_cache = state.image_cache.clone();
    let jobs = state.preview_jobs.clone();
    tauri::async_runtime::spawn(async move {
        for image in warm {
            let attachment_uuid = image.descriptor.attachment_uuid;
            if let Err(error) = read_or_create_preview(
                connection.clone(),
                blob_store.clone(),
                image_cache.clone(),
                image,
                true,
                jobs.clone(),
            )
            .await
            {
                tracing::debug!(%attachment_uuid, %error, "preview warm-up failed");
            }
        }
    });
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
    if let Some(cache) = db::get_attachment_image_cache(connection, attachment.blob_hash).await?
        && cache.format_version == IMAGE_CACHE_FORMAT_VERSION
        && cache.byte_size == attachment.size
        && let Some(image) = authorized_from_cache(&attachment, cache)
    {
        return Ok(Some(image));
    }
    let blob_store = blob_store.clone();
    let inspected = tauri::async_runtime::spawn_blocking(move || {
        inspect_attachment_image(&blob_store, attachment)
    })
    .await
    .context("attachment image inspection task failed")??;
    if let Some(image) = &inspected {
        db::upsert_attachment_image_cache(connection, image.cache.clone()).await?;
    }
    Ok(inspected)
}

fn inspect_attachment_image(
    blob_store: &BlobStore,
    attachment: db::Attachment,
) -> anyhow::Result<Option<AuthorizedImage>> {
    let bytes = blob_store.read_verified(attachment.blob_hash, MAX_MARKDOWN_IMAGE_BYTES)?;
    anyhow::ensure!(
        bytes.len() as u64 == attachment.size,
        "attachment metadata size does not match its verified blob"
    );
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
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
    let (preview_width, preview_height) = preview_dimensions(width, height);
    let cache = db::AttachmentImageCache {
        blob_hash: attachment.blob_hash,
        format_version: IMAGE_CACHE_FORMAT_VERSION,
        byte_size: attachment.size,
        mime: mime.as_str().into(),
        width,
        height,
        preview_hash: None,
        preview_size: None,
        preview_width: None,
        preview_height: None,
    };
    Ok(Some(AuthorizedImage {
        descriptor: AttachmentImageDescriptor {
            attachment_uuid: attachment.uuid,
            blob_hash: attachment.blob_hash,
            byte_size: attachment.size,
            height,
            mime,
            preview_height,
            preview_width,
            resource_version: IMAGE_CACHE_FORMAT_VERSION,
            width,
        },
        cache,
    }))
}

fn authorized_from_cache(
    attachment: &db::Attachment,
    cache: db::AttachmentImageCache,
) -> Option<AuthorizedImage> {
    let mime = mime_from_str(&cache.mime)?;
    let pixels = u64::from(cache.width).checked_mul(u64::from(cache.height))?;
    if cache.width == 0
        || cache.height == 0
        || cache.width > MAX_MARKDOWN_IMAGE_DIMENSION
        || cache.height > MAX_MARKDOWN_IMAGE_DIMENSION
        || pixels > MAX_MARKDOWN_IMAGE_PIXELS
    {
        return None;
    }
    let (preview_width, preview_height) = preview_dimensions(cache.width, cache.height);
    Some(AuthorizedImage {
        descriptor: AttachmentImageDescriptor {
            attachment_uuid: attachment.uuid,
            blob_hash: attachment.blob_hash,
            byte_size: attachment.size,
            height: cache.height,
            mime,
            preview_height,
            preview_width,
            resource_version: IMAGE_CACHE_FORMAT_VERSION,
            width: cache.width,
        },
        cache,
    })
}

fn mime_from_str(mime: &str) -> Option<AttachmentImageMime> {
    match mime {
        "image/gif" => Some(AttachmentImageMime::Gif),
        "image/jpeg" => Some(AttachmentImageMime::Jpeg),
        "image/png" => Some(AttachmentImageMime::Png),
        "image/webp" => Some(AttachmentImageMime::Webp),
        _ => None,
    }
}

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width <= PREVIEW_MAX_EDGE && height <= PREVIEW_MAX_EDGE {
        return (width, height);
    }
    let scale = f64::from(PREVIEW_MAX_EDGE) / f64::from(width.max(height));
    (
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    )
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
    let Some(route) = parse_attachment_uri(request.uri()) else {
        return empty_response(tauri::http::StatusCode::BAD_REQUEST);
    };
    let (connection, blob_store, image_cache, jobs) = {
        let Some(state) = app.try_state::<AppState>() else {
            return empty_response(tauri::http::StatusCode::SERVICE_UNAVAILABLE);
        };
        (
            state.conn.clone(),
            state.blob_store.clone(),
            state.image_cache.clone(),
            state.preview_jobs.clone(),
        )
    };
    let image = match open_authorized_image(&connection, &blob_store, route.attachment_uuid).await {
        Ok(Some(image)) => image,
        Ok(None) => return empty_response(tauri::http::StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::warn!(attachment_uuid = %route.attachment_uuid, %error, "failed to serve attachment image");
            return empty_response(tauri::http::StatusCode::NOT_FOUND);
        }
    };
    if image.descriptor.blob_hash != route.blob_hash {
        return empty_response(tauri::http::StatusCode::NOT_FOUND);
    }
    let descriptor = image.descriptor.clone();
    let resource = match route.variant {
        ImageVariant::Original => read_original(blob_store, &descriptor, head).await,
        ImageVariant::Preview => {
            read_or_create_preview(connection, blob_store, image_cache, image, head, jobs).await
        }
    };
    let resource = match resource {
        Ok(resource) => resource,
        Err(error) => {
            tracing::warn!(attachment_uuid = %route.attachment_uuid, %error, "attachment image resource failed");
            return empty_response(tauri::http::StatusCode::NOT_FOUND);
        }
    };
    tauri::http::Response::builder()
        .status(tauri::http::StatusCode::OK)
        .header(tauri::http::header::CONTENT_TYPE, resource.mime)
        .header(
            tauri::http::header::CONTENT_LENGTH,
            resource.byte_size.to_string(),
        )
        .header(tauri::http::header::CACHE_CONTROL, IMMUTABLE_CACHE_CONTROL)
        .header(tauri::http::header::ETAG, format!("\"{}\"", resource.hash))
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(resource.body)
        .expect("validated attachment protocol response is valid")
}

struct ImageResource {
    body: Vec<u8>,
    byte_size: u64,
    hash: BlobHash,
    mime: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageVariant {
    Preview,
    Original,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageRoute {
    attachment_uuid: uuid::Uuid,
    blob_hash: BlobHash,
    variant: ImageVariant,
}

async fn read_original(
    blob_store: BlobStore,
    descriptor: &AttachmentImageDescriptor,
    head: bool,
) -> anyhow::Result<ImageResource> {
    let hash = descriptor.blob_hash;
    let byte_size = descriptor.byte_size;
    let body = if head {
        Vec::new()
    } else {
        tauri::async_runtime::spawn_blocking(move || {
            blob_store.read_verified(hash, MAX_MARKDOWN_IMAGE_BYTES)
        })
        .await
        .context("attachment image read task failed")??
    };
    anyhow::ensure!(
        head || body.len() as u64 == byte_size,
        "attachment image size changed"
    );
    Ok(ImageResource {
        body,
        byte_size,
        hash,
        mime: descriptor.mime.as_str(),
    })
}

async fn read_or_create_preview(
    connection: Connection,
    blob_store: BlobStore,
    image_cache: BlobStore,
    image: AuthorizedImage,
    head: bool,
    jobs: Arc<PreviewJobs>,
) -> anyhow::Result<ImageResource> {
    // Keep the lock through cache publication even if the requesting future is
    // cancelled: spawn_blocking work cannot be cancelled once it starts.
    tauri::async_runtime::spawn(async move {
        read_or_create_preview_inner(connection, blob_store, image_cache, image, head, jobs).await
    })
    .await
    .context("preview request task failed")?
}

async fn read_or_create_preview_inner(
    connection: Connection,
    blob_store: BlobStore,
    image_cache: BlobStore,
    mut image: AuthorizedImage,
    head: bool,
    jobs: Arc<PreviewJobs>,
) -> anyhow::Result<ImageResource> {
    let started = Instant::now();
    let _background = if head {
        Some(jobs.background.acquire().await?)
    } else {
        None
    };
    let key = jobs.key(image.descriptor.blob_hash);
    let _key = key.lock().await;
    // The descriptor may predate another request's completed generation.
    if let Some(cache) =
        db::get_attachment_image_cache(&connection, image.descriptor.blob_hash).await?
        && cache.format_version == IMAGE_CACHE_FORMAT_VERSION
        && cache.byte_size == image.descriptor.byte_size
    {
        image.cache = cache;
    }
    if let (Some(hash), Some(byte_size)) = (image.cache.preview_hash, image.cache.preview_size) {
        let cache = image_cache.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            cache.read_verified(hash, MAX_MARKDOWN_IMAGE_BYTES)
        })
        .await
        .context("preview image read task failed")?
        {
            Ok(body) if body.len() as u64 == byte_size => {
                return Ok(ImageResource {
                    body: if head { Vec::new() } else { body },
                    byte_size,
                    hash,
                    mime: "image/webp",
                });
            }
            Ok(_) => {
                tracing::warn!(%hash, "cached image preview size changed; rebuilding");
            }
            Err(error) => {
                tracing::warn!(%hash, %error, "cached image preview is unavailable; rebuilding");
            }
        }
        let cache = image_cache.clone();
        tauri::async_runtime::spawn_blocking(move || cache.discard_unverified(hash))
            .await
            .context("stale preview cleanup task failed")??;
    }

    let source_hash = image.descriptor.blob_hash;
    let source_mime = image.descriptor.mime;
    let preview_width = image.descriptor.preview_width;
    let preview_height = image.descriptor.preview_height;
    let cache_for_install = image_cache.clone();
    let permit = jobs.slots.clone().acquire_owned().await?;
    #[cfg(test)]
    jobs.generations
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let queue_ms = started.elapsed().as_secs_f64() * 1000.0;
    let submitted = Instant::now();
    let generated =
        tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<(BlobHash, Vec<u8>)> {
            let _permit = permit;
            let blocking_queue_ms = submitted.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let source = blob_store.read_verified(source_hash, MAX_MARKDOWN_IMAGE_BYTES)?;
            let read_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let decoded = image::load_from_memory_with_format(&source, image_format(source_mime))?;
            let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let preview = if decoded.width() == preview_width && decoded.height() == preview_height
            {
                decoded
            } else {
                decoded.resize(
                    preview_width,
                    preview_height,
                    image::imageops::FilterType::Triangle,
                )
            };
            let (width, height) = (preview.width(), preview.height());
            let resize_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let config = zenwebp::LossyConfig::new().with_quality(PREVIEW_WEBP_QUALITY);
            let bytes = if preview.color().has_alpha() {
                let pixels = preview.into_rgba8();
                zenwebp::EncodeRequest::lossy(
                    &config,
                    &pixels,
                    zenwebp::PixelLayout::Rgba8,
                    width,
                    height,
                )
                .encode()
            } else {
                let pixels = preview.into_rgb8();
                zenwebp::EncodeRequest::lossy(
                    &config,
                    &pixels,
                    zenwebp::PixelLayout::Rgb8,
                    width,
                    height,
                )
                .encode()
            }
            .map_err(|error| anyhow::anyhow!("preview webp encoding failed: {error}"))?;
            let encode_ms = start.elapsed().as_secs_f64() * 1000.0;
            anyhow::ensure!(
                bytes.len() as u64 <= MAX_MARKDOWN_IMAGE_BYTES,
                "generated preview is too large"
            );
            let start = Instant::now();
            let hash = BlobHash::digest(&bytes);
            cache_for_install.install_reader(bytes.as_slice(), hash, MAX_MARKDOWN_IMAGE_BYTES)?;
            let store_ms = start.elapsed().as_secs_f64() * 1000.0;
            tracing::info!(%source_hash, queue_ms, blocking_queue_ms, read_ms, decode_ms, resize_ms, encode_ms,
                store_ms, bytes = bytes.len(), width, height, background = head,
                "image preview generated");
            Ok((hash, bytes))
        })
        .await
        .context("preview generation task failed")??;
    let (hash, bytes) = generated;
    let byte_size = bytes.len() as u64;
    image.cache.preview_hash = Some(hash);
    image.cache.preview_size = Some(byte_size);
    image.cache.preview_width = Some(preview_width);
    image.cache.preview_height = Some(preview_height);
    let publishing = Instant::now();
    db::upsert_attachment_image_cache(&connection, image.cache).await?;
    tracing::info!(%source_hash, publish_ms = publishing.elapsed().as_secs_f64() * 1000.0,
        total_ms = started.elapsed().as_secs_f64() * 1000.0, "image preview ready");
    Ok(ImageResource {
        body: if head { Vec::new() } else { bytes },
        byte_size,
        hash,
        mime: "image/webp",
    })
}

const fn image_format(mime: AttachmentImageMime) -> ImageFormat {
    match mime {
        AttachmentImageMime::Gif => ImageFormat::Gif,
        AttachmentImageMime::Jpeg => ImageFormat::Jpeg,
        AttachmentImageMime::Png => ImageFormat::Png,
        AttachmentImageMime::Webp => ImageFormat::WebP,
    }
}

fn parse_attachment_uri(uri: &tauri::http::Uri) -> Option<ImageRoute> {
    if uri.scheme_str()? != ATTACHMENT_PROTOCOL || uri.authority()?.as_str() != "localhost" {
        return None;
    }
    if uri.query().is_some() {
        return None;
    }
    let mut segments = uri.path().strip_prefix('/')?.split('/');
    let resource_version = segments.next()?.strip_prefix('v')?.parse::<u32>().ok()?;
    if resource_version != IMAGE_CACHE_FORMAT_VERSION {
        return None;
    }
    let uuid_segment = segments.next()?;
    let hash_segment = segments.next()?;
    let variant = match segments.next()? {
        "preview" => ImageVariant::Preview,
        "original" => ImageVariant::Original,
        _ => return None,
    };
    if segments.next().is_some()
        || uuid_segment.bytes().any(|byte| byte.is_ascii_uppercase())
        || hash_segment.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return None;
    }
    let attachment_uuid = uuid_segment.parse::<uuid::Uuid>().ok()?;
    if attachment_uuid.hyphenated().to_string() != uuid_segment {
        return None;
    }
    Some(ImageRoute {
        attachment_uuid,
        blob_hash: hash_segment.parse().ok()?,
        variant,
    })
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
    use image::{DynamicImage, GenericImageView as _, RgbaImage};
    use std::io::{Cursor, Write};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(width, height));
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Png).unwrap();
        encoded.into_inner()
    }

    #[tokio::test]
    async fn preview_jobs_share_keys_and_reserve_capacity_for_requests() {
        let jobs = PreviewJobs::default();
        let hash = BlobHash::digest(b"same image");
        let key = jobs.key(hash);
        assert!(Arc::ptr_eq(&key, &jobs.key(hash)));
        let held = key.lock().await;
        assert!(jobs.key(hash).try_lock().is_err());
        drop(held);
        drop(key);
        let _other = jobs.key(BlobHash::digest(b"other image"));
        assert!(!jobs.keys.lock().unwrap().contains_key(&hash));
        let _background = jobs.background.try_acquire().unwrap();
        assert!(jobs.background.try_acquire().is_err());
        let _first = jobs.slots.try_acquire().unwrap();
        let second = jobs.slots.try_acquire().unwrap();
        assert!(jobs.slots.try_acquire().is_err());
        drop(second);
        assert!(jobs.slots.try_acquire().is_ok());
    }

    #[test]
    fn route_accepts_only_a_canonical_attachment_uuid() {
        let uuid = uuid::Uuid::now_v7();
        let hash = BlobHash::digest(b"image");
        let version = IMAGE_CACHE_FORMAT_VERSION;
        let valid = format!("{ATTACHMENT_PROTOCOL}://localhost/v{version}/{uuid}/{hash}/preview")
            .parse()
            .unwrap();
        assert_eq!(
            parse_attachment_uri(&valid),
            Some(ImageRoute {
                attachment_uuid: uuid,
                blob_hash: hash,
                variant: ImageVariant::Preview,
            })
        );
        for invalid in [
            format!("{ATTACHMENT_PROTOCOL}://localhost/v{version}/{uuid}/{hash}/preview/extra"),
            format!(
                "{ATTACHMENT_PROTOCOL}://localhost/v{version}/{uuid}/{hash}/preview?download=1"
            ),
            format!(
                "{ATTACHMENT_PROTOCOL}://localhost/v{version}/{}/{hash}/preview",
                uuid.to_string().to_uppercase()
            ),
            format!(
                "{ATTACHMENT_PROTOCOL}://localhost/v{}/{uuid}/{hash}/preview",
                version + 1
            ),
            format!("{ATTACHMENT_PROTOCOL}://evil/v{version}/{uuid}/{hash}/preview"),
            format!("asset://localhost/v{version}/{uuid}/{hash}/preview"),
        ] {
            assert!(
                parse_attachment_uri(&invalid.parse().unwrap()).is_none(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn attachment_extraction_recovers_after_a_malformed_reference() {
        let uuid = uuid::Uuid::now_v7();
        let markdown = format!("notes-attachment:bad then notes-attachment:{uuid}");
        assert_eq!(extract_attachment_uuids([markdown.as_str()]), [uuid].into());
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
        let image = inspect_attachment_image(&store, attachment.clone())
            .unwrap()
            .unwrap();
        assert_eq!(image.descriptor.mime.as_str(), "image/png");
        assert_eq!((image.descriptor.width, image.descriptor.height), (4, 3));
        assert_eq!(
            store.read_verified(hash, payload.len() as u64).unwrap(),
            payload
        );

        assert!(
            inspect_attachment_image(&store, attachment)
                .unwrap()
                .is_some()
        );
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(store.path_for(hash))
            .unwrap()
            .write_all(&vec![0; payload.len()])
            .unwrap();
        assert!(store.read_verified(hash, payload.len() as u64).is_err());

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

    #[tokio::test]
    async fn preview_is_resized_cached_and_rebuilt_after_cache_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let connection = db::open(&directory.path().join("notes.db")).await.unwrap();
        let source_store = BlobStore::new(directory.path().join("source"));
        let preview_store = BlobStore::new(directory.path().join("previews"));
        let payload = png(1_200, 600);
        let source_hash = BlobHash::digest(&payload);
        source_store
            .install_reader(payload.as_slice(), source_hash, payload.len() as u64)
            .unwrap();
        let page = db::create_page(&connection, "Images".into()).await.unwrap();
        let attachment = db::create_attachment(
            &connection,
            notes_core::AttachmentOwner::Page(page.uuid),
            source_hash,
            "wide.png".into(),
            "image/png".into(),
            payload.len() as u64,
        )
        .await
        .unwrap();

        let image = open_authorized_image(&connection, &source_store, attachment.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                image.descriptor.preview_width,
                image.descriptor.preview_height
            ),
            (1_024, 512)
        );
        let stale = open_authorized_image(&connection, &source_store, attachment.uuid)
            .await
            .unwrap()
            .unwrap();
        let jobs = Arc::new(PreviewJobs::default());
        let first_request = read_or_create_preview(
            connection.clone(),
            source_store.clone(),
            preview_store.clone(),
            image,
            false,
            jobs.clone(),
        );
        let background_request = read_or_create_preview(
            connection.clone(),
            source_store.clone(),
            preview_store.clone(),
            stale,
            true,
            jobs.clone(),
        );
        let (first, warmed) = tokio::join!(first_request, background_request);
        let first = first.unwrap();
        assert_eq!(warmed.unwrap().hash, first.hash);
        assert_eq!(
            jobs.generations.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(first.mime, "image/webp");
        assert_eq!(
            image::load_from_memory_with_format(&first.body, ImageFormat::WebP)
                .unwrap()
                .dimensions(),
            (1_024, 512)
        );
        let cached = db::get_attachment_image_cache(&connection, source_hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.preview_hash, Some(first.hash));
        assert_eq!(cached.preview_size, Some(first.byte_size));

        std::fs::write(preview_store.path_for(first.hash), b"corrupt").unwrap();
        let image = open_authorized_image(&connection, &source_store, attachment.uuid)
            .await
            .unwrap()
            .unwrap();
        let rebuilt = read_or_create_preview(
            connection,
            source_store,
            preview_store.clone(),
            image,
            false,
            jobs.clone(),
        )
        .await
        .unwrap();
        assert_eq!(rebuilt.body, first.body);
        assert_eq!(
            jobs.generations.load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        assert_eq!(
            preview_store
                .read_verified(rebuilt.hash, MAX_MARKDOWN_IMAGE_BYTES)
                .unwrap(),
            rebuilt.body
        );
    }
}

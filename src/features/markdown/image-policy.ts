import type { MarkdownRenderContext } from "./types";

const ATTACHMENT_HREF_PREFIX = "notes-attachment:";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const MAX_SOURCE_LENGTH = 8_192;
const MAX_IMAGE_BYTES = 32 * 1024 * 1024;
const MAX_IMAGE_DIMENSION = 8_192;
const MAX_IMAGE_PIXELS = 25_000_000;

export type MarkdownImageMime =
  | "image/avif"
  | "image/gif"
  | "image/jpeg"
  | "image/png"
  | "image/svg+xml"
  | "image/webp";

export type MarkdownImageSource =
  | { kind: "attachment"; attachmentUuid: string; href: string }
  | { kind: "remote"; href: string; hostname: string }
  | { kind: "blocked"; reason: "empty" | "invalid" | "unsupported" };

export interface MarkdownImageResolveRequest {
  alt: string;
  attachmentUuid: string;
  context: MarkdownRenderContext;
  title?: string;
}

export interface MarkdownResolvedImage {
  byteSize: number;
  height: number;
  mime: MarkdownImageMime;
  sanitizedSvg?: true;
  src: string;
  width: number;
}

/** A synchronous boundary over an already-loaded, backend-authorized attachment map. */
export type MarkdownImageResolver = (
  request: MarkdownImageResolveRequest,
) => MarkdownResolvedImage | null;

export type ResolvedImageValidation =
  | { kind: "safe"; image: MarkdownResolvedImage }
  | {
      kind: "blocked";
      reason: "invalid_size" | "unsafe_mime" | "unsafe_src" | "unsanitized_svg";
    };

export function attachmentImageHref(uuid: string): string {
  return `${ATTACHMENT_HREF_PREFIX}${encodeURIComponent(uuid.toLowerCase())}`;
}

export function classifyMarkdownImageSource(rawSource: string): MarkdownImageSource {
  if (!rawSource) return { kind: "blocked", reason: "empty" };
  if (rawSource.length > MAX_SOURCE_LENGTH || hasControlCharacter(rawSource)) {
    return { kind: "blocked", reason: "invalid" };
  }

  if (rawSource.startsWith(ATTACHMENT_HREF_PREFIX)) {
    const uuid = decodeTarget(rawSource.slice(ATTACHMENT_HREF_PREFIX.length));
    if (!uuid || !UUID.test(uuid)) return { kind: "blocked", reason: "invalid" };
    const attachmentUuid = uuid.toLowerCase();
    return {
      kind: "attachment",
      attachmentUuid,
      href: attachmentImageHref(attachmentUuid),
    };
  }

  let parsed: URL;
  try {
    parsed = new URL(rawSource);
  } catch {
    return { kind: "blocked", reason: "unsupported" };
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return { kind: "blocked", reason: "unsupported" };
  }
  return { kind: "remote", href: parsed.href, hostname: parsed.hostname };
}

export function safeMarkdownImageSourceTransform(rawSource: string): string {
  const source = classifyMarkdownImageSource(rawSource);
  return source.kind === "blocked" ? "" : source.href;
}

export function validateResolvedMarkdownImage(
  image: MarkdownResolvedImage,
): ResolvedImageValidation {
  if (
    !Number.isSafeInteger(image.byteSize) ||
    image.byteSize <= 0 ||
    image.byteSize > MAX_IMAGE_BYTES ||
    !validDimension(image.width) ||
    !validDimension(image.height) ||
    image.width * image.height > MAX_IMAGE_PIXELS
  ) {
    return { kind: "blocked", reason: "invalid_size" };
  }
  if (!isSupportedMime(image.mime)) return { kind: "blocked", reason: "unsafe_mime" };
  if (image.mime === "image/svg+xml" && image.sanitizedSvg !== true) {
    return { kind: "blocked", reason: "unsanitized_svg" };
  }
  if (!isAuthorizedLocalImageUrl(image.src)) {
    return { kind: "blocked", reason: "unsafe_src" };
  }
  return { kind: "safe", image };
}

function isAuthorizedLocalImageUrl(src: string): boolean {
  if (src.length > MAX_SOURCE_LENGTH || hasControlCharacter(src)) return false;
  try {
    const parsed = new URL(src);
    if (parsed.protocol === "blob:") return isAuthorizedBlobUrl(parsed.pathname);
    if (parsed.username || parsed.password || parsed.port) return false;
    return (
      (parsed.protocol === "asset:" && parsed.hostname === "localhost") ||
      (parsed.protocol === "http:" && parsed.hostname === "asset.localhost")
    );
  } catch {
    return false;
  }
}

function isAuthorizedBlobUrl(innerSource: string): boolean {
  if (innerSource.startsWith("null/")) return innerSource.length > "null/".length;
  try {
    const inner = new URL(innerSource);
    if (inner.username || inner.password) return false;
    if (inner.protocol === "tauri:") {
      return inner.hostname === "localhost" && !inner.port;
    }
    if (inner.protocol !== "http:" && inner.protocol !== "https:") return false;
    if (inner.hostname === "localhost") return true;
    return inner.hostname === "tauri.localhost" && !inner.port;
  } catch {
    return false;
  }
}

function validDimension(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0 && value <= MAX_IMAGE_DIMENSION;
}

function isSupportedMime(value: string): value is MarkdownImageMime {
  return [
    "image/avif",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/svg+xml",
    "image/webp",
  ].includes(value);
}

function decodeTarget(encoded: string): string | null {
  try {
    const value = decodeURIComponent(encoded).trim();
    return value && !hasControlCharacter(value) ? value : null;
  } catch {
    return null;
  }
}

function hasControlCharacter(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x1f || code === 0x7f) return true;
  }
  return false;
}

import type { MarkdownRenderContext } from "./types";

const ATTACHMENT_HREF_PREFIX = "notes-attachment:";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const MAX_SOURCE_LENGTH = 8_192;
const MAX_IMAGE_BYTES = 32 * 1024 * 1024;
const MAX_IMAGE_DIMENSION = 8_192;
const MAX_IMAGE_PIXELS = 25_000_000;
const MAX_ATTACHMENT_REFERENCES = 256;

export type MarkdownImageMime = "image/gif" | "image/jpeg" | "image/png" | "image/webp";

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
  originalSrc: string;
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
      reason: "invalid_size" | "unsafe_mime" | "unsafe_src";
    };

export function attachmentImageHref(uuid: string): string {
  return `${ATTACHMENT_HREF_PREFIX}${encodeURIComponent(uuid.toLowerCase())}`;
}

export function extractMarkdownAttachmentUuids(markdown: string): string[] {
  const matches = markdown.matchAll(
    /notes-attachment:([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/gi,
  );
  const uuids = new Set<string>();
  for (const match of matches) {
    const uuid = match[1]?.toLowerCase();
    if (uuid && UUID.test(uuid)) uuids.add(uuid);
    if (uuids.size === MAX_ATTACHMENT_REFERENCES) break;
  }
  return [...uuids].sort();
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
  attachmentUuid: string,
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
  const preview = authorizedLocalImageRoute(image.src, attachmentUuid, "preview");
  const original = authorizedLocalImageRoute(image.originalSrc, attachmentUuid, "original");
  if (
    !preview ||
    !original ||
    preview.hash !== original.hash ||
    preview.version !== original.version
  ) {
    return { kind: "blocked", reason: "unsafe_src" };
  }
  return { kind: "safe", image };
}

function authorizedLocalImageRoute(
  src: string,
  attachmentUuid: string,
  variant: "preview" | "original",
): { hash: string; version: number } | null {
  if (src.length > MAX_SOURCE_LENGTH || hasControlCharacter(src)) return null;
  try {
    const parsed = new URL(src);
    if (parsed.username || parsed.password || parsed.port || parsed.search || parsed.hash)
      return null;
    const segments = parsed.pathname.split("/");
    if (
      segments.length !== 5 ||
      !/^v[1-9][0-9]*$/.test(segments[1] ?? "") ||
      segments[2] !== attachmentUuid.toLowerCase() ||
      !/^[0-9a-f]{64}$/.test(segments[3] ?? "") ||
      segments[4] !== variant
    ) {
      return null;
    }
    const allowedOrigin =
      parsed.protocol === "notes-attachment:"
        ? parsed.hostname === "localhost"
        : parsed.protocol === "http:" && parsed.hostname === "notes-attachment.localhost";
    if (!allowedOrigin) return null;
    return {
      hash: segments[3],
      version: Number.parseInt(segments[1].slice(1), 10),
    };
  } catch {
    return null;
  }
}

function validDimension(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0 && value <= MAX_IMAGE_DIMENSION;
}

function isSupportedMime(value: string): value is MarkdownImageMime {
  return ["image/gif", "image/jpeg", "image/png", "image/webp"].includes(value);
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

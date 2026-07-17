import type { MarkdownLinkTarget } from "./types";
import { safeMarkdownImageSourceTransform } from "./image-policy";

const PAGE_HREF_PREFIX = "notes-page:";
const BLOCK_HREF_PREFIX = "notes-block:";
const MAX_URL_LENGTH = 8_192;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export type MarkdownBlockedUrlReason =
  | "empty"
  | "too_long"
  | "control_character"
  | "invalid_internal_target"
  | "invalid_url"
  | "unsupported_protocol";

export type ClassifiedMarkdownUrl =
  | {
      kind: "allowed";
      href: string;
      target: MarkdownLinkTarget;
    }
  | {
      kind: "blocked";
      reason: MarkdownBlockedUrlReason;
    };

export function pageTargetHref(title: string): string {
  return `${PAGE_HREF_PREFIX}${encodeURIComponent(title)}`;
}

export function blockTargetHref(uuid: string): string {
  return `${BLOCK_HREF_PREFIX}${encodeURIComponent(uuid.toLowerCase())}`;
}

export function classifyMarkdownUrl(rawUrl: string): ClassifiedMarkdownUrl {
  if (!rawUrl) return { kind: "blocked", reason: "empty" };
  if (rawUrl.length > MAX_URL_LENGTH) return { kind: "blocked", reason: "too_long" };
  if (hasControlCharacter(rawUrl)) {
    return { kind: "blocked", reason: "control_character" };
  }

  if (rawUrl.startsWith(PAGE_HREF_PREFIX)) {
    const title = decodeInternalTarget(rawUrl.slice(PAGE_HREF_PREFIX.length));
    if (!title) return { kind: "blocked", reason: "invalid_internal_target" };
    return {
      kind: "allowed",
      href: pageTargetHref(title),
      target: { kind: "page", title },
    };
  }

  if (rawUrl.startsWith(BLOCK_HREF_PREFIX)) {
    const uuid = decodeInternalTarget(rawUrl.slice(BLOCK_HREF_PREFIX.length));
    if (!uuid || !UUID.test(uuid)) {
      return { kind: "blocked", reason: "invalid_internal_target" };
    }
    const normalizedUuid = uuid.toLowerCase();
    return {
      kind: "allowed",
      href: blockTargetHref(normalizedUuid),
      target: { kind: "block", uuid: normalizedUuid },
    };
  }

  if (rawUrl.startsWith("#")) {
    return {
      kind: "allowed",
      href: rawUrl,
      target: { kind: "fragment", fragment: rawUrl.slice(1) },
    };
  }

  let parsed: URL;
  try {
    parsed = new URL(rawUrl);
  } catch {
    return { kind: "blocked", reason: "invalid_url" };
  }

  const protocol = parsed.protocol.toLowerCase();
  if (protocol === "http:" || protocol === "https:") {
    return {
      kind: "allowed",
      href: parsed.href,
      target: {
        kind: "external",
        href: parsed.href,
        protocol: protocol === "http:" ? "http" : "https",
      },
    };
  }
  if (protocol === "mailto:") {
    const decoded = decodeUrlComponent(parsed.href);
    if (!decoded || hasControlCharacter(decoded)) {
      return { kind: "blocked", reason: "control_character" };
    }
    return {
      kind: "allowed",
      href: parsed.href,
      target: { kind: "external", href: parsed.href, protocol: "mailto" },
    };
  }

  return { kind: "blocked", reason: "unsupported_protocol" };
}

function hasControlCharacter(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x1f || code === 0x7f) return true;
  }
  return false;
}

/** ReactMarkdown URL transform. Images use their own attachment-only resolver boundary. */
export function safeMarkdownUrlTransform(
  rawUrl: string,
  key?: string,
  node?: Readonly<{ tagName?: string }>,
): string {
  if (key === "src" && node?.tagName === "img") {
    return safeMarkdownImageSourceTransform(rawUrl);
  }
  const classified = classifyMarkdownUrl(rawUrl);
  return classified.kind === "allowed" ? classified.href : "";
}

function decodeInternalTarget(encoded: string): string | null {
  try {
    const value = decodeURIComponent(encoded).trim();
    return value && value.length <= 2_048 && !hasControlCharacter(value) ? value : null;
  } catch {
    return null;
  }
}

function decodeUrlComponent(value: string): string | null {
  try {
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

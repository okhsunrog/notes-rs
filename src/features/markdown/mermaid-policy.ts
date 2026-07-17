import type { Config as DomPurifyConfig, DOMPurify } from "dompurify";
import type { MermaidConfig } from "mermaid";

export const MERMAID_DIALECT_VERSION = "notes-mermaid-v1";
export const MERMAID_LIBRARY_VERSION = "11.16.0";
export const MAX_MERMAID_SOURCE_CHARS = 16_000;
export const MAX_MERMAID_SOURCE_LINES = 320;
export const MAX_MERMAID_SVG_CHARS = 512_000;
export const MERMAID_RENDER_TIMEOUT_MS = 8_000;
export const MERMAID_CACHE_ENTRIES = 24;

export type MermaidTheme = "dark" | "light";

export type MermaidSourceValidation =
  | { kind: "valid" }
  | { kind: "oversize"; reason: "characters" | "lines" };

export function validateMermaidSource(source: string): MermaidSourceValidation {
  if (source.length > MAX_MERMAID_SOURCE_CHARS) {
    return { kind: "oversize", reason: "characters" };
  }
  let lines = 1;
  for (let index = 0; index < source.length; index += 1) {
    if (source.charCodeAt(index) === 0x0a && ++lines > MAX_MERMAID_SOURCE_LINES) {
      return { kind: "oversize", reason: "lines" };
    }
  }
  return { kind: "valid" };
}

export function isMermaidFenceLanguage(language: string | null): boolean {
  return language === "mermaid";
}

export function mermaidCacheKey(source: string, theme: MermaidTheme): string {
  return `${MERMAID_DIALECT_VERSION}\u0000${MERMAID_LIBRARY_VERSION}\u0000${theme}\u0000${source}`;
}

export function stableMermaidId(key: string, instanceId: string): string {
  return `notes-mermaid-${fnv1a(key)}-${fnv1a(instanceId)}`;
}

export function mermaidConfig(theme: MermaidTheme, id: string): MermaidConfig {
  return {
    darkMode: theme === "dark",
    deterministicIds: true,
    deterministicIDSeed: id,
    flowchart: { htmlLabels: false },
    fontFamily: "system-ui, sans-serif",
    htmlLabels: false,
    logLevel: "fatal",
    maxTextSize: MAX_MERMAID_SOURCE_CHARS,
    securityLevel: "strict",
    secure: [
      "darkMode",
      "deterministicIds",
      "deterministicIDSeed",
      "fontFamily",
      "htmlLabels",
      "maxTextSize",
      "securityLevel",
      "secure",
      "startOnLoad",
      "suppressErrorRendering",
      "theme",
    ],
    startOnLoad: false,
    suppressErrorRendering: true,
    theme: theme === "dark" ? "dark" : "default",
  };
}

function fnv1a(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(36);
}

export class BoundedLruCache<Key, Value> {
  readonly #entries = new Map<Key, Value>();

  constructor(readonly capacity: number) {
    if (!Number.isInteger(capacity) || capacity < 1)
      throw new Error("cache capacity must be positive");
  }

  get size(): number {
    return this.#entries.size;
  }

  get(key: Key): Value | undefined {
    const value = this.#entries.get(key);
    if (value === undefined) return undefined;
    this.#entries.delete(key);
    this.#entries.set(key, value);
    return value;
  }

  set(key: Key, value: Value): void {
    this.#entries.delete(key);
    this.#entries.set(key, value);
    while (this.#entries.size > this.capacity) {
      const oldest = this.#entries.keys().next().value;
      if (oldest === undefined) break;
      this.#entries.delete(oldest);
    }
  }

  has(key: Key): boolean {
    return this.#entries.has(key);
  }
}

export interface MermaidRequest {
  id: number;
  signal: AbortSignal;
}

/** Owns the latest-only lifecycle used by a diagram component across prop/theme changes. */
export class MermaidRequestController {
  #current: { controller: AbortController; id: number } | null = null;
  #nextId = 0;

  start(): MermaidRequest {
    this.#current?.controller.abort();
    const controller = new AbortController();
    const id = ++this.#nextId;
    this.#current = { controller, id };
    return { id, signal: controller.signal };
  }

  isCurrent(id: number): boolean {
    return this.#current?.id === id && !this.#current.controller.signal.aborted;
  }

  cancel(id?: number): void {
    if (!this.#current || (id !== undefined && this.#current.id !== id)) return;
    this.#current.controller.abort();
    this.#current = null;
  }
}

const FORBIDDEN_SVG_TAGS = [
  "a",
  "animate",
  "animateMotion",
  "animateTransform",
  "audio",
  "canvas",
  "discard",
  "embed",
  "feImage",
  "foreignObject",
  "iframe",
  "image",
  "object",
  "script",
  "set",
  "video",
] as const;

const SVG_SANITIZE_CONFIG: DomPurifyConfig = {
  FORBID_TAGS: [...FORBIDDEN_SVG_TAGS],
  RETURN_TRUSTED_TYPE: false,
  USE_PROFILES: { svg: true, svgFilters: true },
};

const URL_ATTRIBUTE_NAMES = new Set([
  "action",
  "cursor",
  "data",
  "filter",
  "formaction",
  "href",
  "marker-end",
  "marker-mid",
  "marker-start",
  "mask",
  "poster",
  "src",
]);

const FORBIDDEN_URL_TEXT = /(?:javascript|vbscript|data|file|https?|ftp|blob)\s*:/i;
const CSS_NETWORK_DIRECTIVE = /@(?:import|font-face|namespace)\b/i;
const LOCAL_FRAGMENT = /^#[A-Za-z_][A-Za-z0-9_.:-]*$/;

/**
 * Sanitizes Mermaid output for an SVG-image boundary. The result is never inserted as DOM: it is
 * subsequently encoded into an image data URL. DOMPurify removes active markup; this second pass
 * rejects every navigation/subresource primitive, including CSS URL obfuscation.
 */
export function sanitizeMermaidSvg(svg: string, purifier: DOMPurify): string {
  if (svg.length === 0 || svg.length > MAX_MERMAID_SVG_CHARS) {
    throw new Error("Mermaid produced an invalid-sized image");
  }
  const clean = purifier.sanitize(svg, SVG_SANITIZE_CONFIG);
  if (typeof clean !== "string") throw new Error("Mermaid sanitizer returned an invalid image");

  const document = new DOMParser().parseFromString(clean, "image/svg+xml");
  if (document.querySelector("parsererror") || document.documentElement.localName !== "svg") {
    throw new Error("Mermaid produced invalid SVG");
  }

  for (const forbidden of FORBIDDEN_SVG_TAGS) {
    if (document.getElementsByTagName(forbidden).length > 0) {
      throw new Error("Mermaid image contained active content");
    }
  }

  for (const element of document.querySelectorAll("*")) {
    for (const attribute of element.attributes) {
      const name = attribute.localName.toLowerCase();
      const value = attribute.value.trim();
      if (
        attribute.namespaceURI === "http://www.w3.org/2000/xmlns/" &&
        (value === "http://www.w3.org/2000/svg" || value === "http://www.w3.org/1999/xlink")
      ) {
        continue;
      }
      if (name.startsWith("on")) throw new Error("Mermaid image contained an event handler");
      if (FORBIDDEN_URL_TEXT.test(value))
        throw new Error("Mermaid image contained an external URL");
      if (name === "style" || value.toLowerCase().includes("url(")) assertSafeCss(value);
      if (URL_ATTRIBUTE_NAMES.has(name)) assertSafeUrlAttribute(element.localName, name, value);
    }
  }

  for (const style of document.querySelectorAll("style")) assertSafeCss(style.textContent ?? "");
  const serialized = new XMLSerializer().serializeToString(document.documentElement);
  if (serialized.length > MAX_MERMAID_SVG_CHARS) {
    throw new Error("Mermaid produced an oversized image");
  }
  return serialized;
}

function assertSafeUrlAttribute(element: string, name: string, value: string): void {
  if (value === "" || !mayContainUrl(name, value)) return;
  if (
    (element === "use" || element === "textPath") &&
    (name === "href" || name === "xlink:href") &&
    LOCAL_FRAGMENT.test(value)
  ) {
    return;
  }
  if (isLocalCssUrl(value)) return;
  throw new Error("Mermaid image contained a navigable or external reference");
}

function mayContainUrl(name: string, value: string): boolean {
  if (["filter", "cursor", "marker-start", "marker-mid", "marker-end", "mask"].includes(name)) {
    return value !== "none";
  }
  return true;
}

function assertSafeCss(css: string): void {
  if (CSS_NETWORK_DIRECTIVE.test(css) || FORBIDDEN_URL_TEXT.test(css)) {
    throw new Error("Mermaid image contained network-capable CSS");
  }
  const urlPattern = /url\(\s*(["']?)([^)"']+)\1\s*\)/gi;
  let match: RegExpExecArray | null;
  let count = 0;
  while ((match = urlPattern.exec(css))) {
    count += 1;
    if (!LOCAL_FRAGMENT.test(match[2].trim())) {
      throw new Error("Mermaid image contained an external CSS URL");
    }
  }
  if ((css.match(/url\s*\(/gi)?.length ?? 0) !== count) {
    throw new Error("Mermaid image contained malformed CSS URL syntax");
  }
}

function isLocalCssUrl(value: string): boolean {
  const match = /^url\(\s*(["']?)(#[A-Za-z_][A-Za-z0-9_.:-]*)\1\s*\)$/i.exec(value);
  return match !== null;
}

import c from "@shikijs/langs/c";
import css from "@shikijs/langs/css";
import diff from "@shikijs/langs/diff";
import html from "@shikijs/langs/html";
import javascript from "@shikijs/langs/javascript";
import json from "@shikijs/langs/json";
import markdown from "@shikijs/langs/markdown";
import python from "@shikijs/langs/python";
import rust from "@shikijs/langs/rust";
import shellscript from "@shikijs/langs/shellscript";
import sql from "@shikijs/langs/sql";
import toml from "@shikijs/langs/toml";
import typescript from "@shikijs/langs/typescript";
import yaml from "@shikijs/langs/yaml";
import githubDark from "@shikijs/themes/github-dark";
import githubLight from "@shikijs/themes/github-light";
import githubLightHighContrast from "@shikijs/themes/github-light-high-contrast";
import { createHighlighterCoreSync, type ThemeRegistrationRaw } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

/**
 * E-ink with a grayscale panel has no hue to spend, so the mono theme carries meaning with
 * weight and slant only. @shikijs/themes ships no monochrome theme, hence this minimal one.
 */
const EINK_MONO_THEME: ThemeRegistrationRaw = {
  name: "eink-mono",
  type: "light",
  colors: { "editor.background": "#ffffff", "editor.foreground": "#000000" },
  settings: [
    { settings: { background: "#ffffff", foreground: "#000000" } },
    {
      scope: ["comment", "punctuation.definition.comment"],
      settings: { foreground: "#555555" },
    },
    {
      scope: ["keyword", "storage", "storage.type", "constant.language"],
      settings: { fontStyle: "bold" },
    },
    { scope: ["string", "string.quoted"], settings: { fontStyle: "italic" } },
  ],
};

/** Which Shiki theme pair a code block is tokenized with. */
export type CodeTheme = "standard" | "eink" | "eink-mono";

/** Shiki's FontStyle bitmask; only the two flags the mono theme uses are named. */
const FONT_STYLE_ITALIC = 1;
const FONT_STYLE_BOLD = 2;

export function resolveCodeTheme(
  display: "standard" | "eink",
  inkColor: "color" | "mono",
): CodeTheme {
  if (display !== "eink") return "standard";
  return inkColor === "mono" ? "eink-mono" : "eink";
}

/** The e-ink profile is light-only, so both variants resolve to the same theme there. */
function themeNames(theme: CodeTheme): { light: string; dark: string } {
  if (theme === "eink") {
    return { light: "github-light-high-contrast", dark: "github-light-high-contrast" };
  }
  if (theme === "eink-mono") return { light: "eink-mono", dark: "eink-mono" };
  return { light: "github-light", dark: "github-dark" };
}

const MAX_HIGHLIGHT_CHARS = 32_000;
const MAX_HIGHLIGHT_LINES = 200;
const MAX_HIGHLIGHT_LINE_CHARS = 4_096;
const MAX_CACHEABLE_CHARS = 8_192;
const MAX_CACHE_ENTRIES = 24;
const TOKENIZE_TIME_LIMIT_MS = 5;

const LANGUAGE_ALIASES: Readonly<Record<string, string>> = {
  bash: "shellscript",
  c: "c",
  console: "shellscript",
  css: "css",
  diff: "diff",
  html: "html",
  javascript: "javascript",
  js: "javascript",
  json: "json",
  jsonc: "json",
  jsx: "javascript",
  markdown: "markdown",
  md: "markdown",
  patch: "diff",
  plain: "text",
  plaintext: "text",
  py: "python",
  python: "python",
  rs: "rust",
  rust: "rust",
  sh: "shellscript",
  shell: "shellscript",
  shellscript: "shellscript",
  sql: "sql",
  text: "text",
  toml: "toml",
  ts: "typescript",
  tsx: "typescript",
  txt: "text",
  typescript: "typescript",
  yaml: "yaml",
  yml: "yaml",
  zsh: "shellscript",
};

type MarkdownHighlighter = ReturnType<typeof createHighlighterCoreSync>;

let highlighter: MarkdownHighlighter | null = null;

export interface MarkdownCodeToken {
  content: string;
  darkColor?: string;
  lightColor?: string;
  /** Only carried by the monochrome theme, which has nothing but weight and slant to spend. */
  bold?: boolean;
  italic?: boolean;
}

export interface MarkdownCodeHighlight {
  highlighted: boolean;
  language: string;
  lines: MarkdownCodeToken[][];
  requestedLanguage: string | null;
}

const highlightCache = new Map<string, MarkdownCodeHighlight>();

export function highlightMarkdownCode(
  code: string,
  requestedLanguage: string | null,
  theme: CodeTheme = "standard",
): MarkdownCodeHighlight {
  const cleanRequest = normalizeLanguageLabel(requestedLanguage);
  const language = cleanRequest ? (LANGUAGE_ALIASES[cleanRequest] ?? "text") : "text";
  const highlighted = language !== "text" && isHighlightableSource(code);
  const cacheKey = `${language}\u0000${theme}\u0000${highlighted ? "highlight" : "plain"}\u0000${code}`;
  const cached = code.length <= MAX_CACHEABLE_CHARS ? highlightCache.get(cacheKey) : undefined;
  if (cached) return cached;

  let result: MarkdownCodeHighlight;
  if (!highlighted) {
    result = {
      highlighted: false,
      language: "text",
      lines: plainLines(code),
      requestedLanguage: cleanRequest,
    };
  } else {
    try {
      const mono = theme === "eink-mono";
      const lines = getHighlighter().codeToTokensWithThemes(code, {
        lang: language,
        themes: themeNames(theme),
        tokenizeMaxLineLength: MAX_HIGHLIGHT_LINE_CHARS,
        tokenizeTimeLimit: TOKENIZE_TIME_LIMIT_MS,
      });
      result = {
        highlighted: true,
        language,
        lines: lines.map((line) =>
          line.map((token) => {
            // Weight and slant are only read from the mono theme; the color themes keep the
            // plain rendering they have always had.
            const style = mono ? (token.variants.light?.fontStyle ?? 0) : 0;
            return {
              content: token.content,
              darkColor: token.variants.dark?.color,
              lightColor: token.variants.light?.color,
              bold: (style & FONT_STYLE_BOLD) !== 0 || undefined,
              italic: (style & FONT_STYLE_ITALIC) !== 0 || undefined,
            };
          }),
        ),
        requestedLanguage: cleanRequest,
      };
    } catch {
      result = {
        highlighted: false,
        language: "text",
        lines: plainLines(code),
        requestedLanguage: cleanRequest,
      };
    }
  }

  if (code.length <= MAX_CACHEABLE_CHARS) {
    highlightCache.set(cacheKey, result);
    if (highlightCache.size > MAX_CACHE_ENTRIES) {
      const oldest = highlightCache.keys().next().value;
      if (oldest !== undefined) highlightCache.delete(oldest);
    }
  }
  return result;
}

function getHighlighter(): MarkdownHighlighter {
  highlighter ??= createHighlighterCoreSync({
    engine: createJavaScriptRegexEngine({ target: "ES2018" }),
    langs: [
      c,
      css,
      diff,
      html,
      javascript,
      json,
      markdown,
      python,
      rust,
      shellscript,
      sql,
      toml,
      typescript,
      yaml,
    ],
    themes: [githubLight, githubDark, githubLightHighContrast, EINK_MONO_THEME],
  });
  return highlighter;
}

export function extractMarkdownCodeLanguage(className: string | undefined): string | null {
  const match = /(?:^|\s)language-([^\s]+)/.exec(className ?? "");
  return normalizeLanguageLabel(match?.[1] ?? null);
}

function normalizeLanguageLabel(value: string | null): string | null {
  if (!value) return null;
  const normalized = value.trim().toLowerCase().slice(0, 32);
  return /^[a-z0-9_+#.-]+$/.test(normalized) ? normalized : null;
}

function plainLines(code: string): MarkdownCodeToken[][] {
  return [[{ content: code }]];
}

function isHighlightableSource(code: string): boolean {
  if (code.length > MAX_HIGHLIGHT_CHARS) return false;
  let lineCount = 1;
  let lineLength = 0;
  for (let index = 0; index < code.length; index += 1) {
    if (code.charCodeAt(index) === 0x0a) {
      lineCount += 1;
      lineLength = 0;
      if (lineCount > MAX_HIGHLIGHT_LINES) return false;
    } else {
      lineLength += 1;
      if (lineLength > MAX_HIGHLIGHT_LINE_CHARS) return false;
    }
  }
  return true;
}

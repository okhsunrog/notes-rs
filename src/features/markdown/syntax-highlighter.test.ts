import { describe, expect, it } from "vite-plus/test";
import {
  extractMarkdownCodeLanguage,
  highlightMarkdownCode,
  resolveCodeTheme,
} from "./syntax-highlighter";

function flattenedCode(result: ReturnType<typeof highlightMarkdownCode>): string {
  return result.lines.map((line) => line.map((token) => token.content).join("")).join("\n");
}

describe("Markdown syntax highlighter", () => {
  it("uses a bundled grammar and preserves source text exactly", () => {
    const source = 'fn main() {\n    println!("hello");\n}';
    const result = highlightMarkdownCode(source, "rs");

    expect(result.highlighted).toBe(true);
    expect(result.language).toBe("rust");
    expect(flattenedCode(result)).toBe(source);
  });

  it("falls back to inert plain text for unknown languages", () => {
    const source = "<img src=x onerror=alert(1)>";
    const result = highlightMarkdownCode(source, "made-up-language");

    expect(result.highlighted).toBe(false);
    expect(result.language).toBe("text");
    expect(result.requestedLanguage).toBe("made-up-language");
    expect(flattenedCode(result)).toBe(source);
    expect(result.lines).toHaveLength(1);
  });

  it("falls back before large or deeply multiline input reaches the synchronous tokenizer", () => {
    const tooManyLines = "let value = 1;\n".repeat(201);
    const veryLongLine = "x".repeat(4_097);

    expect(highlightMarkdownCode(tooManyLines, "typescript")).toMatchObject({
      highlighted: false,
      language: "text",
    });
    expect(flattenedCode(highlightMarkdownCode(tooManyLines, "typescript"))).toBe(tooManyLines);
    expect(highlightMarkdownCode(veryLongLine, "rust")).toMatchObject({
      highlighted: false,
      language: "text",
    });
    expect(flattenedCode(highlightMarkdownCode(veryLongLine, "rust"))).toBe(veryLongLine);
  });

  it("picks a theme from the resolved display and ink color", () => {
    expect(resolveCodeTheme("standard", "color")).toBe("standard");
    expect(resolveCodeTheme("standard", "mono")).toBe("standard");
    expect(resolveCodeTheme("eink", "color")).toBe("eink");
    expect(resolveCodeTheme("eink", "mono")).toBe("eink-mono");
  });

  it("colors e-ink code with high contrast and monochrome code with weight and slant", () => {
    const source = 'fn main() {\n    // hi\n    println!("hello");\n}';
    const flatten = (result: ReturnType<typeof highlightMarkdownCode>) =>
      result.lines
        .flat()
        .map((token) => token.content)
        .join("");

    const color = highlightMarkdownCode(source, "rs", "eink");
    expect(color.highlighted).toBe(true);
    expect(color.lines.flat().some((token) => token.lightColor !== undefined)).toBe(true);
    expect(color.lines.flat().every((token) => !token.bold && !token.italic)).toBe(true);

    const mono = highlightMarkdownCode(source, "rs", "eink-mono");
    expect(mono.highlighted).toBe(true);
    expect(flatten(mono)).toBe(source.split("\n").join(""));
    // Only black, the comment gray and nothing else.
    const colors = new Set(mono.lines.flat().map((token) => token.lightColor?.toLowerCase()));
    expect([...colors].every((value) => value === "#000000" || value === "#555555")).toBe(true);
    expect(mono.lines.flat().some((token) => token.bold)).toBe(true);
    expect(mono.lines.flat().some((token) => token.italic)).toBe(true);
  });

  it("extracts only bounded language labels", () => {
    expect(extractMarkdownCodeLanguage("language-typescript extra")).toBe("typescript");
    expect(extractMarkdownCodeLanguage("other")).toBeNull();
    expect(extractMarkdownCodeLanguage("language-rust<script>")).toBeNull();
  });
});

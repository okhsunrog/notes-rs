import { describe, expect, it } from "vite-plus/test";
import { extractMarkdownCodeLanguage, highlightMarkdownCode } from "./syntax-highlighter";

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

  it("extracts only bounded language labels", () => {
    expect(extractMarkdownCodeLanguage("language-typescript extra")).toBe("typescript");
    expect(extractMarkdownCodeLanguage("other")).toBeNull();
    expect(extractMarkdownCodeLanguage("language-rust<script>")).toBeNull();
  });
});

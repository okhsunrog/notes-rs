// @vitest-environment jsdom

import { beforeAll, describe, expect, it } from "vite-plus/test";
import { renderMermaid } from "./mermaid-runtime";

beforeAll(() => {
  Object.defineProperty(SVGElement.prototype, "getBBox", {
    configurable: true,
    value: () => ({ height: 20, width: 80, x: 0, y: 0 }),
  });
  Object.defineProperty(SVGElement.prototype, "getComputedTextLength", {
    configurable: true,
    value: () => 80,
  });
});

describe("Mermaid lazy runtime", () => {
  it("turns malformed source into a cached recoverable error", async () => {
    const first = await renderMermaid(
      "malformed-a",
      "this is not a mermaid diagram",
      "light",
      new AbortController().signal,
    );
    const second = await renderMermaid(
      "malformed-b",
      "this is not a mermaid diagram",
      "light",
      new AbortController().signal,
    );

    expect(first).toEqual({ kind: "error", message: "Diagram source is invalid." });
    expect(second).toEqual(first);
  });

  it("renders a valid diagram to sanitized inert SVG", async () => {
    const result = await renderMermaid(
      "valid-flowchart",
      "flowchart LR\nA[Safe] --> B[Diagram]",
      "dark",
      new AbortController().signal,
    );

    expect(result.kind).toBe("success");
    if (result.kind !== "success") return;
    expect(result.svg).toContain("<svg");
    expect(result.svg).toContain("Safe");
    expect(result.svg).not.toMatch(/<script|<foreignObject|\son[a-z]+=|javascript:/i);
  });

  it("strips links, HTML labels and external image attempts from generated output", async () => {
    const result = await renderMermaid(
      "hostile-flowchart",
      'flowchart LR\nA["<img src=https://tracker.example/pixel onerror=alert(1)>unsafe"] --> B\nclick A "javascript:alert(2)"',
      "light",
      new AbortController().signal,
    );

    expect(result.kind).toBe("success");
    if (result.kind !== "success") return;
    expect(result.svg).not.toMatch(
      /<script|<foreignObject|<image|<img|\son[a-z]+=|javascript:|href=/i,
    );
    expect(result.svg).toContain("&lt;img");
  });
});

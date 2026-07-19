import katex from "katex";
import { describe, expect, it } from "vite-plus/test";

function render(expression: string): string {
  return katex.renderToString(expression, {
    maxExpand: 1_000,
    maxSize: 50,
    output: "htmlAndMathml",
    strict: "ignore",
    trust: false,
  });
}

describe("KaTeX untrusted output boundary", () => {
  it("keeps href commands inert", () => {
    const html = render(String.raw`\href{https://tracker.example/secret}{click}`);

    expect(html).toContain("katex");
    expect(html).not.toMatch(/<a\b|href=/i);
  });

  it("keeps includegraphics commands inert", () => {
    const html = render(String.raw`\includegraphics{https://tracker.example/pixel.png}`);

    expect(html).toContain("katex");
    expect(html).not.toMatch(/<img\b|src=/i);
  });

  it("keeps htmlData commands inert", () => {
    const html = render(String.raw`\htmlData{onclick=alert(1) data-secret=owned}{x}`);

    expect(html).toContain("katex");
    expect(html).not.toMatch(/(?:data-secret|onclick)=["']/i);
  });
});

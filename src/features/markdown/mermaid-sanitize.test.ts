// @vitest-environment jsdom

import createDOMPurify from "dompurify";
import { describe, expect, it } from "vite-plus/test";
import { sanitizeMermaidSvg } from "./mermaid-policy";

const purifier = createDOMPurify(window);

function sanitize(body: string): string {
  return sanitizeMermaidSvg(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 40">${body}</svg>`,
    purifier,
  );
}

describe("Mermaid SVG image boundary", () => {
  it("preserves inert SVG and local marker references", () => {
    const svg = sanitize(
      '<defs><marker id="arrow"><path d="M0,0 L10,5 L0,10 z"/></marker></defs><path d="M0 0L100 40" marker-end="url(#arrow)"/><text x="5" y="20">Safe</text>',
    );

    expect(svg).toContain("<svg");
    expect(svg).toContain("url(#arrow)");
    expect(svg).toContain(">Safe</text>");
  });

  it("removes scripts, event handlers and navigable links", () => {
    const svg = sanitize(
      '<script>alert(1)</script><a href="javascript:alert(2)"><text>click</text></a><rect onload="alert(3)" width="10" height="10"/>',
    );

    expect(svg).not.toMatch(/script|javascript|onload|href=/i);
  });

  it("removes HTML labels and remote image elements", () => {
    const svg = sanitize(
      '<foreignObject><div xmlns="http://www.w3.org/1999/xhtml"><img src="https://tracker.example/pixel"/></div></foreignObject><image href="file:///etc/passwd"/>',
    );

    expect(svg).not.toMatch(/foreignObject|<image|<img|tracker|file:/i);
  });

  it("removes animation and filter primitives that can create late subresource loads", () => {
    const svg = sanitize(
      '<rect id="target"/><set href="#target" attributeName="fill" to="url(//tracker.example/pixel)"/><feImage href="//tracker.example/pixel"/>',
    );

    expect(svg).not.toMatch(/<set|<feImage|tracker\.example/i);
  });

  it("rejects external URLs in SVG attributes and styles", () => {
    expect(() => sanitize('<rect fill="url(https://tracker.example/pixel)"/>')).toThrow(
      /external|network/i,
    );
    expect(() => sanitize('<style>@import "https://tracker.example/style.css";</style>')).toThrow(
      /external|network/i,
    );
    expect(() => sanitize('<rect style="fill:url(data:image/svg+xml,bad)"/>')).toThrow(
      /external|network/i,
    );
  });
});

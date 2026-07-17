import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import type { MarkdownImageResolver } from "./image-policy";
import { MarkdownRenderer } from "./markdown-renderer";

const CONTEXT = {
  kind: "note",
  presentation: "reading",
  pageUuid: "page-a",
} as const;
const ATTACHMENT_UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";

function render(markdown: string, resolveImage?: MarkdownImageResolver): string {
  return renderToStaticMarkup(
    <MarkdownRenderer
      context={CONTEXT}
      markdown={markdown}
      onOpenLink={() => undefined}
      resolveImage={resolveImage}
    />,
  );
}

describe("MarkdownRenderer", () => {
  it("renders CommonMark and GFM as semantic HTML", () => {
    const html = render(
      "# Heading\n\n~~old~~ and **new**\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n- [x] done",
    );

    expect(html).toContain("<h1>Heading</h1>");
    expect(html).toContain("<del>old</del>");
    expect(html).toContain("<strong>new</strong>");
    expect(html).toContain('data-markdown-table-scroll="true"');
    expect(html).toContain('aria-label="Scrollable table"');
    expect(html).toContain('role="region"');
    expect(html).toContain('tabindex="0"');
    expect(html).toContain("<table>");
    expect(html).toContain("<th>A</th>");
    expect(html).toContain("<td>1</td>");
    expect(html).toContain('type="checkbox"');
    expect(html).toContain("checked");
  });

  it("renders wikilinks and block references as typed internal links", () => {
    const html = render("Open [[Project Aurora]] and ((019c8d1a-4ab1-7f31-8f00-f594337c3ca5)).");

    expect(html).toContain('data-markdown-link="page"');
    expect(html).toContain('href="notes-page:Project%20Aurora"');
    expect(html).toContain('data-markdown-link="block"');
    expect(html).toContain('href="notes-block:019c8d1a-4ab1-7f31-8f00-f594337c3ca5"');
  });

  it("does not interpret notes syntax inside inline or fenced code", () => {
    const html = render(
      "[[Outside]] `[[Inline code]]`\n\n```text\n[[Fenced code]]\n((019c8d1a-4ab1-7f31-8f00-f594337c3ca5))\n```",
    );

    expect(html.match(/data-markdown-link="page"/g)).toHaveLength(1);
    expect(html).toContain("<code>[[Inline code]]</code>");
    expect(html).toContain("[[Fenced code]]");
    expect(html).not.toContain('data-markdown-link="block"');
  });

  it("drops raw HTML and renders unsafe links as inert text", () => {
    const html = render(
      '<script>alert("boom")</script><svg><a xlink:href="javascript:alert(2)">x</a></svg>\n\n[unsafe](javascript:alert(1)) [encoded](jav&#x61;script:alert(2)) [safe](https://example.com)',
    );

    expect(html).not.toContain("<script");
    expect(html).not.toContain("<svg");
    expect(html).not.toContain("alert(&quot;boom&quot;)");
    expect(html).not.toContain("javascript:");
    expect(html.match(/data-markdown-link="blocked"/g)).toHaveLength(2);
    expect(html).toContain('data-markdown-link="external"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it("blocks remote and malicious image sources without loading them", () => {
    const html = render(
      "![remote](https://tracker.example/pixel.png) ![svg](data:image/svg+xml,%3Csvg%20onload=alert(1)%3E) ![file](file:///etc/passwd)",
    );

    expect(html).not.toContain("<img");
    expect(html).not.toContain("tracker.example");
    expect(html).not.toContain("file://");
    expect(html).not.toContain("onload");
    expect(html).toContain('data-markdown-image="remote-blocked"');
    expect(html).toContain('data-markdown-image="blocked"');
  });

  it("renders only a validated local attachment returned by the typed resolver", () => {
    let resolvedUuid = "";
    const resolveImage: MarkdownImageResolver = (request) => {
      resolvedUuid = request.attachmentUuid;
      return {
        byteSize: 24_000,
        height: 480,
        mime: "image/png",
        src: "blob:https://tauri.localhost/local-image",
        width: 640,
      };
    };
    const html = render(
      `![diagram](notes-attachment:${ATTACHMENT_UUID} "Architecture diagram")`,
      resolveImage,
    );

    expect(resolvedUuid).toBe(ATTACHMENT_UUID);
    expect(html).toContain("<img");
    expect(html).toContain('src="blob:https://tauri.localhost/local-image"');
    expect(html).toContain('loading="lazy"');
    expect(html).toContain('decoding="async"');
    expect(html).toContain('width="640"');
    expect(html).toContain('height="480"');
    expect(html).toContain('<span class="markdown-image-caption">Architecture diagram</span>');
    expect(html).not.toContain("<figure");
    expect(html).not.toContain("<p><div");
    expect(html).toContain('<p><span class="markdown-image" role="group">');
  });

  it("rejects an unsanitized SVG even when returned by the trusted resolver", () => {
    const html = render(`![diagram](notes-attachment:${ATTACHMENT_UUID})`, () => ({
      byteSize: 512,
      height: 480,
      mime: "image/svg+xml",
      src: "asset://localhost/diagram.svg",
      width: 640,
    }));

    expect(html).not.toContain("<img");
    expect(html).toContain('data-markdown-image="blocked"');
  });

  it("renders KaTeX math without allowing trusted commands", () => {
    const html = render(
      String.raw`Euler: $e^{i\pi}+1=0$. $\href{javascript:alert(1)}{click}$ $\includegraphics{https://tracker.example/pixel.png}$`,
    );

    expect(html).toContain('class="katex"');
    expect(html).toContain("<math");
    expect(html).not.toContain('href="javascript:');
    expect(html).not.toContain("<img");
  });

  it("keeps KaTeX trust extensions and HTML-shaped text inert", () => {
    const html = render(
      String.raw`$\htmlClass{owned}{x}$ $\htmlData{onclick=alert(1)}{x}$ $\htmlStyle{background:url(https://tracker.example/pixel)}{x}$ $\url{javascript:alert(1)}$ $\text{<img src=x onerror=alert(1)>}$`,
    );

    expect(html).not.toContain('class="owned"');
    expect(html).not.toMatch(/href="javascript:/i);
    expect(html).not.toMatch(/src="https:\/\/tracker\.example/i);
    expect(html).not.toMatch(/style="[^"]*url\(/i);
    expect(html).not.toContain("<img");
  });

  it("does not let raw HTML opt into the trusted post-sanitize KaTeX transform", () => {
    const html = render(String.raw`<code class="math-inline">x^2</code>`);

    expect(html).not.toContain('class="katex"');
    expect(html).not.toContain("math-inline");
    expect(html).not.toContain("<code");
    expect(html).toContain("x^2");
  });

  it("keeps inline note previews semantic without block wrappers", () => {
    const html = renderToStaticMarkup(
      <MarkdownRenderer
        context={CONTEXT}
        markdown="Inline $x^2$ and `code`"
        mode="inline"
        onOpenLink={() => undefined}
      />,
    );

    expect(html).toContain('class="markdown-renderer"');
    expect(html).toContain('class="katex"');
    expect(html).toContain("<code>code</code>");
    expect(html).not.toContain("<p>");
    expect(html).not.toContain("markdown-code-block");
  });

  it("renders fenced source as inert inline code in an inline preview", () => {
    const html = renderToStaticMarkup(
      <MarkdownRenderer
        context={CONTEXT}
        markdown={"```rust\nfn main() {}\n```"}
        mode="inline"
        onOpenLink={() => undefined}
      />,
    );

    expect(html).toContain('<code class="language-rust">fn main() {}');
    expect(html).not.toContain("<pre");
    expect(html).not.toContain("<div");
    expect(html).not.toContain("markdown-code-block");
  });

  it("falls back to visible code before an oversized expression reaches KaTeX", () => {
    const source = "x".repeat(8_193);
    const html = render(`$${source}$`);

    expect(html).not.toContain('class="katex"');
    expect(html).toContain(`<code>${source}</code>`);
  });

  it("applies the same KaTeX bound to fenced math", () => {
    const source = "x".repeat(8_193);
    const html = render(`\`\`\`math\n${source}\n\`\`\``);

    expect(html).not.toContain('class="katex"');
    expect(html).toContain('data-code-highlighted="false"');
    expect(html).toContain(source);
  });

  it("highlights bundled code languages and safely falls back for unknown languages", () => {
    const known = render('```rust\nfn main() { println!("hello"); }\n```');
    const unknown = render("```made-up-language\n<img src=x onerror=alert(1)>\n```");

    expect(known).toContain('data-code-language="rust"');
    expect(known).toContain('data-code-highlighted="true"');
    expect(known).toContain("markdown-code-token");
    expect(known).toContain('aria-label="Copy code"');
    expect(unknown).toContain('data-code-language="text"');
    expect(unknown).toContain('data-code-highlighted="false"');
    expect(unknown).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(unknown).not.toContain("<img");
  });
});

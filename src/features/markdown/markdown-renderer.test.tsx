import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import type { MarkdownImageResolver, MarkdownResolvedImage } from "./image-policy";
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

  it("uses raw source escapes without losing a later valid notes link", () => {
    const html = render(String.raw`\[[Escaped]] and [[Valid]]`);

    expect(html.match(/data-markdown-link="page"/g)).toHaveLength(1);
    expect(html).toContain("[[Escaped]] and");
    expect(html).toContain('href="notes-page:Valid"');
    expect(html).not.toContain('href="notes-page:Escaped"');
  });

  it("keeps closed nested notes syntax visible and inert", () => {
    const html = render(`[[outer [[inner]] tail]] and [[cross ((${ATTACHMENT_UUID})) tail]]`);

    expect(html).not.toContain('data-markdown-link="page"');
    expect(html).not.toContain('data-markdown-link="block"');
    expect(html).toContain("[[outer [[inner]] tail]]");
    expect(html).toContain(`[[cross ((${ATTACHMENT_UUID})) tail]]`);
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

  it("renders Logseq video macros as privacy-safe external link cards", () => {
    const html = render("Before {{video https://video.example/watch?private=token-value}} after");

    expect(html).toContain('class="markdown-video-card"');
    expect(html).toContain('aria-label="Open video from video.example"');
    expect(html).toContain('data-markdown-link="external"');
    expect(html).toContain('href="https://video.example/watch?private=token-value"');
    expect(html).toContain("video.example");
    expect(html).not.toContain(">token-value<");
    expect(html).not.toContain("<iframe");
    expect(html).not.toContain("<video");
    expect(html).not.toContain("<img");
    expect(html).not.toContain("autoplay");
  });

  it("accepts standard and legacy Logseq Markdown-link video wrappers", () => {
    const html = render(
      "{{video [Watch](https://video.example/standard)}}\n\n{{video [Watch](https://video.example/legacy}}",
    );

    expect(html.match(/class="markdown-video-card"/g)).toHaveLength(2);
    expect(html).toContain('href="https://video.example/standard"');
    expect(html).toContain('href="https://video.example/legacy"');
    expect(html).not.toContain("{{video");
  });

  it("keeps unsupported and malformed video macros visible and inert", () => {
    const html = render(
      "{{video javascript:alert(1)}} {{video [Watch](file:///etc/passwd)}} {{video relative.mov}}",
    );

    expect(html.match(/<code>/g)).toHaveLength(3);
    expect(html).toContain("{{video javascript:alert(1)}}");
    expect(html).toContain("{{video [Watch](file:///etc/passwd)}}");
    expect(html).toContain("{{video relative.mov}}");
    expect(html).not.toContain("href=");
    expect(html).not.toContain("<iframe");
    expect(html).not.toContain("<video");
  });

  it("does not interpret Logseq video macros inside code", () => {
    const html = render(
      "`{{video https://video.example/inline}}`\n\n```text\n{{video https://video.example/fenced}}\n```",
    );

    expect(html).not.toContain("markdown-video-card");
    expect(html).toContain("{{video https://video.example/inline}}");
    expect(html).toContain("{{video https://video.example/fenced}}");
    expect(html).not.toContain('href="https://video.example');
  });

  it("does not create a nested video link inside an ordinary Markdown link", () => {
    const html = render("[{{video https://video.example/nested}}](https://example.com)");

    expect(html).not.toContain("markdown-video-card");
    expect(html.match(/<a /g)).toHaveLength(1);
    expect(html).toContain('href="https://example.com/"');
    expect(html).toContain("{{video https://video.example/nested}}");
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
        src: `notes-attachment://localhost/${ATTACHMENT_UUID}`,
        width: 640,
      };
    };
    const html = render(
      `![diagram](notes-attachment:${ATTACHMENT_UUID} "Architecture diagram")`,
      resolveImage,
    );

    expect(resolvedUuid).toBe(ATTACHMENT_UUID);
    expect(html).toContain("<img");
    expect(html).toContain(`src="notes-attachment://localhost/${ATTACHMENT_UUID}"`);
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
    const html = render(
      `![diagram](notes-attachment:${ATTACHMENT_UUID})`,
      () =>
        ({
          byteSize: 512,
          height: 480,
          mime: "image/svg+xml",
          src: `notes-attachment://localhost/${ATTACHMENT_UUID}`,
          width: 640,
        }) as unknown as MarkdownResolvedImage,
    );

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
    expect(html).toContain('data-markdown-mode="inline"');
    expect(html).toContain('class="katex"');
    expect(html).toContain("<code>code</code>");
    expect(html).not.toContain("<p>");
    expect(html).not.toContain("markdown-code-block");
  });

  it("keeps full Markdown semantics in compact flow mode", () => {
    const html = renderToStaticMarkup(
      <MarkdownRenderer
        context={CONTEXT}
        markdown={
          "# Heading\n\n- parent\n  - child\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n$$\nx^2\n$$"
        }
        mode="compact_flow"
        onOpenLink={() => undefined}
      />,
    );

    expect(html).toContain('data-markdown-mode="compact_flow"');
    expect(html).toContain("note-prose--compact-flow");
    expect(html).toContain("<h1>Heading</h1>");
    expect(html.match(/<ul>/g)).toHaveLength(2);
    expect(html).toContain('data-markdown-table-scroll="true"');
    expect(html).toContain("markdown-code-block");
    expect(html).toContain('class="katex-display"');
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

  it("recognizes only fenced Mermaid and keeps its source visible while loading", () => {
    const mermaid = render("```mermaid\nflowchart LR\nA --> B\n```");
    const ordinary = render("```text\nflowchart LR\nA --> B\n```");

    expect(mermaid).toContain('data-mermaid-state="loading"');
    expect(mermaid).toContain('role="status"');
    expect(mermaid).toContain("Rendering Mermaid diagram");
    expect(mermaid).toContain("flowchart LR");
    expect(ordinary).not.toContain("data-mermaid-state");
    expect(ordinary).toContain('data-code-language="text"');
  });

  it("falls back to accessible visible source for an oversized Mermaid fence", () => {
    const source = "x".repeat(16_001);
    const html = render(`\`\`\`mermaid\n${source}\n\`\`\``);

    expect(html).toContain('data-mermaid-state="oversize"');
    expect(html).toContain('role="alert"');
    expect(html).toContain("too large to render safely");
    expect(html).toContain(source);
  });

  it("does not turn Mermaid-looking inline or unlabelled code into a diagram", () => {
    const html = render("`flowchart LR`\n\n```\nflowchart LR\nA --> B\n```");

    expect(html).not.toContain("data-mermaid-state");
    expect(html).toContain("flowchart LR");
  });
});

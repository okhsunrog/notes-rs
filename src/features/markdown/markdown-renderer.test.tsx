import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import { MarkdownRenderer } from "./markdown-renderer";

const CONTEXT = {
  kind: "note",
  presentation: "reading",
  pageUuid: "page-a",
} as const;

function render(markdown: string): string {
  return renderToStaticMarkup(
    <MarkdownRenderer context={CONTEXT} markdown={markdown} onOpenLink={() => undefined} />,
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
    expect(html).toContain("<table>");
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
      '<script>alert("boom")</script>\n\n[unsafe](javascript:alert(1)) [safe](https://example.com)',
    );

    expect(html).not.toContain("<script");
    expect(html).not.toContain("alert(&quot;boom&quot;)");
    expect(html).not.toContain("javascript:");
    expect(html).toContain('data-markdown-link="blocked"');
    expect(html).toContain('data-markdown-link="external"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it("never loads Markdown images before the attachment resolver exists", () => {
    const html = render("![diagram](https://tracker.example/pixel.png)");

    expect(html).not.toContain("<img");
    expect(html).not.toContain("tracker.example");
    expect(html).toContain('data-markdown-image="unavailable"');
    expect(html).toContain("[Image: diagram]");
  });
});

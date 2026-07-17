import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import type { Block, BlockStyle } from "@/lib/api";
import { RenderedBlock } from "./rendered-block";

function block(markdown: string, style: BlockStyle = "paragraph"): Block {
  return {
    uuid: "019c8d1a-4ab1-7f31-8f00-f594337c3ca5",
    pageUuid: "019c8d1a-4ab1-7f31-8f00-f594337c3ca6",
    parentUuid: null,
    orderKey: "a0",
    style,
    markdown,
    createdAt: 0,
    updatedAt: 0,
  };
}

function render(markdown: string, style: BlockStyle = "paragraph", readOnly = false) {
  return renderToStaticMarkup(
    <RenderedBlock
      block={block(markdown, style)}
      layout="document"
      onOpenLink={() => undefined}
      ordinal={3}
      readOnly={readOnly}
    />,
  );
}

describe("RenderedBlock Markdown integration", () => {
  it("uses the shared flow renderer for paragraph blocks", () => {
    const html = render("**Connected** to [[Project Aurora]].\n\n| A | B |\n| - | - |\n| 1 | 2 |");

    expect(html).toContain('data-markdown-context="note"');
    expect(html).toContain("<strong>Connected</strong>");
    expect(html).toContain('data-markdown-link="page"');
    expect(html).toContain('data-markdown-table-scroll="true"');
  });

  it("uses inline Markdown inside typed heading chrome", () => {
    const html = render("## **Architecture**", "heading_1", true);

    expect(html).toContain("<h2");
    expect(html).toContain("<strong>Architecture</strong>");
    expect(html).toContain('data-markdown-context="note"');
    expect(html).not.toContain("<p>");
  });

  it("keeps raw code blocks literal", () => {
    const html = render("[[Not a link]]\n<script>not executable</script>", "code");

    expect(html).toContain("<pre");
    expect(html).toContain("[[Not a link]]");
    expect(html).toContain("&lt;script&gt;not executable&lt;/script&gt;");
    expect(html).not.toContain("data-markdown-link");
  });

  it("preserves numbered document block chrome", () => {
    const html = render("item with *emphasis*", "numbered");

    expect(html).toContain(">3.</span>");
    expect(html).toContain("<em>emphasis</em>");
  });
});

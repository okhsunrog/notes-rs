import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Block, BlockStyle } from "@/lib/api";
import { RenderedBlock } from "./rendered-block";

function block(markdown: string, style: BlockStyle = { kind: "paragraph" }): Block {
  return {
    uuid: "019c8d1a-4ab1-7f31-8f00-f594337c3ca5",
    pageUuid: "019c8d1a-4ab1-7f31-8f00-f594337c3ca6",
    parentUuid: null,
    orderKey: "a0",
    style,
    markdown,
    markdownRevision: "test-markdown-revision",
    createdAt: 0,
    updatedAt: 0,
  };
}

function render(
  markdown: string,
  style: BlockStyle = { kind: "paragraph" },
  readOnly = false,
  layout: "document" | "outline" = "document",
) {
  const queryClient = new QueryClient();
  return renderToStaticMarkup(
    <QueryClientProvider client={queryClient}>
      <RenderedBlock
        block={block(markdown, style)}
        layout={layout}
        onOpenLink={() => undefined}
        ordinal={3}
        readOnly={readOnly}
        taskBusy={false}
        onTaskStateChange={() => undefined}
      />
    </QueryClientProvider>,
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
    const html = render("## **Architecture**", { kind: "heading_1" }, true);

    expect(html).toContain("<h2");
    expect(html).toContain("<strong>Architecture</strong>");
    expect(html).toContain('data-markdown-context="note"');
    expect(html).toContain('data-markdown-mode="inline"');
    expect(html).not.toContain("<p>");
  });

  it("keeps raw code blocks literal", () => {
    const html = render("[[Not a link]]\n<script>not executable</script>", { kind: "code" });

    expect(html).toContain("<pre");
    expect(html).toContain("[[Not a link]]");
    expect(html).toContain("&lt;script&gt;not executable&lt;/script&gt;");
    expect(html).not.toContain("data-markdown-link");
  });

  it("preserves numbered document block chrome", () => {
    const html = render("item with *emphasis*", { kind: "numbered" });

    expect(html).toContain(">3.</span>");
    expect(html).toContain("<em>emphasis</em>");
    expect(html).toContain('data-markdown-mode="compact_flow"');
  });

  it("preserves imported Markdown inside a bullet as compact semantic flow", () => {
    const html = render(
      "# Imported section\n\n- parent\n  - child\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n$$\nx^2\n$$",
      { kind: "bullet" },
    );

    expect(html).toContain('data-markdown-mode="compact_flow"');
    expect(html).toContain("<h1>Imported section</h1>");
    expect(html.match(/<ul>/g)).toHaveLength(2);
    expect(html).toContain('data-markdown-table-scroll="true"');
    expect(html).toContain("markdown-code-block");
    expect(html).toContain('class="katex-display"');
    expect(html).toContain('<div class="min-w-0 flex-1 break-words"><div');
    expect(html).not.toContain('<span class="min-w-0 whitespace-pre-wrap break-words">');
  });

  it("uses compact semantic flow for an outline bullet without invalid paragraph nesting", () => {
    const html = render("Paragraph\n\n```text\ncode\n```", { kind: "bullet" }, false, "outline");

    expect(html).toContain('data-markdown-mode="compact_flow"');
    expect(html).toContain("markdown-code-block");
    expect(html).not.toContain("<p><div");
  });

  it("renders a real accessible task action with its typed state", () => {
    const html = render("Ship the release", { kind: "task", state: "now" });

    expect(html).toContain('role="checkbox"');
    expect(html).toContain('aria-checked="false"');
    expect(html).toContain("Now task; change to Done");
    expect(html).toContain('data-markdown-mode="compact_flow"');
    expect(html).not.toContain('disabled=""');
  });

  it("renders terminal task states distinctly", () => {
    const done = render("Shipped", { kind: "task", state: "done" });
    const cancelled = render("Dropped", { kind: "task", state: "cancelled" });

    expect(done).toContain('aria-checked="true"');
    expect(cancelled).toContain('aria-checked="mixed"');
  });

  it("keeps reading presentation semantic and non-mutating", () => {
    const html = render("Ship the release", { kind: "task", state: "waiting" }, true);

    expect(html).toContain('role="img"');
    expect(html).toContain('aria-label="Waiting task"');
    expect(html).not.toContain('role="checkbox"');
  });

  it("keeps task chrome for an empty task block", () => {
    const html = render("", { kind: "task", state: "todo" });

    expect(html).toContain('role="checkbox"');
    expect(html).toContain("Start writing…");
  });
});

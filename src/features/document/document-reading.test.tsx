import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import { DocumentReadingSurface } from "./document-page";

function render(markdown: string) {
  return renderToStaticMarkup(
    <QueryClientProvider client={new QueryClient()}>
      <DocumentReadingSurface pageUuid="page" markdown={markdown} />
    </QueryClientProvider>,
  );
}

describe("DocumentReadingSurface", () => {
  it("renders the continuous buffer through the shared Markdown renderer", () => {
    const html = render("# Heading\n\nParagraph with **strong** text.\n\n- item");

    expect(html).toContain('class="markdown-renderer note-prose document-reading-surface"');
    expect(html).toContain("<h1>Heading</h1>");
    expect(html).toContain("<strong>strong</strong>");
    expect(html).toContain("<li>item</li>");
    expect(html).not.toContain("cm-editor");
  });

  it("shows an explicit empty Reading state", () => {
    expect(render(" ")).toContain("This document is empty.");
  });
});

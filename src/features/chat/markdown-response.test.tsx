import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import MarkdownResponse from "./markdown-response";

describe("assistant Markdown integration", () => {
  it("uses the shared notes dialect and safe link policy", () => {
    const queryClient = new QueryClient();
    const html = renderToStaticMarkup(
      <QueryClientProvider client={queryClient}>
        <MarkdownResponse onOpenLink={() => undefined}>
          {"## Result\n\nSee [[Project Aurora]], ~~old~~, and [unsafe](javascript:alert(1))."}
        </MarkdownResponse>
      </QueryClientProvider>,
    );

    expect(html).toContain('data-markdown-context="assistant"');
    expect(html).toContain("<h2>Result</h2>");
    expect(html).toContain('data-markdown-link="page"');
    expect(html).toContain("<del>old</del>");
    expect(html).toContain('data-markdown-link="blocked"');
    expect(html).not.toContain("javascript:");
  });
});

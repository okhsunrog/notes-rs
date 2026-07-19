// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { MarkdownRenderer } from "./markdown-renderer";
import { cachedMathRuntime } from "./math-runtime";

const roots: Array<ReturnType<typeof createRoot>> = [];

afterEach(() => {
  for (const root of roots.splice(0)) act(() => root.unmount());
});

describe("math lazy runtime", () => {
  it("does not load for ordinary Markdown and renders math after the async chunk arrives", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);
    roots.push(root);
    const render = (markdown: string) => (
      <MarkdownRenderer
        context={{ kind: "assistant" }}
        markdown={markdown}
        onOpenLink={() => undefined}
      />
    );

    expect(cachedMathRuntime()).toBeNull();
    await act(async () => root.render(render("Ordinary **Markdown** without formulas.")));
    await new Promise((resolve) => window.setTimeout(resolve, 0));
    expect(cachedMathRuntime()).toBeNull();
    expect(container.innerHTML).toContain("<strong>Markdown</strong>");

    await act(async () => root.render(render(String.raw`Euler: $e^{i\pi}+1=0$.`)));
    await vi.waitFor(() => {
      expect(cachedMathRuntime()).not.toBeNull();
      expect(container.querySelector(".katex")).not.toBeNull();
    });
  });
});

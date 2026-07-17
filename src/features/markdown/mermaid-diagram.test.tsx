// @vitest-environment jsdom

import { ThemeProvider } from "next-themes";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vite-plus/test";
import { MermaidDiagram } from "./mermaid-diagram";

const mounted: Array<() => void> = [];

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: () => ({
      addEventListener: () => undefined,
      addListener: () => undefined,
      matches: false,
      removeEventListener: () => undefined,
      removeListener: () => undefined,
    }),
  });
});

afterEach(() => {
  for (const unmount of mounted.splice(0)) unmount();
});

describe("MermaidDiagram", () => {
  it("keeps malformed source visible after asynchronous rendering fails", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    mounted.push(() => {
      act(() => root.unmount());
      container.remove();
    });

    await act(async () => {
      root.render(
        <ThemeProvider attribute="class" forcedTheme="light">
          <MermaidDiagram source="definitely not valid Mermaid source" />
        </ThemeProvider>,
      );
    });
    await act(async () => new Promise((resolve) => window.setTimeout(resolve, 1_000)));

    expect(container.querySelector('[data-mermaid-state="error"]')).not.toBeNull();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Diagram source is invalid.",
    );
    expect(container.querySelector("code")?.textContent).toBe(
      "definitely not valid Mermaid source",
    );
  });
});

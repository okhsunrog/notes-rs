// @vitest-environment jsdom

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import { TangleafMark } from "./TangleafMark";

describe("Tangleaf mark", () => {
  it("keeps gradient references unique across multiple instances", () => {
    const container = document.createElement("div");
    container.innerHTML = renderToStaticMarkup(
      <>
        <TangleafMark />
        <TangleafMark title="Tangleaf" />
      </>,
    );
    const ids = [...container.querySelectorAll("[id]")].map((element) => element.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const svg of container.querySelectorAll("svg")) {
      const localIds = [...svg.querySelectorAll("[id]")].map((element) => element.id);
      for (const path of svg.querySelectorAll('path[fill^="url("]')) {
        expect(localIds).toContain(path.getAttribute("fill")!.slice(5, -1));
      }
    }
    expect(container.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    const labelled = container.querySelector('svg[role="img"]')!;
    expect(labelled.getAttribute("aria-labelledby")).toBe(labelled.querySelector("title")!.id);
  });

  it("uses theme variables in the UI but fixed Nordic colors for branded marks", () => {
    expect(renderToStaticMarkup(<TangleafMark />)).toContain("var(--brand-start");
    const branded = renderToStaticMarkup(<TangleafMark branded />);
    expect(branded).not.toContain("var(--brand-");
    expect(branded).toContain("#143b66");
  });
});

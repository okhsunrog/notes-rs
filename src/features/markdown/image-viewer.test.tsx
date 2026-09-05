// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it } from "vite-plus/test";
import { MarkdownImageViewer } from "./image-viewer";

it("marks the portaled viewer as fullscreen and loads the original only when open", async () => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () =>
      root.render(
        <MarkdownImageViewer
          alt="Photo"
          image={{
            src: "http://localhost/preview",
            originalSrc: "http://localhost/original",
            width: 1024,
            height: 771,
            byteSize: 1000,
            mime: "image/png",
          }}
        />,
      ),
    );
    expect(document.querySelector('[src="http://localhost/original"]')).toBeNull();
    await act(async () => container.querySelector<HTMLButtonElement>("button")!.click());
    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog?.getAttribute("data-fullscreen")).toBe("true");
    expect(dialog?.classList.contains("bg-black")).toBe(true);
    expect(dialog?.querySelector('[src="http://localhost/original"]')).not.toBeNull();
    expect(container.contains(dialog)).toBe(false);
    const original = dialog!.querySelector<HTMLImageElement>('[src="http://localhost/original"]')!;
    let finishDecode!: () => void;
    original.decode = () =>
      new Promise<void>((resolve) => {
        finishDecode = resolve;
      });
    await act(async () => original.dispatchEvent(new Event("load")));
    expect(original.style.visibility).toBe("hidden");
    expect(dialog!.querySelector('[src="http://localhost/preview"]')).not.toBeNull();
    await act(async () => finishDecode());
    expect(original.style.visibility).toBe("visible");
    expect(original.style.transition).toBe("");
    expect(dialog!.querySelector('[src="http://localhost/preview"]')).toBeNull();
    await act(async () =>
      document.querySelector<HTMLButtonElement>('[aria-label="Zoom in"]')!.click(),
    );
    expect(
      document.querySelector<HTMLImageElement>('[src="http://localhost/original"]')!.style
        .transform,
    ).toContain("scale(1.25)");
    await act(async () =>
      document.querySelector<HTMLButtonElement>('[aria-label="Close image viewer"]')!.click(),
    );
    expect(document.querySelector('[src="http://localhost/original"]')).toBeNull();
    await act(async () => container.querySelector<HTMLButtonElement>("button")!.click());
    expect(
      document.querySelector<HTMLImageElement>('[src="http://localhost/original"]')!.style
        .transform,
    ).toBe("translate3d(0px, 0px, 0) scale(1)");
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

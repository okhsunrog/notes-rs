// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vite-plus/test";
import { PagePresentation } from "@/features/pages/page-presentation";
import { DocumentAuthoringControls } from "./document-authoring-controls";
import { DocumentAuthoringAvailability } from "./document-authoring-model";

function render(
  layout: "outline" | "document",
  presentation = PagePresentation.Editing,
  authoringMode: "live_preview" | "source" = "live_preview",
) {
  return renderToStaticMarkup(
    <DocumentAuthoringControls
      layout={layout}
      presentation={presentation}
      authoringMode={authoringMode}
      availability={DocumentAuthoringAvailability.Available}
      onAction={() => undefined}
    />,
  );
}

describe("DocumentAuthoringControls", () => {
  it("renders explicit Write, Source, and Read actions for Document", () => {
    const html = render("document");
    expect(html).toContain('aria-label="Document view"');
    expect(html).toContain('data-document-action="write"');
    expect(html).toContain('data-document-action="source"');
    expect(html).toContain('data-document-action="read"');
    expect(html).toContain('aria-label="Write"');
    expect(html).toContain('aria-label="Source"');
    expect(html).toContain('aria-label="Read"');
  });

  it("keeps the return authoring preference visible while Read is active", () => {
    const html = render("document", PagePresentation.Reading, "source");
    expect(html).toContain('data-document-action="read"');
    expect(html).toContain('data-document-action="source"');
    expect(html).toContain('data-return-mode="true"');
    expect(html).toContain('aria-label="Source, preferred when leaving Read"');
  });

  it("renders no authoring controls for Outline", () => {
    expect(render("outline")).toBe("");
  });

  it("reports the selected action through its component boundary", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const onAction = vi.fn();
    try {
      await act(async () => {
        root.render(
          <DocumentAuthoringControls
            layout="document"
            presentation={PagePresentation.Editing}
            authoringMode="live_preview"
            availability={DocumentAuthoringAvailability.Available}
            onAction={onAction}
          />,
        );
      });
      await act(async () => {
        container.querySelector<HTMLButtonElement>('[data-document-action="source"]')?.click();
      });
      expect(onAction).toHaveBeenCalledOnce();
      expect(onAction).toHaveBeenCalledWith("source");
    } finally {
      act(() => root.unmount());
      container.remove();
    }
  });

  it("disables writer actions and explains the existing writer in another pane", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const onAction = vi.fn();
    try {
      await act(async () => {
        root.render(
          <DocumentAuthoringControls
            layout="document"
            presentation={PagePresentation.Reading}
            authoringMode="source"
            availability={DocumentAuthoringAvailability.WriterInOtherPane}
            onAction={onAction}
          />,
        );
      });

      const write = container.querySelector<HTMLButtonElement>('[data-document-action="write"]');
      const source = container.querySelector<HTMLButtonElement>('[data-document-action="source"]');
      const read = container.querySelector<HTMLButtonElement>('[data-document-action="read"]');
      const status = container.querySelector<HTMLElement>('[role="status"]');

      expect(write?.disabled).toBe(true);
      expect(source?.disabled).toBe(true);
      expect(read?.disabled).toBe(false);
      expect(status?.textContent).toContain("Editing in another pane");
      expect(write?.getAttribute("aria-describedby")).toBe(status?.id);

      await act(async () => write?.click());
      await act(async () => source?.click());
      expect(onAction).not.toHaveBeenCalled();
    } finally {
      act(() => root.unmount());
      container.remove();
    }
  });
});

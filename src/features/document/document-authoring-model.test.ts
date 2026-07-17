import { describe, expect, it } from "vite-plus/test";
import { PagePresentation } from "@/features/pages/page-presentation";
import type { PaneId } from "@/features/workspace/workspace-model";
import {
  activeDocumentAuthoringAction,
  documentAuthoringAvailability,
  DocumentAuthoringAvailability,
  isDocumentAuthoringActionEnabled,
  transitionDocumentAuthoring,
} from "./document-authoring-model";

const pane = (value: string) => value as PaneId;

describe("document authoring actions", () => {
  it("Write selects editing live preview and persists it", () => {
    expect(transitionDocumentAuthoring("write", "source")).toEqual({
      presentation: PagePresentation.Editing,
      authoringMode: "live_preview",
      persistedAuthoringMode: "live_preview",
    });
  });

  it("Source selects editing source and persists it", () => {
    expect(transitionDocumentAuthoring("source", "live_preview")).toEqual({
      presentation: PagePresentation.Editing,
      authoringMode: "source",
      persistedAuthoringMode: "source",
    });
  });

  it("Read preserves rather than overwrites the authoring preference", () => {
    expect(transitionDocumentAuthoring("read", "source")).toEqual({
      presentation: PagePresentation.Reading,
      authoringMode: "source",
      persistedAuthoringMode: null,
    });
    expect(activeDocumentAuthoringAction(PagePresentation.Reading, "source")).toBe("read");
    expect(activeDocumentAuthoringAction(PagePresentation.Editing, "live_preview")).toBe("write");
    expect(activeDocumentAuthoringAction(PagePresentation.Editing, "source")).toBe("source");
  });

  it("derives writer availability from the window-local lease owner", () => {
    expect(documentAuthoringAvailability(pane("reader"), null)).toBe(
      DocumentAuthoringAvailability.Available,
    );
    expect(documentAuthoringAvailability(pane("writer"), pane("writer"))).toBe(
      DocumentAuthoringAvailability.Available,
    );
    expect(documentAuthoringAvailability(pane("reader"), pane("writer"))).toBe(
      DocumentAuthoringAvailability.WriterInOtherPane,
    );
  });

  it("rejects preference-changing actions while another pane owns the writer lease", () => {
    const blocked = DocumentAuthoringAvailability.WriterInOtherPane;
    expect(isDocumentAuthoringActionEnabled("write", blocked)).toBe(false);
    expect(isDocumentAuthoringActionEnabled("source", blocked)).toBe(false);
    expect(isDocumentAuthoringActionEnabled("read", blocked)).toBe(true);
    expect(transitionDocumentAuthoring("write", "source", blocked)).toBeNull();
    expect(transitionDocumentAuthoring("source", "live_preview", blocked)).toBeNull();
    expect(transitionDocumentAuthoring("read", "source", blocked)).toEqual({
      presentation: PagePresentation.Reading,
      authoringMode: "source",
      persistedAuthoringMode: null,
    });
  });
});

// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { useState } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vite-plus/test";
import { PagePresentation } from "@/features/pages/page-presentation";
import {
  PageSessionProvider,
  PageSessionRegistry,
  usePageWriterPaneId,
} from "@/features/pages/page-session";
import type { PaneId } from "@/features/workspace/workspace-model";
import { DocumentAuthoringControls } from "./document-authoring-controls";
import {
  documentAuthoringAvailability,
  transitionDocumentAuthoring,
  type DocumentAuthoringAction,
} from "./document-authoring-model";
import { ContinuousDocumentEditor, type DocumentAuthoringMode } from "./continuous-document-editor";

const cleanup: Array<() => void> = [];

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose();
});

function editor(mode: DocumentAuthoringMode) {
  return (
    <ContinuousDocumentEditor
      value="# Stable draft"
      readOnly={false}
      focusRequest={0}
      mode={mode}
      onChange={() => undefined}
      onCompositionEnd={() => undefined}
      onBlur={() => undefined}
    />
  );
}

function LeaseAwareAuthoringHarness({
  pageUuid,
  paneId,
  onPreferenceChange,
}: {
  pageUuid: string;
  paneId: PaneId;
  onPreferenceChange: (mode: DocumentAuthoringMode) => void;
}) {
  const writerPaneId = usePageWriterPaneId(pageUuid);
  const availability = documentAuthoringAvailability(paneId, writerPaneId);
  const [authoringMode, setAuthoringMode] = useState<DocumentAuthoringMode>("live_preview");
  const [presentation, setPresentation] = useState(PagePresentation.Reading);
  const select = (action: DocumentAuthoringAction) => {
    const transition = transitionDocumentAuthoring(action, authoringMode, availability);
    if (!transition) return;
    if (transition.persistedAuthoringMode) {
      setAuthoringMode(transition.persistedAuthoringMode);
      onPreferenceChange(transition.persistedAuthoringMode);
    }
    setPresentation(transition.presentation);
  };

  return (
    <>
      <DocumentAuthoringControls
        layout="document"
        presentation={presentation}
        authoringMode={authoringMode}
        availability={availability}
        onAction={select}
      />
      <button type="button" data-programmatic-source onClick={() => select("source")}>
        Programmatic Source
      </button>
      <output data-preferred-mode>{authoringMode}</output>
      <output data-presentation>{presentation}</output>
    </>
  );
}

describe("document authoring mode wiring", () => {
  it("reconfigures Write and Source without remounting the editor", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });

    await act(async () => root.render(editor("live_preview")));
    const initialEditor = container.querySelector(".cm-editor");
    expect(initialEditor).not.toBeNull();
    expect(container.querySelector('[data-document-authoring-mode="live_preview"]')).not.toBeNull();

    await act(async () => root.render(editor("source")));
    expect(container.querySelector(".cm-editor")).toBe(initialEditor);
    expect(container.querySelector('[data-document-authoring-mode="source"]')).not.toBeNull();
    expect(container.querySelector(".cm-content")?.textContent).toContain("Stable draft");
  });

  it("does not persist a Reading pane's mode until the other pane releases its writer lease", async () => {
    const pageUuid = "page";
    const writerPane = "writer-pane" as PaneId;
    const readingPane = "reading-pane" as PaneId;
    const registry = new PageSessionRegistry();
    const writerToken = registry.createWriterLeaseToken();
    expect(registry.acquireWriter(pageUuid, writerPane, writerToken)).toBe(true);

    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const onPreferenceChange = vi.fn();
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });

    await act(async () => {
      root.render(
        <PageSessionProvider registry={registry}>
          <LeaseAwareAuthoringHarness
            pageUuid={pageUuid}
            paneId={readingPane}
            onPreferenceChange={onPreferenceChange}
          />
        </PageSessionProvider>,
      );
    });

    expect(
      container.querySelector<HTMLButtonElement>('[data-document-action="source"]')?.disabled,
    ).toBe(true);
    await act(async () => {
      container.querySelector<HTMLButtonElement>("[data-programmatic-source]")?.click();
    });
    expect(onPreferenceChange).not.toHaveBeenCalled();
    expect(container.querySelector("[data-preferred-mode]")?.textContent).toBe("live_preview");
    expect(container.querySelector("[data-presentation]")?.textContent).toBe(
      PagePresentation.Reading,
    );

    act(() => registry.releaseWriter(pageUuid, writerPane, writerToken));
    const source = container.querySelector<HTMLButtonElement>('[data-document-action="source"]');
    expect(source?.disabled).toBe(false);
    await act(async () => source?.click());

    expect(onPreferenceChange).toHaveBeenCalledOnce();
    expect(onPreferenceChange).toHaveBeenCalledWith("source");
    expect(container.querySelector("[data-preferred-mode]")?.textContent).toBe("source");
    expect(container.querySelector("[data-presentation]")?.textContent).toBe(
      PagePresentation.Editing,
    );
  });
});

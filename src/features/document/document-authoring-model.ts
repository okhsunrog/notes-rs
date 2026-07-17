import { PagePresentation } from "@/features/pages/page-presentation";
import type { PaneId } from "@/features/workspace/workspace-model";
import type { DocumentAuthoringMode } from "./continuous-document-editor";

export type DocumentAuthoringAction = "write" | "source" | "read";

export enum DocumentAuthoringAvailability {
  Available = "available",
  WriterInOtherPane = "writer_in_other_pane",
}

export type DocumentAuthoringTransition = Readonly<{
  presentation: PagePresentation;
  authoringMode: DocumentAuthoringMode;
  persistedAuthoringMode: DocumentAuthoringMode | null;
}>;

/**
 * Read is a pane-local projection and never changes the device's preferred
 * authoring mode. Write and Source both return to the one writer surface and
 * persist which CodeMirror configuration that surface should use.
 */
export function transitionDocumentAuthoring(
  action: DocumentAuthoringAction,
  currentAuthoringMode: DocumentAuthoringMode,
  availability = DocumentAuthoringAvailability.Available,
): DocumentAuthoringTransition | null {
  if (!isDocumentAuthoringActionEnabled(action, availability)) return null;
  if (action === "read") {
    return {
      presentation: PagePresentation.Reading,
      authoringMode: currentAuthoringMode,
      persistedAuthoringMode: null,
    };
  }
  const authoringMode: DocumentAuthoringMode = action === "write" ? "live_preview" : "source";
  return {
    presentation: PagePresentation.Editing,
    authoringMode,
    persistedAuthoringMode: authoringMode,
  };
}

export function documentAuthoringAvailability(
  paneId: PaneId,
  writerPaneId: PaneId | null,
): DocumentAuthoringAvailability {
  return writerPaneId !== null && writerPaneId !== paneId
    ? DocumentAuthoringAvailability.WriterInOtherPane
    : DocumentAuthoringAvailability.Available;
}

export function isDocumentAuthoringActionEnabled(
  action: DocumentAuthoringAction,
  availability: DocumentAuthoringAvailability,
): boolean {
  return action === "read" || availability === DocumentAuthoringAvailability.Available;
}

export function activeDocumentAuthoringAction(
  presentation: PagePresentation,
  authoringMode: DocumentAuthoringMode,
): DocumentAuthoringAction {
  if (presentation === PagePresentation.Reading) return "read";
  return authoringMode === "live_preview" ? "write" : "source";
}

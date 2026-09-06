// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vite-plus/test";
import { PageSessionProvider } from "@/features/pages/page-session";
import {
  PaneContentKind,
  currentDisposition,
  pageTarget,
  type PaneId,
} from "@/features/workspace/workspace-model";
import { useWorkspaceStore } from "@/features/workspace/workspace-store";
import type { InkHistorySnapshot, Page } from "@/lib/bindings";
import { HandwritingNoteView } from "./handwriting-note-view";
import { resetHandwritingSessions } from "./handwriting-session";
import { useHandwritingPreference } from "./input-capabilities";
import { emptyDraft } from "./ink-model";

const api = vi.hoisted(() => ({
  loadHandwritingNote: vi.fn(),
  saveHandwritingPatch: vi.fn(),
  completeHandwritingNote: vi.fn(),
  completeAllHandwriting: vi.fn(),
  setHandwritingBackground: vi.fn(),
  handwritingHistory: vi.fn(),
  renamePage: vi.fn(),
  getPage: vi.fn(),
  notifyRetryableError: vi.fn(),
}));

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return { ...actual, ...api };
});
vi.mock("@/lib/notify", () => ({
  notifyError: vi.fn(),
  notifyInfo: vi.fn(),
  notifySuccess: vi.fn(),
  notifyRetryableError: api.notifyRetryableError,
}));

const page: Page = {
  uuid: "019f0000-0000-7000-8000-00000000000a",
  kind: { kind: "handwriting" },
  title: "Sketch",
  layout: "outline",
  titleRevision: "title-revision",
  createdAt: 0,
  updatedAt: 0,
};

const history = (revision: string | null): InkHistorySnapshot => ({
  snapshot: { draft: emptyDraft(), revision },
  canUndo: false,
  canRedo: false,
});

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  for (const mock of Object.values(api)) mock.mockReset();
  api.loadHandwritingNote.mockResolvedValue(history("revision-1"));
  api.completeHandwritingNote.mockResolvedValue({});
  api.setHandwritingBackground.mockResolvedValue(undefined);
  useWorkspaceStore.getState().dispatch({ type: "reset" });
  // Drawing without a detected pen is the explicit device preference.
  useHandwritingPreference.setState({ mouseEnabled: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  resetHandwritingSessions();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function paneContentKind() {
  const state = useWorkspaceStore.getState();
  return state.panes[state.primaryPaneId]?.content.kind;
}

async function mount() {
  const paneId = useWorkspaceStore.getState().primaryPaneId;
  useWorkspaceStore.getState().dispatch({
    type: "open_target",
    target: pageTarget(page.uuid),
    disposition: currentDisposition,
  });
  await act(async () =>
    root.render(
      <PageSessionProvider>
        <HandwritingNoteView
          paneId={paneId as PaneId}
          page={page}
          onSaved={() => {}}
          onDelete={() => {}}
        />
      </PageSessionProvider>,
    ),
  );
}

function button(label: string) {
  const found = [...container.querySelectorAll("button")].find(
    (element) =>
      element.textContent?.trim() === label || element.getAttribute("aria-label") === label,
  );
  if (!found) throw new Error(`no button labelled ${label}`);
  return found;
}

async function visibility(state: "hidden" | "visible") {
  Object.defineProperty(document, "visibilityState", { value: state, configurable: true });
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
}

it("opens the note for editing and reports its background transitions", async () => {
  await mount();

  expect(api.loadHandwritingNote).toHaveBeenCalledWith(page.uuid, true);
  expect(container.querySelector("canvas")).not.toBeNull();

  await visibility("hidden");

  expect(api.setHandwritingBackground).toHaveBeenLastCalledWith(true);
  expect(api.completeHandwritingNote).toHaveBeenCalledWith(page.uuid);
});

it("re-adopts the note with a new revision when the window becomes visible", async () => {
  await mount();

  api.loadHandwritingNote.mockResolvedValue(history("revision-2"));
  let finishCompletion!: () => void;
  api.completeHandwritingNote.mockReturnValue(
    new Promise((resolve) => {
      finishCompletion = () => resolve({});
    }),
  );
  await visibility("hidden");
  await visibility("visible");

  expect(api.setHandwritingBackground).toHaveBeenLastCalledWith(false);
  // The re-read waits for the completion the suspend started.
  expect(api.loadHandwritingNote).toHaveBeenCalledTimes(1);

  await act(async () => finishCompletion());

  expect(api.loadHandwritingNote).toHaveBeenLastCalledWith(page.uuid, true);
  expect(api.loadHandwritingNote).toHaveBeenCalledTimes(2);
});

it("stays on the note when the flush before Back fails", async () => {
  api.saveHandwritingPatch.mockRejectedValue(new Error("disk full"));
  await mount();

  await act(async () => {
    button("Grid paper").click();
  });
  await act(async () => {
    button("Back").click();
  });

  expect(paneContentKind()).toBe(PaneContentKind.Page);
  expect(api.completeHandwritingNote).not.toHaveBeenCalled();
  expect(container.textContent).toContain("Could not save your changes");
});

it("leaves after a successful flush and offers a retry when completion fails", async () => {
  api.saveHandwritingPatch.mockResolvedValue("revision-2");
  api.completeHandwritingNote.mockRejectedValue(new Error("outbox unavailable"));
  await mount();

  await act(async () => {
    button("Grid paper").click();
  });
  await act(async () => {
    button("Back").click();
  });

  expect(paneContentKind()).toBe(PaneContentKind.Home);
  expect(api.saveHandwritingPatch).toHaveBeenCalledWith(
    page.uuid,
    expect.objectContaining({ background: "grid" }),
    "revision-1",
  );
  expect(api.notifyRetryableError).toHaveBeenCalledWith(
    "Could not prepare sync",
    expect.any(Error),
    expect.any(Function),
  );

  api.completeHandwritingNote.mockResolvedValue({});
  const retry = api.notifyRetryableError.mock.calls[0]![2] as () => void;
  await act(async () => retry());

  expect(api.completeHandwritingNote).toHaveBeenCalledTimes(2);
});

it("opens read-only without a pen and without the mouse preference", async () => {
  useHandwritingPreference.setState({ mouseEnabled: false });
  await mount();

  expect(api.loadHandwritingNote).toHaveBeenCalledWith(page.uuid, false);
  expect(container.querySelector('[role="toolbar"]')).toBeNull();
  expect(container.textContent).toContain("Connect a pen or enable mouse drawing in Settings");
  expect(container.querySelector("canvas")).not.toBeNull();
});

// @vitest-environment jsdom
import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vite-plus/test";
import type { InkVersionInfo } from "@/lib/bindings";
import { HandwritingConflictDialog } from "./handwriting-conflicts";
import { emptyDraft } from "./ink-model";

const api = vi.hoisted(() => ({
  previewHandwritingVersion: vi.fn(),
  resolveHandwritingConflict: vi.fn(),
  handwritingNoteStatus: vi.fn(),
}));

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return { ...actual, ...api };
});

const head = (versionUuid: string, available = true): InkVersionInfo => ({
  versionUuid,
  parents: [],
  rootHash: `hash-${versionUuid}`,
  deviceName: `Device ${versionUuid}`,
  deviceId: `id-${versionUuid}`,
  modifiedAtMs: 1_757_000_000_000,
  available,
});

let container: HTMLDivElement;
let root: Root;
let queryClient: QueryClient;
const onClose = vi.fn();
const onResolved = vi.fn();

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
  onClose.mockReset();
  onResolved.mockReset();
  api.previewHandwritingVersion.mockResolvedValue(emptyDraft());
  queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function render(heads: InkVersionInfo[]) {
  await act(async () =>
    root.render(
      <QueryClientProvider client={queryClient}>
        <HandwritingConflictDialog
          pageUuid="page-a"
          heads={heads}
          onClose={onClose}
          onResolved={onResolved}
        />
      </QueryClientProvider>,
    ),
  );
}

function cards() {
  return [...document.querySelectorAll("section[aria-label^='Version from']")];
}

function buttonIn(card: Element, label: string) {
  const found = [...card.querySelectorAll("button")].find(
    (element) => element.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no ${label} button`);
  return found;
}

function dialogButton(label: string) {
  const found = [...document.querySelectorAll("button")].find(
    (element) => element.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no ${label} button`);
  return found;
}

it("renders one card per head and refuses to keep a version that is not downloaded", async () => {
  await render([head("a"), head("b", false), head("c")]);

  expect(cards()).toHaveLength(3);
  expect(document.body.textContent).toContain("Not downloaded yet");
  expect(api.previewHandwritingVersion).toHaveBeenCalledTimes(2);
  expect(buttonIn(cards()[1]!, "Keep this one").disabled).toBe(true);
  expect(dialogButton("Keep all as separate notes").disabled).toBe(true);
});

it("keeps one version and passes every current head as the expectation", async () => {
  api.resolveHandwritingConflict.mockResolvedValue(["page-a"]);
  await render([head("a"), head("b")]);

  await act(async () => {
    buttonIn(cards()[1]!, "Keep this one").click();
  });

  expect(api.resolveHandwritingConflict).toHaveBeenCalledWith("page-a", ["a", "b"], ["b"]);
  expect(onResolved).toHaveBeenCalledWith(["page-a"]);
});

it("keeps every version as separate notes in head order", async () => {
  api.resolveHandwritingConflict.mockResolvedValue(["page-a", "page-b"]);
  await render([head("a"), head("b")]);

  await act(async () => {
    dialogButton("Keep all as separate notes").click();
  });

  expect(api.resolveHandwritingConflict).toHaveBeenCalledWith("page-a", ["a", "b"], ["a", "b"]);
});

it("re-reads the versions and asks again after a stale resolve", async () => {
  const { CommandFailure } = await import("@/lib/api");
  api.resolveHandwritingConflict.mockRejectedValue(
    new CommandFailure({ code: "conflict", message: "heads moved" }),
  );
  api.handwritingNoteStatus.mockResolvedValue({
    pageUuid: "page-a",
    revision: "revision-2",
    unpublishedChanges: false,
    publicationRequested: false,
    baseVersion: null,
    heads: [head("a"), head("c")],
  });
  await render([head("a"), head("b")]);

  await act(async () => {
    buttonIn(cards()[1]!, "Keep this one").click();
  });

  expect(document.body.textContent).toContain("Versions changed, choose again.");
  expect(cards().map((card) => card.getAttribute("aria-label"))).toEqual([
    "Version from Device a",
    "Version from Device c",
  ]);
  expect(onResolved).not.toHaveBeenCalled();
});

it("closes without resolving when the choice is postponed", async () => {
  await render([head("a"), head("b")]);

  await act(async () => {
    dialogButton("Later").click();
  });

  expect(onClose).toHaveBeenCalled();
  expect(api.resolveHandwritingConflict).not.toHaveBeenCalled();
});

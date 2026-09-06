// @vitest-environment jsdom
import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { ConfirmationProvider } from "@/app/confirmation";
import { useWorkspaceStore } from "@/features/workspace/workspace-store";
import type { Page } from "@/lib/api";
import { PageSessionProvider } from "./page-session";
import { useNotesWorkspace } from "./use-notes-workspace";

const api = vi.hoisted(() => ({ createHandwrittenNote: vi.fn() }));
vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return { ...actual, ...api };
});
vi.mock("@/lib/notify", () => ({
  notifyError: vi.fn(),
  notifyInfo: vi.fn(),
  notifySuccess: vi.fn(),
  notifyRetryableError: vi.fn(),
}));

const created: Page = {
  uuid: "019f0000-0000-7000-8000-0000000000ff",
  kind: { kind: "handwriting" },
  title: "Handwriting 2026-09-07 02:05",
  layout: "outline",
  titleRevision: "title-revision",
  createdAt: 0,
  updatedAt: 0,
};

let workspace: ReturnType<typeof useNotesWorkspace>;
function Harness() {
  workspace = useNotesWorkspace(false, () => {});
  return null;
}

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

beforeAll(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
});

beforeEach(() => {
  api.createHandwrittenNote.mockReset().mockResolvedValue(created);
  useWorkspaceStore.getState().dispatch({ type: "reset" });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

it("names a new handwritten note after the moment it was created", async () => {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  await act(async () =>
    root.render(
      <QueryClientProvider client={queryClient}>
        <ConfirmationProvider>
          <PageSessionProvider>
            <Harness />
          </PageSessionProvider>
        </ConfirmationProvider>
      </QueryClientProvider>,
    ),
  );

  await act(async () => workspace.createHandwrittenNote());

  // The sheet has nowhere to type a name, so the note is never created untitled.
  expect(api.createHandwrittenNote).toHaveBeenCalledTimes(1);
  expect(api.createHandwrittenNote.mock.calls[0]![0]).toMatch(
    /^Handwriting \d{4}-\d{2}-\d{2} \d{2}:\d{2}$/,
  );
});

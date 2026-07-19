// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { WorkspaceControllerProvider } from "@/features/workspace/workspace-controller";
import type { WorkspaceController } from "@/features/workspace/workspace-controller";
import type { Page, SearchHit } from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { SearchCard } from "./search-card";

const api = vi.hoisted(() => ({
  searchResults: [] as SearchHit[],
  search: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  contentText: (content: { record: { markdown?: string; title?: string | null } }) =>
    content.record.markdown ?? content.record.title ?? "",
  contentUuid: (content: { record: { uuid: string } }) => content.record.uuid,
  getPage: vi.fn(async () => null),
  listJournals: vi.fn(async () => []),
  listPages: vi.fn(async () => []),
  loadSettings: vi.fn(async () => ({
    aiSearchEnabled: false,
    aiSearchRerank: true,
    aiSearchTrigger: "as_you_type",
    configuredKeys: [],
    searchDebugSources: false,
    syncServerUrl: "",
  })),
  search: api.search,
}));

const mounted: Array<{ container: HTMLDivElement; root: Root }> = [];

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.useFakeTimers();
  api.search.mockReset();
  api.search.mockImplementation(async () => api.searchResults);
  api.searchResults = [];
});

afterEach(() => {
  for (const { container, root } of mounted.splice(0)) {
    act(() => root.unmount());
    container.remove();
  }
  vi.useRealTimers();
});

describe("SearchCard page-list refreshes", () => {
  it("re-runs active local search after a page rename", async () => {
    const original = page("page-1", "Project Alpha");
    const renamed = page("page-1", "Project Beta");
    const { container, queryClient } = await renderSearchCard([original]);
    api.searchResults = [pageHit(original)];

    await enterQuery(container, "Project");
    expect(container.textContent).toContain("Project Alpha");

    api.searchResults = [pageHit(renamed)];
    await act(async () => {
      queryClient.setQueryData(queryKeys.pages, [renamed]);
      await vi.advanceTimersByTimeAsync(0);
      await Promise.resolve();
    });

    expect(api.search).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("Project Beta");
    expect(container.textContent).not.toContain("Project Alpha");
  });

  it("removes a displayed hit after its page is deleted", async () => {
    const deleted = page("page-1", "Disposable note");
    const { container, queryClient } = await renderSearchCard([deleted]);
    api.searchResults = [pageHit(deleted)];

    await enterQuery(container, "Disposable");
    expect(container.textContent).toContain("Disposable note");

    api.searchResults = [];
    await act(async () => {
      queryClient.setQueryData(queryKeys.pages, []);
      await vi.advanceTimersByTimeAsync(0);
      await Promise.resolve();
    });

    expect(api.search).toHaveBeenCalledTimes(2);
    expect(container.textContent).not.toContain("Disposable note");
  });
});

async function renderSearchCard(initialPages: Page[]) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Number.POSITIVE_INFINITY } },
  });
  queryClient.setQueryData(queryKeys.pages, initialPages);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  mounted.push({ container, root });
  await act(async () => {
    root.render(
      <QueryClientProvider client={queryClient}>
        <WorkspaceControllerProvider controller={controller}>
          <SearchCard variant="dialog" onOpenContent={() => undefined} />
        </WorkspaceControllerProvider>
      </QueryClientProvider>,
    );
  });
  return { container, queryClient };
}

async function enterQuery(container: HTMLDivElement, value: string) {
  const input = container.querySelector<HTMLInputElement>('input[role="combobox"]');
  if (!input) throw new Error("search input was not rendered");
  await act(async () => {
    const descriptor = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
    if (!descriptor?.set) throw new Error("input value setter is unavailable");
    descriptor.set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(150);
  });
}

function page(uuid: string, title: string): Page {
  return {
    uuid,
    kind: { kind: "note" },
    title,
    layout: "outline",
    titleRevision: "0000000000000000-00000000-00000000000000000000000000000001",
    createdAt: 0,
    updatedAt: 0,
  };
}

function pageHit(record: Page): SearchHit {
  return { content: { kind: "page", record }, score: 1, snippet: null };
}

const controller: WorkspaceController = {
  createNewNote: () => undefined,
  openContent: () => undefined,
  openJournal: () => undefined,
  captureJournal: async () => true,
  onSaved: () => undefined,
  onDelete: () => undefined,
  openMarkdownLink: () => undefined,
};

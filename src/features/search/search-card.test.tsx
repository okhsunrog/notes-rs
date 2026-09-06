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
  notifyError: vi.fn(),
  createNewNote: vi.fn(),
  settings: {
    aiSearchEnabled: false,
    aiSearchRerank: true,
    aiSearchTrigger: "as_you_type",
    configuredKeys: [] as string[],
    searchDebugSources: false,
    syncServerUrl: "",
  },
}));

vi.mock("@/lib/api", () => ({
  contentText: (content: { record: { markdown?: string; title?: string | null } }) =>
    content.record.markdown ?? content.record.title ?? "",
  contentUuid: (content: { record: { uuid: string } }) => content.record.uuid,
  getPage: vi.fn(async () => null),
  listJournals: vi.fn(async () => []),
  listPages: vi.fn(async () => []),
  loadSettings: vi.fn(async () => api.settings),
  search: api.search,
}));

vi.mock("@/lib/notify", () => ({ notifyError: api.notifyError }));

const mounted: Array<{ container: HTMLDivElement; root: Root }> = [];

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.useFakeTimers();
  api.search.mockReset();
  api.notifyError.mockReset();
  api.createNewNote.mockReset();
  api.search.mockImplementation(async () => api.searchResults);
  api.searchResults = [];
  api.settings = {
    aiSearchEnabled: false,
    aiSearchRerank: true,
    aiSearchTrigger: "as_you_type",
    configuredKeys: [],
    searchDebugSources: false,
    syncServerUrl: "",
  };
});

afterEach(() => {
  for (const { container, root } of mounted.splice(0)) {
    act(() => root.unmount());
    container.remove();
  }
  vi.useRealTimers();
});

describe("SearchCard page-list refreshes", () => {
  it("uses full-text search in the sidebar and renders matching block content", async () => {
    const containingPage = page("page-1", "Math notes");
    const block: SearchHit = {
      content: {
        kind: "block",
        record: {
          uuid: "block-1",
          pageUuid: containingPage.uuid,
          parentUuid: null,
          orderKey: "a0",
          markdown: "Теорема Пифагора",
          style: { kind: "bullet" },
          markdownRevision: "0000000000000000-00000000-00000000000000000000000000000001",
          createdAt: 0,
          updatedAt: 0,
        },
      },
      score: 1,
      snippet: "<mark>Теорема</mark> Пифагора",
    };
    api.searchResults = [block];
    const { container } = await renderSearchCard([containingPage], "sidebar");

    await enterQuery(container, "Теорема");

    expect(api.search).toHaveBeenCalledWith("fts", "Теорема", 20);
    expect(container.textContent).toContain("Теорема Пифагора");
  });

  it("re-runs active local search after a page rename", async () => {
    const original = page("page-1", "Project Alpha");
    const renamed = page("page-1", "Project Beta");
    const { container, queryClient } = await renderSearchCard([original]);
    api.searchResults = [pageHit(original)];

    await enterQuery(container, "Project");
    expect(container.textContent).toContain("Project Alpha");

    api.searchResults = [pageHit(renamed)];
    await act(async () => {
      queryClient.setQueryData(queryKeys.pageList("notes", 10_000), [renamed]);
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
      queryClient.setQueryData(queryKeys.pageList("notes", 10_000), []);
      await vi.advanceTimersByTimeAsync(0);
      await Promise.resolve();
    });

    expect(api.search).toHaveBeenCalledTimes(2);
    expect(container.textContent).not.toContain("Disposable note");
  });

  it("shows an enter-only failure and retries it on the next Enter", async () => {
    api.settings = {
      ...api.settings,
      aiSearchEnabled: true,
      aiSearchTrigger: "enter_only",
      configuredKeys: ["SYNC_TOKEN"],
      syncServerUrl: "https://sync.example.test",
    };
    let semanticAttempts = 0;
    api.search.mockImplementation(async (mode: string) => {
      if (mode === "fts") return [];
      semanticAttempts += 1;
      if (semanticAttempts === 1) throw new Error("temporary server failure");
      return [];
    });
    const { container } = await renderSearchCard([]);

    await enterQuery(container, "semantic");
    await pressEnter(container);
    expect(container.textContent).toContain("AI search failed — press Enter to retry");

    await pressEnter(container);
    expect(semanticAttempts).toBe(2);
    expect(
      api.search.mock.calls.filter(([mode]) => mode === "semantic").map((call) => call[3]),
    ).toEqual([true, true]);
    expect(container.textContent).not.toContain("AI search failed — press Enter to retry");
  });

  it("starts enter-only AI search before the local debounce settles", async () => {
    api.settings = {
      ...api.settings,
      aiSearchEnabled: true,
      aiSearchTrigger: "enter_only",
      configuredKeys: ["SYNC_TOKEN"],
      syncServerUrl: "https://sync.example.test",
    };
    api.search.mockResolvedValue([]);
    const { container } = await renderSearchCard([]);

    await typeQuery(container, "instant");
    await pressEnter(container);

    expect(api.search).toHaveBeenCalledWith("semantic", "instant", 20, true);
    expect(api.search).not.toHaveBeenCalledWith("fts", "instant", 20);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(150);
    });
    expect(api.createNewNote).not.toHaveBeenCalled();
  });

  it("refreshes a frozen result snapshot without exposing later server reordering", async () => {
    api.settings = {
      ...api.settings,
      aiSearchEnabled: true,
      aiSearchTrigger: "as_you_type",
      configuredKeys: ["SYNC_TOKEN"],
      syncServerUrl: "https://sync.example.test",
    };
    const alpha = page("page-a", "Alpha");
    const renamed = page("page-a", "Alpha renamed");
    const beta = page("page-b", "Beta");
    const serverOnly = page("page-server", "Server reordered");
    let localResults = [pageHit(alpha), pageHit(beta)];
    let resolveSemantic: ((hits: SearchHit[]) => void) | undefined;
    api.search.mockImplementation(async (mode: string) => {
      if (mode === "fts") return localResults;
      return new Promise<SearchHit[]>((resolve) => {
        resolveSemantic = resolve;
      });
    });
    const { container, queryClient } = await renderSearchCard([alpha, beta]);

    await enterQuery(container, "alpha");
    await pressArrowDown(container);
    localResults = [pageHit(renamed), pageHit(beta)];
    await act(async () => {
      queryClient.setQueryData(queryKeys.pageList("notes", 10_000), [renamed, beta]);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(container.textContent).toContain("Alpha renamed");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(250);
      resolveSemantic?.([pageHit(serverOnly)]);
      await Promise.resolve();
    });
    expect(container.textContent).toContain("Alpha renamed");
    expect(container.textContent).not.toContain("Server reordered");
  });

  it("disables reranking for as-you-type semantic requests", async () => {
    api.settings = {
      ...api.settings,
      aiSearchEnabled: true,
      aiSearchRerank: true,
      aiSearchTrigger: "as_you_type",
      configuredKeys: ["SYNC_TOKEN"],
      syncServerUrl: "https://sync.example.test",
    };
    const { container } = await renderSearchCard([]);

    await enterQuery(container, "semantic");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250);
    });

    expect(api.search).toHaveBeenCalledWith("semantic", "semantic", 20, false);
  });

  it("reports local failures and withholds Create until a successful empty search", async () => {
    api.search.mockRejectedValueOnce(new Error("database unavailable"));
    const { container } = await renderSearchCard([]);

    await enterQuery(container, "broken");
    expect(api.notifyError).toHaveBeenCalledWith("search", expect.any(Error));
    expect(container.textContent).not.toContain("Create “broken”");

    api.search.mockResolvedValueOnce([]);
    await enterQuery(container, "recovered");
    expect(container.textContent).toContain("Create “recovered”");
  });
});

async function renderSearchCard(initialPages: Page[], variant: "dialog" | "sidebar" = "dialog") {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Number.POSITIVE_INFINITY } },
  });
  queryClient.setQueryData(queryKeys.pageList("notes", 10_000), initialPages);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  mounted.push({ container, root });
  await act(async () => {
    root.render(
      <QueryClientProvider client={queryClient}>
        <WorkspaceControllerProvider controller={controller}>
          <SearchCard variant={variant} onOpenContent={() => undefined} />
        </WorkspaceControllerProvider>
      </QueryClientProvider>,
    );
  });
  return { container, queryClient };
}

async function enterQuery(container: HTMLDivElement, value: string) {
  await typeQuery(container, value);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(150);
  });
}

async function typeQuery(container: HTMLDivElement, value: string) {
  const input = container.querySelector<HTMLInputElement>('input[role="combobox"]');
  if (!input) throw new Error("search input was not rendered");
  await act(async () => {
    const descriptor = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
    if (!descriptor?.set) throw new Error("input value setter is unavailable");
    descriptor.set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function pressEnter(container: HTMLDivElement) {
  const input = container.querySelector<HTMLInputElement>('input[role="combobox"]');
  if (!input) throw new Error("search input was not rendered");
  await act(async () => {
    input.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Enter" }));
    await Promise.resolve();
  });
}

async function pressArrowDown(container: HTMLDivElement) {
  const input = container.querySelector<HTMLInputElement>('input[role="combobox"]');
  if (!input) throw new Error("search input was not rendered");
  await act(async () => {
    input.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowDown" }));
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
  createNewNote: api.createNewNote,
  createHandwrittenNote: () => undefined,
  openAllNotes: () => undefined,
  openContent: () => undefined,
  openJournal: () => undefined,
  captureJournal: async () => true,
  onSaved: () => undefined,
  onDelete: () => undefined,
  openMarkdownLink: () => undefined,
};

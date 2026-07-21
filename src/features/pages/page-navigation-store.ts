import { create } from "zustand";

const STORAGE_KEY = "notes-rs:page-navigation:v1";
const MAX_RECENT_PAGES = 20;

type StoredPageNavigation = {
  version: 1;
  recentPageUuids: string[];
  favoritePageUuids: string[];
};

type PageNavigationStore = {
  recentPageUuids: string[];
  favoritePageUuids: string[];
  recordOpenedPage: (pageUuid: string) => void;
  toggleFavoritePage: (pageUuid: string) => void;
  removePage: (pageUuid: string) => void;
};

const initial = readStoredPageNavigation();

export const usePageNavigationStore = create<PageNavigationStore>()((set) => ({
  recentPageUuids: initial.recentPageUuids,
  favoritePageUuids: initial.favoritePageUuids,
  recordOpenedPage: (pageUuid) =>
    set((state) => {
      const recentPageUuids = nextRecentPageUuids(state.recentPageUuids, pageUuid);
      persistPageNavigation(recentPageUuids, state.favoritePageUuids);
      return { recentPageUuids };
    }),
  toggleFavoritePage: (pageUuid) =>
    set((state) => {
      const favoritePageUuids = toggleFavoritePageUuid(state.favoritePageUuids, pageUuid);
      persistPageNavigation(state.recentPageUuids, favoritePageUuids);
      return { favoritePageUuids };
    }),
  removePage: (pageUuid) =>
    set((state) => {
      const recentPageUuids = state.recentPageUuids.filter((uuid) => uuid !== pageUuid);
      const favoritePageUuids = state.favoritePageUuids.filter((uuid) => uuid !== pageUuid);
      persistPageNavigation(recentPageUuids, favoritePageUuids);
      return { recentPageUuids, favoritePageUuids };
    }),
}));

export function nextRecentPageUuids(current: readonly string[], pageUuid: string): string[] {
  return [pageUuid, ...current.filter((uuid) => uuid !== pageUuid)].slice(0, MAX_RECENT_PAGES);
}

export function toggleFavoritePageUuid(current: readonly string[], pageUuid: string): string[] {
  return current.includes(pageUuid)
    ? current.filter((uuid) => uuid !== pageUuid)
    : [...current, pageUuid];
}

function readStoredPageNavigation(): StoredPageNavigation {
  const fallback: StoredPageNavigation = {
    version: 1,
    recentPageUuids: [],
    favoritePageUuids: [],
  };
  try {
    const storage = localStorageOrNull();
    if (!storage) return fallback;
    const parsed = JSON.parse(storage.getItem(STORAGE_KEY) ?? "null") as unknown;
    if (!isStoredPageNavigation(parsed)) return fallback;
    return {
      version: 1,
      recentPageUuids: uniqueStrings(parsed.recentPageUuids).slice(0, MAX_RECENT_PAGES),
      favoritePageUuids: uniqueStrings(parsed.favoritePageUuids),
    };
  } catch {
    return fallback;
  }
}

function persistPageNavigation(recentPageUuids: string[], favoritePageUuids: string[]) {
  const stored: StoredPageNavigation = {
    version: 1,
    recentPageUuids,
    favoritePageUuids,
  };
  try {
    localStorageOrNull()?.setItem(STORAGE_KEY, JSON.stringify(stored));
  } catch {
    // Navigation stays functional when device storage is unavailable or full.
  }
}

function isStoredPageNavigation(value: unknown): value is StoredPageNavigation {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<StoredPageNavigation>;
  return (
    candidate.version === 1 &&
    Array.isArray(candidate.recentPageUuids) &&
    Array.isArray(candidate.favoritePageUuids)
  );
}

function uniqueStrings(values: unknown[]): string[] {
  return [...new Set(values.filter((value): value is string => typeof value === "string"))];
}

function localStorageOrNull(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

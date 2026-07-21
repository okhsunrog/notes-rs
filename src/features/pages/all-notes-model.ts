import { pageDisplayTitle } from "@/features/journal/journal-date";
import type { Page, PageLayout } from "@/lib/api";

export type AllNotesScope = "all" | "favorites";
export type AllNotesLayout = "all" | PageLayout;
export type AllNotesSort = "opened" | "updated" | "created" | "title";

export type AllNotesOptions = Readonly<{
  query: string;
  scope: AllNotesScope;
  layout: AllNotesLayout;
  sort: AllNotesSort;
  favoritePageUuids: readonly string[];
  recentPageUuids: readonly string[];
}>;

export function presentAllNotes(pages: readonly Page[], options: AllNotesOptions): Page[] {
  const query = normalizeTitle(options.query);
  const favorites = new Set(options.favoritePageUuids);
  const recentRank = new Map(options.recentPageUuids.map((uuid, index) => [uuid, index]));

  return pages
    .filter((page) => page.kind.kind === "note")
    .filter((page) => options.scope === "all" || favorites.has(page.uuid))
    .filter((page) => options.layout === "all" || page.layout === options.layout)
    .filter((page) => !query || normalizeTitle(pageDisplayTitle(page)).includes(query))
    .sort((left, right) => comparePages(left, right, options.sort, recentRank));
}

function comparePages(
  left: Page,
  right: Page,
  sort: AllNotesSort,
  recentRank: ReadonlyMap<string, number>,
): number {
  if (sort === "title") {
    const titleOrder = pageDisplayTitle(left).localeCompare(pageDisplayTitle(right), undefined, {
      sensitivity: "base",
      numeric: true,
    });
    return titleOrder || left.uuid.localeCompare(right.uuid);
  }
  if (sort === "opened") {
    const leftRank = recentRank.get(left.uuid) ?? Number.POSITIVE_INFINITY;
    const rightRank = recentRank.get(right.uuid) ?? Number.POSITIVE_INFINITY;
    if (leftRank !== rightRank) return leftRank - rightRank;
  }
  const timestampOrder =
    sort === "created" ? right.createdAt - left.createdAt : right.updatedAt - left.updatedAt;
  return timestampOrder || left.uuid.localeCompare(right.uuid);
}

function normalizeTitle(value: string): string {
  return value.normalize("NFKC").trim().toLocaleLowerCase();
}

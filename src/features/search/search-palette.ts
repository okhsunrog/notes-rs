import { parseJournalDate } from "@/features/journal/journal-date";
import type { JournalDate, SearchHit } from "@/lib/api";

export type PaletteKeyedItem = { key: string };

export function journalDateFromSearchQuery(query: string): JournalDate | null {
  return parseJournalDate(query.trim());
}

export function normalizeSearchTitle(title: string): string {
  return title.trim().normalize("NFKC").toLowerCase();
}

export function hasExactPageTitle(query: string, hits: readonly SearchHit[]): boolean {
  const normalizedQuery = normalizeSearchTitle(query);
  return hits.some(
    (hit) =>
      hit.content.kind === "page" &&
      hit.content.record.title !== null &&
      normalizeSearchTitle(hit.content.record.title) === normalizedQuery,
  );
}

export function stablePaletteItems<T>(
  current: readonly T[],
  incoming: readonly T[],
  frozen: boolean,
): readonly T[] {
  return frozen ? current : incoming;
}

export function preservePaletteSelection<T extends PaletteKeyedItem>(
  selectedKey: string | null,
  items: readonly T[],
): string | null {
  if (items.length === 0) return null;
  return selectedKey !== null && items.some((item) => item.key === selectedKey)
    ? selectedKey
    : items[0].key;
}

export function movePaletteSelection<T extends PaletteKeyedItem>(
  selectedKey: string | null,
  items: readonly T[],
  direction: -1 | 1,
): string | null {
  if (items.length === 0) return null;
  const currentIndex = items.findIndex((item) => item.key === selectedKey);
  if (currentIndex < 0) {
    return direction === 1 ? items[0].key : (items[items.length - 1]?.key ?? null);
  }
  const nextIndex = (currentIndex + direction + items.length) % items.length;
  return items[nextIndex].key;
}

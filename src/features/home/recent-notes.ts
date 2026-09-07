import type { Page } from "@/lib/api";

/** The old "Continue" card showed 4 and "Recently edited" 6; the merged card keeps the larger. */
export const RECENT_NOTES_LIMIT = 6;

/** How many of the device's last-opened notes stay at the top whatever their edit time. */
const PINNED_RECENTLY_OPENED = 3;

/**
 * The one "Recent" list of the dashboard.
 *
 * Two sources feed it and they overlap almost completely: what this device last opened, and what
 * was last edited on any device. Only the second is timestamped — the navigation store keeps an
 * ordered list of uuids and no "opened at" — so the two cannot be interleaved by time. The rule
 * instead: the three most recently opened notes lead, in the order they were opened, and the rest
 * follow by `updatedAt`, newest first, with the store's order breaking ties between equal stamps.
 * Reading a note therefore keeps it in reach without letting a long reading session push away
 * everything that was actually written.
 *
 * A note reached through the store is resolved against `notes`, so anything already deleted or
 * outside the note list simply drops out, and no page appears twice.
 */
export function recentNotes(
  notes: readonly Page[],
  recentPageUuids: readonly string[],
  limit: number = RECENT_NOTES_LIMIT,
): Page[] {
  const byUuid = new Map(notes.map((page) => [page.uuid, page]));
  const openedRank = new Map(recentPageUuids.map((uuid, index) => [uuid, index]));
  const opened = recentPageUuids
    .flatMap((uuid) => {
      const page = byUuid.get(uuid);
      return page ? [page] : [];
    })
    .slice(0, PINNED_RECENTLY_OPENED);
  const pinned = new Set(opened.map((page) => page.uuid));
  const rank = (page: Page) => openedRank.get(page.uuid) ?? Number.MAX_SAFE_INTEGER;
  const rest = notes
    .filter((page) => !pinned.has(page.uuid))
    .sort((a, b) => b.updatedAt - a.updatedAt || rank(a) - rank(b));
  return [...opened, ...rest].slice(0, limit);
}

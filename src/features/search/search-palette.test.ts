import { describe, expect, it } from "vitest";
import type { SearchHit } from "@/lib/api";
import {
  hasExactPageTitle,
  journalDateFromSearchQuery,
  movePaletteSelection,
  preservePaletteSelection,
  resolvePendingPaletteEnter,
  stablePaletteItems,
} from "./search-palette";

function pageHit(uuid: string, title: string): SearchHit {
  return {
    content: {
      kind: "page",
      record: {
        uuid,
        kind: { kind: "note" },
        title,
        layout: "outline",
        titleRevision: "0000000000000000-00000000-00000000000000000000000000000001",
        createdAt: 0,
        updatedAt: 0,
      },
    },
    score: 1,
    snippet: null,
  };
}

describe("search palette helpers", () => {
  it("recognizes valid trimmed ISO journal dates only", () => {
    expect(journalDateFromSearchQuery(" 2026-07-19 ")).toBe("2026-07-19");
    expect(journalDateFromSearchQuery("2026-02-30")).toBeNull();
    expect(journalDateFromSearchQuery("19.07.2026")).toBeNull();
  });

  it("matches exact titles using the backend normalization contract", () => {
    const hits = [pageHit("page-1", "Проект Ёж")];
    expect(hasExactPageTitle("  ПРОЕКТ ЁЖ  ", hits)).toBe(true);
    expect(hasExactPageTitle("Проект", hits)).toBe(false);
  });

  it("freezes incoming items after keyboard navigation", () => {
    const local = [{ key: "content:shared" }, { key: "content:local" }];
    const server = [{ key: "content:server" }, { key: "content:shared" }];
    expect(stablePaletteItems(local, server, true)).toBe(local);
    expect(stablePaletteItems(local, server, false)).toBe(server);
  });

  it("preserves selection by stable content key and falls back to the first row", () => {
    const server = [{ key: "content:server" }, { key: "content:shared" }];
    expect(preservePaletteSelection("content:shared", server)).toBe("content:shared");
    expect(preservePaletteSelection("content:local", server)).toBe("content:server");
    expect(preservePaletteSelection("content:local", [])).toBeNull();
  });

  it("moves selection cyclically in either direction", () => {
    const items = [{ key: "one" }, { key: "two" }, { key: "three" }];
    expect(movePaletteSelection("one", items, -1)).toBe("three");
    expect(movePaletteSelection("three", items, 1)).toBe("one");
  });

  it("resolves a pending Enter to the first row only after the same query settles", () => {
    const pending = { query: "project", shiftKey: true };
    const rows = [{ key: "first" }, { key: "second" }];

    expect(resolvePendingPaletteEnter(pending, "project", false, rows)).toBeNull();
    expect(resolvePendingPaletteEnter(pending, "changed", true, rows)).toBeNull();
    expect(resolvePendingPaletteEnter(pending, "project", true, [])).toBeNull();
    expect(resolvePendingPaletteEnter(pending, "project", true, rows)).toEqual({
      item: rows[0],
      shiftKey: true,
    });
  });
});

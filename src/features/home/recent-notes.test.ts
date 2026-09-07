import { describe, expect, it } from "vitest";
import type { Page } from "@/lib/api";
import { recentNotes, RECENT_NOTES_LIMIT } from "./recent-notes";

function note(uuid: string, updatedAt: number): Page {
  return {
    uuid,
    kind: { kind: "note" },
    title: uuid,
    layout: "outline",
    titleRevision: "1",
    createdAt: 0,
    updatedAt,
  };
}

// listPages hands the dashboard its notes newest-edited first.
const notes = [
  note("f", 60),
  note("e", 50),
  note("d", 40),
  note("c", 30),
  note("b", 20),
  note("a", 10),
];

const uuids = (pages: Page[]) => pages.map((page) => page.uuid);

describe("recentNotes", () => {
  it("falls back to edit order when nothing was opened on this device", () => {
    expect(uuids(recentNotes(notes, []))).toEqual(["f", "e", "d", "c", "b", "a"]);
  });

  it("floats the three most recently opened notes above the freshly edited ones", () => {
    expect(uuids(recentNotes(notes, ["a", "b", "c", "d"]))).toEqual(["a", "b", "c", "f", "e", "d"]);
  });

  it("shows a note that was both opened and edited only once", () => {
    const merged = recentNotes(notes, ["f", "a"]);
    expect(uuids(merged)).toEqual(["f", "a", "e", "d", "c", "b"]);
    expect(new Set(uuids(merged)).size).toBe(merged.length);
  });

  it("breaks a tie between equal edit times by how recently the note was opened", () => {
    const sameDay = [note("x", 10), note("y", 10), note("z", 10)];
    expect(uuids(recentNotes(sameDay, ["z", "y", "x"], 3))).toEqual(["z", "y", "x"]);
  });

  it("drops recents that are no longer in the note list", () => {
    expect(uuids(recentNotes(notes, ["gone", "a"]))).toEqual(["a", "f", "e", "d", "c", "b"]);
  });

  it("keeps the longer of the two lists it replaced", () => {
    const many = Array.from({ length: 20 }, (_, index) => note(`n${index}`, 100 - index));
    expect(recentNotes(many, [])).toHaveLength(RECENT_NOTES_LIMIT);
  });

  it("leaves the caller's array alone", () => {
    const source = [...notes];
    recentNotes(source, ["a"]);
    expect(uuids(source)).toEqual(["f", "e", "d", "c", "b", "a"]);
  });
});

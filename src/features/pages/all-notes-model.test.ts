import { describe, expect, it } from "vite-plus/test";
import type { Page } from "@/lib/api";
import { presentAllNotes, type AllNotesOptions } from "./all-notes-model";

function page(
  uuid: string,
  title: string,
  options: Partial<Pick<Page, "layout" | "createdAt" | "updatedAt" | "kind">> = {},
): Page {
  return {
    uuid,
    kind: options.kind ?? { kind: "note" },
    title,
    layout: options.layout ?? "outline",
    titleRevision: "0000000000000000-00000000-00000000000000000000000000000001",
    createdAt: options.createdAt ?? 1,
    updatedAt: options.updatedAt ?? 1,
  };
}

const defaults: AllNotesOptions = {
  query: "",
  scope: "all",
  type: "all",
  sort: "updated",
  favoritePageUuids: [],
  recentPageUuids: [],
};

describe("presentAllNotes", () => {
  const pages = [
    page("alpha", "Álpha", { updatedAt: 2 }),
    page("beta", "Beta", { layout: "document", updatedAt: 4 }),
    page("gamma", "Gamma", { updatedAt: 3 }),
    page("journal", "Journal", { kind: { kind: "journal", date: "2026-07-20" } }),
  ];

  it("searches only note titles and applies scope and type filters", () => {
    expect(presentAllNotes(pages, { ...defaults, query: "BETA" }).map((item) => item.uuid)).toEqual(
      ["beta"],
    );
    expect(
      presentAllNotes(pages, {
        ...defaults,
        scope: "favorites",
        type: "outline",
        favoritePageUuids: ["alpha", "beta"],
      }).map((item) => item.uuid),
    ).toEqual(["alpha"]);
  });

  it("sorts recent pages first and falls back to updated time", () => {
    expect(
      presentAllNotes(pages, {
        ...defaults,
        sort: "opened",
        recentPageUuids: ["alpha", "gamma"],
      }).map((item) => item.uuid),
    ).toEqual(["alpha", "gamma", "beta"]);
  });

  it("supports created and natural title ordering", () => {
    const dated = [
      page("ten", "Note 10", { createdAt: 2 }),
      page("two", "Note 2", { createdAt: 3 }),
    ];
    expect(
      presentAllNotes(dated, { ...defaults, sort: "created" }).map((item) => item.uuid),
    ).toEqual(["two", "ten"]);
    expect(presentAllNotes(dated, { ...defaults, sort: "title" }).map((item) => item.uuid)).toEqual(
      ["two", "ten"],
    );
  });
});

it("lists handwritten notes among text notes and keeps journals out", () => {
  const handwritten = page("ink", "Ink", { kind: { kind: "handwriting" } });
  const journal = page("journal", "Journal", { kind: { kind: "journal", date: "2026-07-20" } });
  const listed = presentAllNotes([page("text", "Text"), handwritten, journal], {
    query: "",
    scope: "all",
    type: "all",
    sort: "title",
    favoritePageUuids: [],
    recentPageUuids: [],
  }).map((entry) => entry.uuid);
  expect(listed).toContain("ink");
  expect(listed).toContain("text");
  expect(listed).not.toContain("journal");
});

it("filters handwritten notes as their own type", () => {
  const pages = [page("text", "Text"), page("ink", "Ink", { kind: { kind: "handwriting" } })];
  const options = {
    query: "",
    scope: "all" as const,
    type: "handwriting" as const,
    sort: "title" as const,
    favoritePageUuids: [],
    recentPageUuids: [],
  };
  expect(presentAllNotes(pages, options).map((entry) => entry.uuid)).toEqual(["ink"]);
  expect(
    presentAllNotes(pages, { ...options, type: "outline" }).map((entry) => entry.uuid),
  ).toEqual(["text"]);
});

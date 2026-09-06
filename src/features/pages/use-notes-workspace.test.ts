import { describe, expect, it } from "vite-plus/test";
import type { Page } from "@/lib/api";
import { isNewerPageRecord } from "./use-notes-workspace";

const page = (overrides: Partial<Page> = {}): Page => ({
  uuid: "019f0000-0000-7000-8000-00000000000a",
  kind: { kind: "note" },
  title: "Cached title",
  layout: "outline",
  titleRevision: "revision-1",
  createdAt: 10,
  updatedAt: 20,
  ...overrides,
});

describe("isNewerPageRecord", () => {
  it("seeds the cache when nothing is there", () => {
    expect(isNewerPageRecord(page(), undefined)).toBe(true);
  });

  it("refuses a list record that is older than what the cache holds", () => {
    const cached = page({ title: "Renamed", titleRevision: "revision-2", updatedAt: 30 });
    const stale = page({ title: "Cached title", titleRevision: "revision-1", updatedAt: 20 });

    expect(isNewerPageRecord(stale, cached)).toBe(false);
  });

  it("accepts a record that carries a newer revision", () => {
    const cached = page();
    const fresher = page({ title: "Renamed", titleRevision: "revision-2", updatedAt: 30 });

    expect(isNewerPageRecord(fresher, cached)).toBe(true);
  });

  it("accepts an unchanged revision only when the page is genuinely newer", () => {
    const cached = page();

    expect(isNewerPageRecord(page({ updatedAt: 20 }), cached)).toBe(false);
    expect(isNewerPageRecord(page({ updatedAt: 21 }), cached)).toBe(true);
  });
});

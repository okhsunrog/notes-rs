import { describe, expect, it } from "vite-plus/test";
import { nextRecentPageUuids, toggleFavoritePageUuid } from "./page-navigation-store";

describe("page navigation state", () => {
  it("keeps a strict most-recently-opened order without duplicates", () => {
    expect(nextRecentPageUuids(["one", "two", "three"], "two")).toEqual(["two", "one", "three"]);
  });

  it("caps recents while retaining the page that was just opened", () => {
    const current = Array.from({ length: 20 }, (_, index) => `page-${index}`);
    const next = nextRecentPageUuids(current, "new-page");

    expect(next).toHaveLength(20);
    expect(next[0]).toBe("new-page");
    expect(next).not.toContain("page-19");
  });

  it("adds and removes favorites without changing the order of the others", () => {
    expect(toggleFavoritePageUuid(["one", "two"], "three")).toEqual(["one", "two", "three"]);
    expect(toggleFavoritePageUuid(["one", "two", "three"], "two")).toEqual(["one", "three"]);
  });
});

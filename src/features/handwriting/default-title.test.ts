import { expect, it } from "vitest";
import { defaultHandwritingTitle } from "./default-title";

it("names a new sheet after the local minute it was started", () => {
  expect(defaultHandwritingTitle(new Date(2026, 8, 7, 2, 5))).toBe("Handwriting 2026-09-07 02:05");
});

it("pads every field so the names sort chronologically", () => {
  const early = defaultHandwritingTitle(new Date(2026, 0, 9, 9, 9));
  const later = defaultHandwritingTitle(new Date(2026, 0, 9, 10, 0));
  expect(early).toBe("Handwriting 2026-01-09 09:09");
  expect([later, early].sort()).toEqual([early, later]);
});

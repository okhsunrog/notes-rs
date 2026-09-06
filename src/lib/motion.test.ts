// @vitest-environment jsdom

import { afterEach, expect, it, vi } from "vite-plus/test";
import { scrollBehavior } from "./motion";

const original = window.matchMedia;

afterEach(() => {
  window.matchMedia = original;
});

function stubMatchMedia(matches: boolean) {
  const matchMedia = vi.fn(
    (query: string) => ({ matches, media: query }) as unknown as MediaQueryList,
  );
  window.matchMedia = matchMedia;
  return matchMedia;
}

it("jumps when the user asked for reduced motion", () => {
  const matchMedia = stubMatchMedia(true);
  expect(scrollBehavior()).toBe("auto");
  expect(matchMedia).toHaveBeenCalledWith("(prefers-reduced-motion: reduce)");
});

it("animates when reduced motion is not requested", () => {
  stubMatchMedia(false);
  expect(scrollBehavior()).toBe("smooth");
});

it("falls back to smooth where matchMedia is unavailable", () => {
  Reflect.deleteProperty(window, "matchMedia");
  expect(scrollBehavior()).toBe("smooth");
});

// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { useProgressiveRows } from "./use-progressive-rows";

let count = 0;
let total = 40;
let enabled = true;
let root: ReturnType<typeof createRoot>;
function Probe() {
  count = useProgressiveRows(total, enabled);
  return null;
}
beforeEach(() => {
  vi.useFakeTimers();
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.stubGlobal("requestAnimationFrame", (cb: () => void) => setTimeout(cb, 16));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  total = 40;
  enabled = true;
  root = createRoot(document.createElement("div"));
  act(() => root.render(<Probe />));
});
afterEach(() => {
  act(() => root.unmount());
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
describe("progressive rows", () => {
  it("starts with a skeleton and adds bounded batches after yielding", () => {
    expect(count).toBe(0);
    act(() => {
      vi.advanceTimersByTime(20);
    });
    expect(count).toBe(16);
    act(() => {
      vi.advanceTimersByTime(20);
    });
    expect(count).toBe(32);
    act(() => {
      vi.advanceTimersByTime(20);
    });
    expect(count).toBe(40);
    total = 50;
    act(() => root.render(<Probe />));
    expect(count).toBe(50);
  });
  it("does not limit virtualized or small lists", () => {
    enabled = false;
    act(() => root.render(<Probe />));
    expect(count).toBe(40);
  });
  it("cancels scheduled work when unmounted", () => {
    act(() => root.unmount());
    expect(vi.getTimerCount()).toBe(0);
    root = createRoot(document.createElement("div"));
  });
});

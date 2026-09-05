import { expect, it, vi } from "vite-plus/test";
import { dismissBackOverlay, registerBackOverlay } from "./back-overlays";

it("dismisses only the top overlay and releases registrations on cleanup", () => {
  const parent = vi.fn();
  const child = vi.fn();
  const removeParent = registerBackOverlay(parent);
  const removeChild = registerBackOverlay(child);
  try {
    expect(dismissBackOverlay()).toBe(true);
    expect(child).toHaveBeenCalledOnce();
    expect(parent).not.toHaveBeenCalled();
    removeChild();
    expect(dismissBackOverlay()).toBe(true);
    expect(parent).toHaveBeenCalledOnce();
    removeParent();
    expect(dismissBackOverlay()).toBe(false);
  } finally {
    removeChild();
    removeParent();
  }
});

it("can remove an underlying overlay without removing the top one", () => {
  const removeParent = registerBackOverlay(vi.fn());
  const child = vi.fn();
  const removeChild = registerBackOverlay(child);
  try {
    removeParent();
    removeParent();
    expect(dismissBackOverlay()).toBe(true);
    expect(child).toHaveBeenCalledOnce();
  } finally {
    removeParent();
    removeChild();
  }
  expect(dismissBackOverlay()).toBe(false);
});

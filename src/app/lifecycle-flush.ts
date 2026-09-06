import { completeEveryNote } from "@/features/handwriting/handwriting-session";

/**
 * Process-level drain of work that is queued but not yet stored.
 *
 * A window can go away without unmounting anything — Android kills a
 * backgrounded WebView outright — so the last chance to persist a note must not
 * depend on a component being mounted. This module is deliberately free of
 * React: it is registered once from the entry point and outlives every tree.
 */
let registered = false;

export function registerLifecycleFlush(): void {
  if (registered) return;
  registered = true;
  const drain = () => {
    void completeEveryNote();
  };
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") drain();
  });
  window.addEventListener("pagehide", drain);
}

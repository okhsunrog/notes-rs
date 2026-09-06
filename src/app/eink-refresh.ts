import { requestFullRefresh } from "@/lib/api";

/**
 * Full-panel refreshes for e-ink displays.
 *
 * Partial update modes leave the previous image behind, and it builds up: after a handful of
 * navigations the panel shows a grey shadow of every screen before this one. Only a full refresh
 * clears it, and a full refresh flashes the whole display — so it is asked for once things have
 * settled, never per frame.
 *
 * Deliberately free of React: navigation dispatches and the shared overlay wrappers are the places
 * a screen actually changes, and none of them is a component this could hang off. The resolved
 * display is pushed in from the appearance provider instead of being read from the DOM, so the
 * gate is a plain value in tests.
 */
const SETTLE_MS = 300;

let eink = false;
let pending: ReturnType<typeof setTimeout> | null = null;

/** Called by the appearance provider whenever the resolved display changes. */
export function setEinkRefreshEnabled(enabled: boolean): void {
  eink = enabled;
  if (!eink) cancel();
}

/**
 * Refreshes the panel once the screen stops changing. A burst of changes — a dialog closing into a
 * navigation — collapses into one flash.
 */
export function requestFullRefreshSoon(): void {
  if (!eink) return;
  cancel();
  pending = setTimeout(() => {
    pending = null;
    void requestFullRefresh().catch(() => undefined);
  }, SETTLE_MS);
}

/**
 * Wraps an overlay's `onOpenChange` so closing it schedules a refresh.
 *
 * Dialogs, popovers, menus and selects paint over the page and their partial redraw leaves the
 * sharpest ghost of all. Hooked once in the shared wrappers so no call site has to remember, and
 * shaped as a decorator so the caller's own handler still runs, arguments untouched.
 */
export function refreshPanelAfterClose<Rest extends unknown[]>(
  handler?: (open: boolean, ...rest: Rest) => void,
): (open: boolean, ...rest: Rest) => void {
  return (open, ...rest) => {
    if (!open) requestFullRefreshSoon();
    handler?.(open, ...rest);
  };
}

function cancel(): void {
  if (pending === null) return;
  clearTimeout(pending);
  pending = null;
}

/** Test seam: drops the gate and any refresh still waiting. */
export function resetEinkRefresh(): void {
  eink = false;
  cancel();
}

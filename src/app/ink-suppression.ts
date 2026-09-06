import { useEffect, useId, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * Holding the BOOX firmware pen down while something else needs the screen.
 *
 * The pen is not part of the page: the firmware paints it into a screen region, above every
 * window, and the soft keyboard never takes our window focus. So nothing native can tell that a
 * text field, a dialog or the keyboard itself is in the way — the page has to say so. Each caller
 * names a reason; the plugin keeps the set and only re-arms the pen once it is empty.
 *
 * Deliberately free of React except for the overlay hook, and a no-op wherever the plugin is not
 * there — the wrappers that call it are shared with desktop.
 */
const TEXT_FOCUS = "text-focus";

/** Long enough for a dismissed overlay to be gone from the panel before the pen paints again. */
const OVERLAY_RESUME_MS = 300;

let supported = false;
const held = new Set<string>();

/** Called by the input-capabilities provider: only the BOOX plugin has a pen to suspend. */
export function setInkSuppressionSupported(value: boolean): void {
  if (supported === value) return;
  supported = value;
  // Capabilities resolve after the first render; whatever is already open is pushed now.
  if (supported) for (const reason of held) push(reason, true);
}

export function suppressOnyxInk(reason: string, active: boolean): void {
  if (active) {
    if (held.has(reason)) return;
    held.add(reason);
  } else if (!held.delete(reason)) {
    return;
  }
  push(reason, active);
}

function push(reason: string, active: boolean): void {
  if (!supported) return;
  void invoke("plugin:mobile-system|suppress_onyx_ink", { reason, active }).catch(() => undefined);
}

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.isContentEditable ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLInputElement
  );
}

/**
 * Pauses the pen for as long as the caret is in a text field anywhere in the app.
 *
 * A DOM field raises no Android focus event, so this is the only signal that the keyboard is
 * about to come up — and it arrives before it does, which the inset listener cannot. Focus moving
 * between two fields must not release: `focusout` runs before the next `focusin`, so the release
 * is deferred by a turn and cancelled if the caret lands in another field.
 */
export function installTextFocusSuppression(root: Document = document): () => void {
  let release: ReturnType<typeof setTimeout> | null = null;
  const cancel = () => {
    if (release !== null) clearTimeout(release);
    release = null;
  };
  const focusin = (event: FocusEvent) => {
    cancel();
    suppressOnyxInk(TEXT_FOCUS, isEditable(event.target));
  };
  const focusout = () => {
    cancel();
    release = setTimeout(() => {
      release = null;
      if (!isEditable(root.activeElement)) suppressOnyxInk(TEXT_FOCUS, false);
    }, 0);
  };
  root.addEventListener("focusin", focusin);
  root.addEventListener("focusout", focusout);
  return () => {
    cancel();
    root.removeEventListener("focusin", focusin);
    root.removeEventListener("focusout", focusout);
    suppressOnyxInk(TEXT_FOCUS, false);
  };
}

/**
 * Registers an overlay — dialog, popover, select, menu — as a reason while it is open.
 *
 * Returns the state reporter for the wrapper's `onOpenChange`; an uncontrolled overlay has no
 * other way to say it opened. The reason is dropped on unmount as well as on close, because an
 * overlay whose owner disappears while it is open would otherwise hold the pen for the life of
 * the process.
 */
export function useOverlayInkSuppression(open?: boolean): (open: boolean) => void {
  const reason = `overlay:${useId()}`;
  const [uncontrolled, setUncontrolled] = useState(open ?? false);
  const shown = open ?? uncontrolled;

  useEffect(() => {
    if (shown) {
      suppressOnyxInk(reason, true);
      return;
    }
    const timer = setTimeout(() => suppressOnyxInk(reason, false), OVERLAY_RESUME_MS);
    return () => clearTimeout(timer);
  }, [reason, shown]);
  useEffect(() => () => suppressOnyxInk(reason, false), [reason]);

  return setUncontrolled;
}

/** Test seam: drops the gate and every held reason. */
export function resetInkSuppression(): void {
  supported = false;
  held.clear();
}

import { useEffect } from "react";

type ShortcutActions = {
  enabled: boolean;
  /** Undo/redo only; the handwriting editor owns Ctrl/Cmd+Z for ink history. */
  historyEnabled: boolean;
  createNote: () => void;
  openSearch: () => void;
  undo: () => void;
  redo: () => void;
};

function isEditableTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.matches("input, textarea, [contenteditable=true]");
}

export function useAppShortcuts({
  enabled,
  historyEnabled,
  createNote,
  openSearch,
  undo,
  redo,
}: ShortcutActions) {
  useEffect(() => {
    if (!enabled) return;
    const keydown = (event: KeyboardEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      // Every one of these shortcuts belongs to the text field the caret is in
      // when there is one: Ctrl+N and Ctrl+K are ordinary editing keys there,
      // and stealing them made typing lose characters or open a dialog.
      if (isEditableTarget(event.target)) return;
      const key = event.key.toLocaleLowerCase();
      if (key === "k") {
        event.preventDefault();
        openSearch();
        return;
      }
      if (key === "n") {
        event.preventDefault();
        createNote();
        return;
      }
      if (key === "z" && historyEnabled) {
        event.preventDefault();
        if (event.shiftKey) redo();
        else undo();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [createNote, enabled, historyEnabled, openSearch, redo, undo]);
}

import { useEffect } from "react";

type ShortcutActions = {
  enabled: boolean;
  createNote: () => void;
  openSearch: () => void;
  undo: () => void;
  redo: () => void;
};

export function useAppShortcuts({ enabled, createNote, openSearch, undo, redo }: ShortcutActions) {
  useEffect(() => {
    if (!enabled) return;
    const keydown = (event: KeyboardEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
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
      const target = event.target;
      if (
        key === "z" &&
        !(target instanceof Element && target.matches("input, textarea, [contenteditable=true]"))
      ) {
        event.preventDefault();
        if (event.shiftKey) redo();
        else undo();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [createNote, enabled, openSearch, redo, undo]);
}

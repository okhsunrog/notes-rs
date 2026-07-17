export type DocumentHistoryAction = "undo" | "redo";

export type DocumentEditorKey = Readonly<{
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey?: boolean;
  isComposing?: boolean;
}>;

/** Keys consumed by the focused CodeMirror before app-level history sees them. */
export function resolveDocumentHistoryKey(event: DocumentEditorKey): DocumentHistoryAction | null {
  if (event.isComposing || event.altKey || (!event.ctrlKey && !event.metaKey)) return null;
  const key = event.key.toLowerCase();
  if (key === "z") return event.shiftKey ? "redo" : "undo";
  if (key === "y" && !event.shiftKey) return "redo";
  return null;
}

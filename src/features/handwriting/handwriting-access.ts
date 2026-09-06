export type EditorOwnership = "pending" | "owned" | "taken";

/** Why the editor is not accepting input yet, or that it is. */
export type EditorAccess = "editable" | "pending" | "other_pane" | "no_input";

/**
 * Drawing needs a pen or the explicit mouse preference. Opening a note never
 * does: an existing note is readable on any device.
 */
export function canDrawHandwriting(available: boolean, mouseEnabled: boolean): boolean {
  return available || mouseEnabled;
}

export function editorAccess(ownership: EditorOwnership, canDraw: boolean): EditorAccess {
  if (ownership === "taken") return "other_pane";
  if (!canDraw) return "no_input";
  return ownership === "owned" ? "editable" : "pending";
}

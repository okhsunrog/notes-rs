import type { PaneId } from "./workspace-model";

/**
 * A pane's content may need a word before the pane navigates away from it: the handwriting
 * editor writes its last gestures first and refuses to leave while a save is failing. The guard
 * answers whether navigation may proceed; the caller then navigates as it would have.
 *
 * Kept outside React so the pane frame, the hardware Back key and the editor can share it
 * without threading callbacks through the workbench.
 */
export type PaneLeaveGuard = () => Promise<boolean> | boolean;

const guards = new Map<PaneId, PaneLeaveGuard>();

export function registerPaneLeaveGuard(paneId: PaneId, guard: PaneLeaveGuard): () => void {
  guards.set(paneId, guard);
  return () => {
    if (guards.get(paneId) === guard) guards.delete(paneId);
  };
}

/** True when nothing objects, or when the pane has no guard. A throwing guard keeps the pane. */
export async function mayLeavePane(paneId: PaneId): Promise<boolean> {
  const guard = guards.get(paneId);
  if (!guard) return true;
  try {
    return await guard();
  } catch {
    return false;
  }
}

/** Test seam. */
export function resetPaneLeaveGuards(): void {
  guards.clear();
}

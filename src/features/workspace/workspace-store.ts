import { create } from "zustand";
import {
  createInitialWorkspaceState,
  type WorkspaceAction,
  type WorkspaceState,
  workspaceReducer,
} from "@/features/workspace/workspace-model";
import {
  loadWorkspaceSession,
  persistWorkspaceSession,
} from "@/features/workspace/workspace-session";
import { noteNavigation } from "@/app/eink-refresh";

/** Actions that put a different screen in front of the user. */
const NAVIGATIONS: ReadonlySet<WorkspaceAction["type"]> = new Set([
  "open_target",
  "go_back",
  "go_forward",
  "close_pane",
  "show_compact_pane",
]);

export type WorkspaceStore = WorkspaceState & {
  dispatch: (action: WorkspaceAction) => void;
};

export const useWorkspaceStore = create<WorkspaceStore>()((set) => ({
  ...createInitialWorkspaceState(),
  dispatch: (action: WorkspaceAction) =>
    set((state: WorkspaceStore) => {
      const next = workspaceReducer(state, action);
      persistWorkspaceSession(next);
      // Every partial e-ink update leaves the old screen faintly behind; a navigation is where
      // that becomes visible, so the panel is cleaned once the moving around stops.
      if (NAVIGATIONS.has(action.type) && next !== state) noteNavigation();
      return next;
    }),
}));

export function restoreWorkspaceSession() {
  const session = loadWorkspaceSession();
  if (!session) return false;
  useWorkspaceStore.setState(session);
  return true;
}

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

export type WorkspaceStore = WorkspaceState & {
  dispatch: (action: WorkspaceAction) => void;
};

export const useWorkspaceStore = create<WorkspaceStore>()((set) => ({
  ...createInitialWorkspaceState(),
  dispatch: (action: WorkspaceAction) =>
    set((state: WorkspaceStore) => {
      const next = workspaceReducer(state, action);
      persistWorkspaceSession(next);
      return next;
    }),
}));

export function restoreWorkspaceSession() {
  const session = loadWorkspaceSession();
  if (!session) return false;
  useWorkspaceStore.setState(session);
  return true;
}

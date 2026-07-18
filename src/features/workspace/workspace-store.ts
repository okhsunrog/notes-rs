import { create } from "zustand";
import {
  createInitialWorkspaceState,
  type WorkspaceAction,
  type WorkspaceState,
  workspaceReducer,
} from "@/features/workspace/workspace-model";

export type WorkspaceStore = WorkspaceState & {
  dispatch: (action: WorkspaceAction) => void;
};

export const useWorkspaceStore = create<WorkspaceStore>()((set) => ({
  ...createInitialWorkspaceState(),
  dispatch: (action: WorkspaceAction) =>
    set((state: WorkspaceStore) => workspaceReducer(state, action)),
}));

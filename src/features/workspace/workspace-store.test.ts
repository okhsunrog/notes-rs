import { beforeEach, describe, expect, it } from "vitest";
import { currentDisposition, pageTarget } from "./workspace-model";
import { createInitialWorkspaceState } from "./workspace-model";
import { type WorkspaceStore, useWorkspaceStore } from "./workspace-store";

const resetStore = () => {
  useWorkspaceStore.setState((state: WorkspaceStore) => ({
    ...createInitialWorkspaceState(),
    dispatch: state.dispatch,
  }));
};

describe("workspace-store", () => {
  beforeEach(() => {
    resetStore();
  });

  it("opens a page into the active pane", () => {
    useWorkspaceStore.getState().dispatch({
      type: "open_target",
      target: pageTarget("target-page"),
      disposition: currentDisposition,
    });

    const state = useWorkspaceStore.getState();
    expect(state.panes[state.activePaneId].content).toMatchObject({
      kind: "page",
      pageUuid: "target-page",
    });
  });

  it("restores initial workspace after reset", () => {
    useWorkspaceStore.getState().dispatch({
      type: "open_target",
      target: pageTarget("target-page"),
      disposition: currentDisposition,
    });
    useWorkspaceStore.getState().dispatch({ type: "reset" });

    const state = useWorkspaceStore.getState();
    const initial = createInitialWorkspaceState();
    expect(state).toMatchObject({
      tree: initial.tree,
      panes: initial.panes,
      primaryPaneId: initial.primaryPaneId,
      activePaneId: initial.activePaneId,
      compactVisiblePaneId: initial.compactVisiblePaneId,
      nextPaneOrdinal: initial.nextPaneOrdinal,
      nextSplitOrdinal: initial.nextSplitOrdinal,
      assistantDock: initial.assistantDock,
    });
  });

  it("keeps a dispatch function in state", () => {
    expect(typeof useWorkspaceStore.getState().dispatch).toBe("function");
  });
});

import { beforeEach, describe, expect, it } from "vite-plus/test";
import { createInitialWorkspaceState, currentDisposition, pageTarget } from "./workspace-model";
import { useWorkspaceStore } from "./workspace-store";

describe("workspace-store", () => {
  beforeEach(() => {
    // setState shallow-merges, so this resets the workspace fields while keeping dispatch.
    useWorkspaceStore.setState(createInitialWorkspaceState());
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
});

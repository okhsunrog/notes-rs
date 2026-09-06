import { describe, expect, it } from "vitest";
import { PagePresentation } from "@/features/pages/page-presentation";
import {
  DockProjection,
  DockVisibility,
  OpenDispositionKind,
  PaneContentKind,
  SplitAxis,
  WorkspaceNodeKind,
  adjacentDisposition,
  allNotesTarget,
  createInitialWorkspaceState,
  getActivePane,
  leafPaneIds,
  pageTarget,
  projectDock,
  validateWorkspaceState,
  visiblePaneIds,
  workspaceReducer,
  type WorkspaceNode,
} from "./workspace-model";

describe("window-local workspace model", () => {
  it("creates one valid primary leaf", () => {
    const state = createInitialWorkspaceState();

    expect(state.tree.kind).toBe(WorkspaceNodeKind.Pane);
    expect(leafPaneIds(state.tree)).toEqual([state.primaryPaneId]);
    expect(validateWorkspaceState(state)).toEqual([]);
  });

  it("opens the All Notes catalog as a history-aware pane target", () => {
    const initial = createInitialWorkspaceState();
    const opened = workspaceReducer(initial, {
      type: "open_target",
      target: allNotesTarget,
      disposition: { kind: OpenDispositionKind.Current },
    });

    expect(getActivePane(opened).content).toEqual({ kind: PaneContentKind.AllNotes });
    expect(getActivePane(opened).back).toEqual([{ kind: PaneContentKind.Home }]);
    expect(validateWorkspaceState(opened)).toEqual([]);
  });

  it("detects cyclic trees instead of recursing forever", () => {
    const state = createInitialWorkspaceState();
    const cycle = {
      kind: WorkspaceNodeKind.Split,
      splitId: "cycle" as never,
      axis: SplitAxis.Horizontal,
      ratio: 0.5,
      first: state.tree,
      second: state.tree,
    } as Extract<WorkspaceNode, { kind: WorkspaceNodeKind.Split }>;
    (cycle as { first: WorkspaceNode }).first = cycle;

    expect(
      validateWorkspaceState({
        ...state,
        tree: cycle,
      }),
    ).toContain("pane tree contains a cycle");
  });

  it("opens adjacent without replacing primary and caps the tree at two leaves", () => {
    let state = createInitialWorkspaceState();
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("page-a"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("page-b"),
      disposition: adjacentDisposition,
    });
    const adjacent = state.activePaneId;

    expect(state.tree.kind).toBe(WorkspaceNodeKind.Split);
    expect(getActivePane(state).content).toMatchObject({
      kind: PaneContentKind.Page,
      pageUuid: "page-b",
    });
    expect(state.panes[state.primaryPaneId].content).toMatchObject({ pageUuid: "page-a" });

    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("page-c"),
      disposition: adjacentDisposition,
    });
    expect(state.activePaneId).toBe(adjacent);
    expect(leafPaneIds(state.tree)).toHaveLength(2);
    expect(state.panes[state.primaryPaneId].content).toMatchObject({ pageUuid: "page-a" });
    expect(getActivePane(state).content).toMatchObject({ pageUuid: "page-c" });
    expect(validateWorkspaceState(state)).toEqual([]);
  });

  it("coerces the same page opened beside its editor to Reading", () => {
    let state = createInitialWorkspaceState();
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("same-page"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("same-page"),
      disposition: adjacentDisposition,
    });

    expect(state.panes[state.primaryPaneId].content).toMatchObject({
      pageUuid: "same-page",
      presentation: PagePresentation.Editing,
    });
    expect(getActivePane(state).content).toMatchObject({
      pageUuid: "same-page",
      presentation: PagePresentation.Reading,
    });
    expect(validateWorkspaceState(state)).toEqual([]);
  });

  it("does not promote a duplicate Reading surface to a second editor", () => {
    let state = createInitialWorkspaceState();
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("same-page"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("same-page"),
      disposition: adjacentDisposition,
    });
    const readingPane = state.activePaneId;

    state = workspaceReducer(state, {
      type: "set_page_presentation",
      paneId: readingPane,
      presentation: PagePresentation.Editing,
    });

    expect(state.panes[readingPane].content).toMatchObject({
      pageUuid: "same-page",
      presentation: PagePresentation.Reading,
    });
    expect(validateWorkspaceState(state)).toEqual([]);
  });

  it("normalizes duplicate editors restored through pane back and forward history", () => {
    let backState = createInitialWorkspaceState();
    backState = workspaceReducer(backState, {
      type: "open_target",
      target: pageTarget("primary-b"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    backState = workspaceReducer(backState, {
      type: "open_target",
      target: pageTarget("shared"),
      disposition: adjacentDisposition,
    });
    const backPane = backState.activePaneId;
    backState = workspaceReducer(backState, {
      type: "open_target",
      target: pageTarget("adjacent-c"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    backState = workspaceReducer(backState, {
      type: "focus_pane",
      paneId: backState.primaryPaneId,
    });
    backState = workspaceReducer(backState, {
      type: "open_target",
      target: pageTarget("shared"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    backState = workspaceReducer(backState, { type: "go_back", paneId: backPane });

    expect(backState.panes[backPane].content).toMatchObject({
      pageUuid: "shared",
      presentation: PagePresentation.Reading,
    });
    expect(validateWorkspaceState(backState)).toEqual([]);

    let forwardState = createInitialWorkspaceState();
    forwardState = workspaceReducer(forwardState, {
      type: "open_target",
      target: pageTarget("primary-b"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    forwardState = workspaceReducer(forwardState, {
      type: "open_target",
      target: pageTarget("adjacent-c"),
      disposition: adjacentDisposition,
    });
    const forwardPane = forwardState.activePaneId;
    forwardState = workspaceReducer(forwardState, {
      type: "open_target",
      target: pageTarget("shared"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    forwardState = workspaceReducer(forwardState, { type: "go_back", paneId: forwardPane });
    forwardState = workspaceReducer(forwardState, {
      type: "focus_pane",
      paneId: forwardState.primaryPaneId,
    });
    forwardState = workspaceReducer(forwardState, {
      type: "open_target",
      target: pageTarget("shared"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    forwardState = workspaceReducer(forwardState, {
      type: "go_forward",
      paneId: forwardPane,
    });

    expect(forwardState.panes[forwardPane].content).toMatchObject({
      pageUuid: "shared",
      presentation: PagePresentation.Reading,
    });
    expect(validateWorkspaceState(forwardState)).toEqual([]);
  });

  it("allows different pages to remain writable", () => {
    let state = createInitialWorkspaceState();
    for (const [pageUuid, disposition] of [
      ["page-a", { kind: OpenDispositionKind.Current }],
      ["page-b", adjacentDisposition],
    ] as const) {
      state = workspaceReducer(state, {
        type: "open_target",
        target: pageTarget(pageUuid),
        disposition,
      });
    }

    expect(leafPaneIds(state.tree).map((paneId) => state.panes[paneId].content)).toEqual([
      expect.objectContaining({ presentation: PagePresentation.Editing }),
      expect.objectContaining({ presentation: PagePresentation.Editing }),
    ]);
  });

  it("keeps independent pane back/forward history and focuses after close", () => {
    let state = createInitialWorkspaceState();
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("primary"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("adjacent"),
      disposition: adjacentDisposition,
    });
    const adjacent = state.activePaneId;
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("adjacent-next"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    state = workspaceReducer(state, { type: "go_back", paneId: adjacent });
    expect(getActivePane(state).content).toMatchObject({ pageUuid: "adjacent" });

    state = workspaceReducer(state, { type: "close_pane", paneId: adjacent });
    expect(state.activePaneId).toBe(state.primaryPaneId);
    expect(leafPaneIds(state.tree)).toEqual([state.primaryPaneId]);
    expect(validateWorkspaceState(state)).toEqual([]);
  });

  it("projects compact mode without mutating the recursive tree", () => {
    let state = createInitialWorkspaceState();
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("beside"),
      disposition: {
        kind: OpenDispositionKind.Adjacent,
        axis: SplitAxis.Horizontal,
      },
    });
    const tree = state.tree;

    expect(visiblePaneIds(state, true)).toEqual([state.activePaneId]);
    expect(visiblePaneIds(state, false)).toEqual(leafPaneIds(tree));
    expect(state.tree).toBe(tree);
  });

  it("leaves no trace of a deleted page in any pane's history", () => {
    let state = createInitialWorkspaceState();
    for (const uuid of ["page-a", "page-b"]) {
      state = workspaceReducer(state, {
        type: "open_target",
        target: pageTarget(uuid),
        disposition: { kind: OpenDispositionKind.Current },
      });
    }

    state = workspaceReducer(state, { type: "forget_page", pageUuid: "page-b" });

    const pane = getActivePane(state);
    expect(pane.content).toEqual({ kind: PaneContentKind.Home });
    // Going back must reach the page the user actually came from, not the home
    // view that replaced the deleted one.
    const back = workspaceReducer(state, { type: "go_back", paneId: pane.id });
    expect(getActivePane(back).content).toMatchObject({
      kind: PaneContentKind.Page,
      pageUuid: "page-a",
    });

    const entries = Object.values(state.panes).flatMap((record) => [
      ...record.back,
      ...record.forward,
      record.content,
    ]);
    expect(
      entries.some((entry) => entry.kind === PaneContentKind.Page && entry.pageUuid === "page-b"),
    ).toBe(false);
    expect(validateWorkspaceState(state)).toEqual([]);
  });

  it("prunes a deleted page from panes that are not showing it", () => {
    let state = createInitialWorkspaceState();
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("page-b"),
      disposition: { kind: OpenDispositionKind.Current },
    });
    state = workspaceReducer(state, {
      type: "open_target",
      target: pageTarget("page-c"),
      disposition: { kind: OpenDispositionKind.Current },
    });

    state = workspaceReducer(state, { type: "forget_page", pageUuid: "page-b" });

    const pane = getActivePane(state);
    expect(pane.content).toMatchObject({ kind: PaneContentKind.Page, pageUuid: "page-c" });
    expect(pane.back).toEqual([{ kind: PaneContentKind.Home }]);
  });

  it("projects every dock visibility without rewriting dock state", () => {
    expect(projectDock(DockVisibility.Hidden, false)).toBe(DockProjection.Hidden);
    expect(projectDock(DockVisibility.Rail, false)).toBe(DockProjection.Rail);
    expect(projectDock(DockVisibility.Open, false)).toBe(DockProjection.Panel);
    expect(projectDock(DockVisibility.Rail, true)).toBe(DockProjection.CompactSurface);
    expect(projectDock(DockVisibility.Open, true)).toBe(DockProjection.CompactSurface);
  });
});

import { PagePresentation } from "@/features/pages/page-presentation";
import type { JournalDate } from "@/lib/api";

declare const paneIdBrand: unique symbol;
declare const splitIdBrand: unique symbol;

export type PaneId = string & { readonly [paneIdBrand]: true };
export type SplitId = string & { readonly [splitIdBrand]: true };

export enum WorkspaceNodeKind {
  Pane = "pane",
  Split = "split",
}

export enum SplitAxis {
  Horizontal = "horizontal",
  Vertical = "vertical",
}

export enum PaneContentKind {
  Home = "home",
  Page = "page",
  Graph = "graph",
  JournalDay = "journal_day",
}

export enum OpenDispositionKind {
  Current = "current",
  Adjacent = "adjacent",
  NewNativeWindow = "new_native_window",
}

export enum DockVisibility {
  Hidden = "hidden",
  Rail = "rail",
  Open = "open",
}

export enum DockProjection {
  Hidden = "hidden",
  Rail = "rail",
  Panel = "panel",
  CompactSurface = "compact_surface",
}

export type PaneContent =
  | { readonly kind: PaneContentKind.Home }
  | {
      readonly kind: PaneContentKind.Page;
      readonly pageUuid: string;
      readonly blockUuid: string | null;
      readonly presentation: PagePresentation;
    }
  | {
      readonly kind: PaneContentKind.Graph;
      readonly focusPageUuid: string | null;
    }
  | {
      readonly kind: PaneContentKind.JournalDay;
      readonly date: JournalDate;
    };

export type OpenTarget = PaneContent;

export type OpenDisposition =
  | { readonly kind: OpenDispositionKind.Current }
  | {
      readonly kind: OpenDispositionKind.Adjacent;
      readonly axis?: SplitAxis;
    }
  | { readonly kind: OpenDispositionKind.NewNativeWindow };

export type WorkspaceNode =
  | {
      readonly kind: WorkspaceNodeKind.Pane;
      readonly paneId: PaneId;
    }
  | {
      readonly kind: WorkspaceNodeKind.Split;
      readonly splitId: SplitId;
      readonly axis: SplitAxis;
      readonly ratio: number;
      readonly first: WorkspaceNode;
      readonly second: WorkspaceNode;
    };

export type PaneRecord = Readonly<{
  id: PaneId;
  content: PaneContent;
  back: readonly PaneContent[];
  forward: readonly PaneContent[];
}>;

export type AssistantDockState = Readonly<{
  visibility: DockVisibility;
  width: number;
}>;

export type WorkspaceState = Readonly<{
  tree: WorkspaceNode;
  panes: Readonly<Record<string, PaneRecord>>;
  primaryPaneId: PaneId;
  activePaneId: PaneId;
  compactVisiblePaneId: PaneId;
  nextPaneOrdinal: number;
  nextSplitOrdinal: number;
  assistantDock: AssistantDockState;
}>;

export type WorkspaceAction =
  | {
      readonly type: "open_target";
      readonly target: OpenTarget;
      readonly disposition: OpenDisposition;
    }
  | { readonly type: "focus_pane"; readonly paneId: PaneId }
  | { readonly type: "show_compact_pane"; readonly paneId: PaneId }
  | { readonly type: "close_pane"; readonly paneId: PaneId }
  | { readonly type: "go_back"; readonly paneId: PaneId }
  | { readonly type: "go_forward"; readonly paneId: PaneId }
  | {
      readonly type: "set_page_presentation";
      readonly paneId: PaneId;
      readonly presentation: PagePresentation;
    }
  | { readonly type: "forget_page"; readonly pageUuid: string }
  | { readonly type: "resize_split"; readonly splitId: SplitId; readonly ratio: number }
  | { readonly type: "set_dock_visibility"; readonly visibility: DockVisibility }
  | { readonly type: "set_dock_width"; readonly width: number }
  | { readonly type: "reset" };

export const currentDisposition: OpenDisposition = {
  kind: OpenDispositionKind.Current,
};

export const adjacentDisposition: OpenDisposition = {
  kind: OpenDispositionKind.Adjacent,
  axis: SplitAxis.Horizontal,
};

/** Converts a DOM modifier at the component boundary; events never enter navigation state. */
export function dispositionFromShiftKey(shiftKey: boolean): OpenDisposition {
  return shiftKey ? adjacentDisposition : currentDisposition;
}

export const homeTarget: OpenTarget = { kind: PaneContentKind.Home };

export function pageTarget(
  pageUuid: string,
  options: Readonly<{
    blockUuid?: string | null;
    presentation?: PagePresentation;
  }> = {},
): OpenTarget {
  return {
    kind: PaneContentKind.Page,
    pageUuid,
    blockUuid: options.blockUuid ?? null,
    presentation: options.presentation ?? PagePresentation.Editing,
  };
}

export function graphTarget(focusPageUuid: string | null): OpenTarget {
  return { kind: PaneContentKind.Graph, focusPageUuid };
}

export function journalDayTarget(date: JournalDate): OpenTarget {
  return { kind: PaneContentKind.JournalDay, date };
}

export function createInitialWorkspaceState(): WorkspaceState {
  const primaryPaneId = paneId(1);
  return {
    tree: { kind: WorkspaceNodeKind.Pane, paneId: primaryPaneId },
    panes: {
      [primaryPaneId]: createPane(primaryPaneId, homeTarget),
    },
    primaryPaneId,
    activePaneId: primaryPaneId,
    compactVisiblePaneId: primaryPaneId,
    nextPaneOrdinal: 2,
    nextSplitOrdinal: 1,
    assistantDock: {
      visibility: DockVisibility.Open,
      width: 360,
    },
  };
}

export function workspaceReducer(state: WorkspaceState, action: WorkspaceAction): WorkspaceState {
  switch (action.type) {
    case "open_target":
      return openTarget(state, action.target, action.disposition);
    case "focus_pane":
      return hasPane(state, action.paneId)
        ? { ...state, activePaneId: action.paneId, compactVisiblePaneId: action.paneId }
        : state;
    case "show_compact_pane":
      return hasPane(state, action.paneId)
        ? { ...state, compactVisiblePaneId: action.paneId, activePaneId: action.paneId }
        : state;
    case "close_pane":
      return closePane(state, action.paneId);
    case "go_back":
      return moveHistory(state, action.paneId, "back");
    case "go_forward":
      return moveHistory(state, action.paneId, "forward");
    case "set_page_presentation":
      return setPagePresentation(state, action.paneId, action.presentation);
    case "forget_page":
      return forgetPage(state, action.pageUuid);
    case "resize_split":
      return {
        ...state,
        tree: updateSplitRatio(state.tree, action.splitId, action.ratio),
      };
    case "set_dock_visibility":
      return {
        ...state,
        assistantDock: { ...state.assistantDock, visibility: action.visibility },
      };
    case "set_dock_width":
      return {
        ...state,
        assistantDock: { ...state.assistantDock, width: clampDockWidth(action.width) },
      };
    case "reset":
      return createInitialWorkspaceState();
  }
}

export function leafPaneIds(node: WorkspaceNode): readonly PaneId[] {
  if (node.kind === WorkspaceNodeKind.Pane) return [node.paneId];
  return [...leafPaneIds(node.first), ...leafPaneIds(node.second)];
}

export function visiblePaneIds(state: WorkspaceState, compact: boolean): readonly PaneId[] {
  return compact ? [state.compactVisiblePaneId] : leafPaneIds(state.tree);
}

export function projectDock(visibility: DockVisibility, compact: boolean): DockProjection {
  if (visibility === DockVisibility.Hidden) return DockProjection.Hidden;
  if (compact) return DockProjection.CompactSurface;
  return visibility === DockVisibility.Rail ? DockProjection.Rail : DockProjection.Panel;
}

export function getPane(state: WorkspaceState, paneId: PaneId): PaneRecord {
  const pane = state.panes[paneId];
  if (!pane) throw new Error(`workspace pane ${paneId} is missing`);
  return pane;
}

export function getActivePane(state: WorkspaceState): PaneRecord {
  return getPane(state, state.activePaneId);
}

/** Returns invariant errors without mutating or repairing window-local state. */
export function validateWorkspaceState(state: WorkspaceState): readonly string[] {
  const errors: string[] = [];
  const leaves: PaneId[] = [];
  const visitedNodes = new WeakSet<object>();
  const splitIds = new Set<SplitId>();
  const visit = (node: WorkspaceNode) => {
    if (visitedNodes.has(node)) {
      errors.push("pane tree contains a cycle");
      return;
    }
    visitedNodes.add(node);
    if (node.kind === WorkspaceNodeKind.Pane) {
      leaves.push(node.paneId);
      return;
    }
    if (splitIds.has(node.splitId)) errors.push(`duplicate split id ${node.splitId}`);
    splitIds.add(node.splitId);
    visit(node.first);
    visit(node.second);
  };
  visit(state.tree);
  const uniqueLeaves = new Set(leaves);
  if (leaves.length !== uniqueLeaves.size) errors.push("pane tree contains duplicate leaves");
  if (leaves.length === 0 || leaves.length > 2)
    errors.push("pane tree must contain one or two leaves");
  for (const paneId of leaves) {
    if (!state.panes[paneId]) errors.push(`pane tree references missing pane ${paneId}`);
  }
  for (const paneId of Object.keys(state.panes)) {
    if (!uniqueLeaves.has(paneId as PaneId))
      errors.push(`pane record ${paneId} is not a tree leaf`);
  }
  if (!uniqueLeaves.has(state.primaryPaneId)) errors.push("primary pane is not a tree leaf");
  if (!uniqueLeaves.has(state.activePaneId)) errors.push("active pane is not a tree leaf");
  if (!uniqueLeaves.has(state.compactVisiblePaneId)) {
    errors.push("compact visible pane is not a tree leaf");
  }

  const writablePages = new Set<string>();
  for (const paneId of leaves) {
    const content = state.panes[paneId]?.content;
    if (content?.kind !== PaneContentKind.Page) continue;
    if (content.presentation !== PagePresentation.Editing) continue;
    if (writablePages.has(content.pageUuid)) {
      errors.push(`page ${content.pageUuid} has more than one writable pane`);
    }
    writablePages.add(content.pageUuid);
  }
  return errors;
}

function openTarget(
  state: WorkspaceState,
  requestedTarget: OpenTarget,
  disposition: OpenDisposition,
): WorkspaceState {
  if (disposition.kind === OpenDispositionKind.NewNativeWindow) return state;

  if (disposition.kind === OpenDispositionKind.Current) {
    return navigatePane(state, state.activePaneId, requestedTarget);
  }

  const leaves = leafPaneIds(state.tree);
  const existingAdjacent = leaves.find((id) => id !== state.primaryPaneId);
  if (existingAdjacent) {
    const destination =
      state.activePaneId === state.primaryPaneId ? existingAdjacent : state.activePaneId;
    return navigatePane(state, destination, requestedTarget);
  }

  const newPaneId = paneId(state.nextPaneOrdinal);
  const target = enforceSingleWritablePage(state, newPaneId, requestedTarget);
  const splitId = makeSplitId(state.nextSplitOrdinal);
  return {
    ...state,
    tree: {
      kind: WorkspaceNodeKind.Split,
      splitId,
      axis: disposition.axis ?? SplitAxis.Horizontal,
      ratio: 0.5,
      first: state.tree,
      second: { kind: WorkspaceNodeKind.Pane, paneId: newPaneId },
    },
    panes: {
      ...state.panes,
      [newPaneId]: createPane(newPaneId, target),
    },
    activePaneId: newPaneId,
    compactVisiblePaneId: newPaneId,
    nextPaneOrdinal: state.nextPaneOrdinal + 1,
    nextSplitOrdinal: state.nextSplitOrdinal + 1,
  };
}

function navigatePane(
  state: WorkspaceState,
  destination: PaneId,
  requestedTarget: OpenTarget,
): WorkspaceState {
  const pane = getPane(state, destination);
  const target = enforceSingleWritablePage(state, destination, requestedTarget);
  if (paneContentEquals(pane.content, target)) {
    return { ...state, activePaneId: destination, compactVisiblePaneId: destination };
  }
  return {
    ...state,
    panes: {
      ...state.panes,
      [destination]: {
        ...pane,
        content: target,
        back: [...pane.back, pane.content],
        forward: [],
      },
    },
    activePaneId: destination,
    compactVisiblePaneId: destination,
  };
}

function enforceSingleWritablePage(
  state: WorkspaceState,
  destination: PaneId,
  target: OpenTarget,
): OpenTarget {
  if (target.kind !== PaneContentKind.Page) return target;
  if (target.presentation !== PagePresentation.Editing) return target;
  const duplicateEditor = leafPaneIds(state.tree).some((paneId) => {
    if (paneId === destination) return false;
    const content = state.panes[paneId]?.content;
    return (
      content?.kind === PaneContentKind.Page &&
      content.pageUuid === target.pageUuid &&
      content.presentation === PagePresentation.Editing
    );
  });
  return duplicateEditor ? { ...target, presentation: PagePresentation.Reading } : target;
}

function closePane(state: WorkspaceState, target: PaneId): WorkspaceState {
  if (!hasPane(state, target)) return state;
  if (target === state.primaryPaneId) return navigatePane(state, target, homeTarget);

  const tree = removePane(state.tree, target);
  if (!tree) return state;
  const panes = { ...state.panes };
  delete panes[target];
  const activePaneId = state.activePaneId === target ? state.primaryPaneId : state.activePaneId;
  const compactVisiblePaneId =
    state.compactVisiblePaneId === target ? activePaneId : state.compactVisiblePaneId;
  return { ...state, tree, panes, activePaneId, compactVisiblePaneId };
}

function setPagePresentation(
  state: WorkspaceState,
  paneId: PaneId,
  presentation: PagePresentation,
): WorkspaceState {
  const pane = state.panes[paneId];
  if (!pane || pane.content.kind !== PaneContentKind.Page) return state;
  const content = enforceSingleWritablePage(state, paneId, {
    ...pane.content,
    presentation,
  });
  return {
    ...state,
    panes: { ...state.panes, [paneId]: { ...pane, content } },
  };
}

function forgetPage(state: WorkspaceState, pageUuid: string): WorkspaceState {
  let next = state;
  for (const paneId of leafPaneIds(state.tree)) {
    const content = next.panes[paneId]?.content;
    const referencesPage =
      (content?.kind === PaneContentKind.Page && content.pageUuid === pageUuid) ||
      (content?.kind === PaneContentKind.Graph && content.focusPageUuid === pageUuid);
    if (!referencesPage) continue;
    next =
      paneId === next.primaryPaneId
        ? navigatePane(next, paneId, homeTarget)
        : closePane(next, paneId);
  }
  return next;
}

function moveHistory(
  state: WorkspaceState,
  paneId: PaneId,
  direction: "back" | "forward",
): WorkspaceState {
  if (!hasPane(state, paneId)) return state;
  const pane = getPane(state, paneId);
  const source = direction === "back" ? pane.back : pane.forward;
  const target = source[source.length - 1];
  if (!target) return state;
  const normalized = enforceSingleWritablePage(state, paneId, target);
  const nextPane: PaneRecord =
    direction === "back"
      ? {
          ...pane,
          content: normalized,
          back: source.slice(0, -1),
          forward: [...pane.forward, pane.content],
        }
      : {
          ...pane,
          content: normalized,
          back: [...pane.back, pane.content],
          forward: source.slice(0, -1),
        };
  return {
    ...state,
    panes: { ...state.panes, [paneId]: nextPane },
    activePaneId: paneId,
    compactVisiblePaneId: paneId,
  };
}

function removePane(node: WorkspaceNode, paneToRemove: PaneId): WorkspaceNode | null {
  if (node.kind === WorkspaceNodeKind.Pane) {
    return node.paneId === paneToRemove ? null : node;
  }
  const first = removePane(node.first, paneToRemove);
  const second = removePane(node.second, paneToRemove);
  if (!first) return second;
  if (!second) return first;
  return { ...node, first, second };
}

function updateSplitRatio(node: WorkspaceNode, splitId: SplitId, ratio: number): WorkspaceNode {
  if (node.kind === WorkspaceNodeKind.Pane) return node;
  if (node.splitId === splitId) return { ...node, ratio: clampRatio(ratio) };
  return {
    ...node,
    first: updateSplitRatio(node.first, splitId, ratio),
    second: updateSplitRatio(node.second, splitId, ratio),
  };
}

function createPane(id: PaneId, content: PaneContent): PaneRecord {
  return { id, content, back: [], forward: [] };
}

function paneContentEquals(left: PaneContent, right: PaneContent): boolean {
  if (left.kind !== right.kind) return false;
  switch (left.kind) {
    case PaneContentKind.Home:
      return true;
    case PaneContentKind.Page:
      return (
        right.kind === PaneContentKind.Page &&
        left.pageUuid === right.pageUuid &&
        left.blockUuid === right.blockUuid &&
        left.presentation === right.presentation
      );
    case PaneContentKind.Graph:
      return right.kind === PaneContentKind.Graph && left.focusPageUuid === right.focusPageUuid;
    case PaneContentKind.JournalDay:
      return right.kind === PaneContentKind.JournalDay && left.date === right.date;
  }
}

function hasPane(state: WorkspaceState, paneId: PaneId) {
  return state.panes[paneId] !== undefined;
}

function paneId(ordinal: number): PaneId {
  return `pane-${ordinal}` as PaneId;
}

function makeSplitId(ordinal: number): SplitId {
  return `split-${ordinal}` as SplitId;
}

function clampRatio(ratio: number) {
  return Math.max(0.25, Math.min(ratio, 0.75));
}

function clampDockWidth(width: number) {
  return Math.max(320, Math.min(width, 480));
}

import {
  DockVisibility,
  PaneContentKind,
  SplitAxis,
  WorkspaceNodeKind,
  validateWorkspaceState,
  type PaneContent,
  type WorkspaceNode,
  type WorkspaceState,
} from "./workspace-model";

const SESSION_KEY = "notes-rs:workspace-session:v1";
const SESSION_VERSION = 1;

type StoredWorkspaceSession = {
  version: typeof SESSION_VERSION;
  state: WorkspaceState;
};

export function encodeWorkspaceSession(state: WorkspaceState) {
  return JSON.stringify({ version: SESSION_VERSION, state } satisfies StoredWorkspaceSession);
}

export function decodeWorkspaceSession(value: string): WorkspaceState | null {
  try {
    const parsed: unknown = JSON.parse(value);
    if (!isRecord(parsed) || parsed.version !== SESSION_VERSION) return null;
    const state = parsed.state;
    if (!isWorkspaceState(state)) return null;
    return validateWorkspaceState(state).length === 0 ? state : null;
  } catch {
    return null;
  }
}

export function loadWorkspaceSession(): WorkspaceState | null {
  const storage = localStorageOrNull();
  const value = storage?.getItem(SESSION_KEY);
  return value ? decodeWorkspaceSession(value) : null;
}

export function persistWorkspaceSession(state: WorkspaceState) {
  try {
    localStorageOrNull()?.setItem(SESSION_KEY, encodeWorkspaceSession(state));
  } catch {
    // Storage can be disabled or full; navigation must remain usable regardless.
  }
}

function isWorkspaceState(value: unknown): value is WorkspaceState {
  if (!isRecord(value) || !isWorkspaceNode(value.tree) || !isRecord(value.panes)) return false;
  if (
    typeof value.primaryPaneId !== "string" ||
    typeof value.activePaneId !== "string" ||
    typeof value.compactVisiblePaneId !== "string" ||
    typeof value.nextPaneOrdinal !== "number" ||
    typeof value.nextSplitOrdinal !== "number" ||
    !isRecord(value.assistantDock) ||
    !Object.values(DockVisibility).includes(value.assistantDock.visibility as DockVisibility) ||
    typeof value.assistantDock.width !== "number"
  ) {
    return false;
  }
  return Object.values(value.panes).every(
    (pane) =>
      isRecord(pane) &&
      typeof pane.id === "string" &&
      isPaneContent(pane.content) &&
      Array.isArray(pane.back) &&
      pane.back.every(isPaneContent) &&
      Array.isArray(pane.forward) &&
      pane.forward.every(isPaneContent),
  );
}

function isWorkspaceNode(value: unknown): value is WorkspaceNode {
  if (!isRecord(value)) return false;
  if (value.kind === WorkspaceNodeKind.Pane) return typeof value.paneId === "string";
  return (
    value.kind === WorkspaceNodeKind.Split &&
    typeof value.splitId === "string" &&
    Object.values(SplitAxis).includes(value.axis as SplitAxis) &&
    typeof value.ratio === "number" &&
    isWorkspaceNode(value.first) &&
    isWorkspaceNode(value.second)
  );
}

function isPaneContent(value: unknown): value is PaneContent {
  if (!isRecord(value)) return false;
  switch (value.kind) {
    case PaneContentKind.Home:
    case PaneContentKind.AllNotes:
      return true;
    case PaneContentKind.Page:
      return (
        typeof value.pageUuid === "string" &&
        (value.blockUuid === null || typeof value.blockUuid === "string") &&
        (value.presentation === "editing" || value.presentation === "reading")
      );
    case PaneContentKind.Graph:
      return value.focusPageUuid === null || typeof value.focusPageUuid === "string";
    case PaneContentKind.JournalDay:
      return typeof value.date === "string";
    default:
      return false;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function localStorageOrNull(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

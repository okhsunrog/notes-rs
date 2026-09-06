import type { InkNoteStatus, InkVersionInfo } from "@/lib/bindings";

/**
 * Published branches of one note. There can be more than two, and a branch
 * whose body has not been downloaded yet must never be chosen: keeping it
 * would write an empty drawing over the note.
 */
export type ConflictState = Readonly<{
  heads: readonly InkVersionInfo[];
  selectedVersionUuid: string | null;
  busy: boolean;
  notice: string | null;
}>;

export type ConflictAction =
  | { readonly type: "select"; readonly versionUuid: string }
  | { readonly type: "resolve_started" }
  | { readonly type: "resolve_stale"; readonly heads: readonly InkVersionInfo[] }
  | { readonly type: "resolve_failed"; readonly message: string };

export const STALE_NOTICE = "Versions changed, choose again.";

export function hasConflict(status: InkNoteStatus | undefined | null): boolean {
  return (status?.heads.length ?? 0) > 1;
}

export function unsentChanges(status: InkNoteStatus | undefined | null): boolean {
  return Boolean(status && (status.unpublishedChanges || status.publicationRequested));
}

export function initialConflictState(heads: readonly InkVersionInfo[]): ConflictState {
  return { heads, selectedVersionUuid: null, busy: false, notice: null };
}

export function isSelectable(state: ConflictState, versionUuid: string): boolean {
  return state.heads.some((head) => head.versionUuid === versionUuid && head.available);
}

/** Every current head, in order; the backend rejects a stale expectation. */
export function expectedHeads(state: ConflictState): string[] {
  return state.heads.map((head) => head.versionUuid);
}

/** The chosen branch stays in this note; nothing else is kept. */
export function keepOne(state: ConflictState): string[] | null {
  const selected = state.selectedVersionUuid;
  if (!selected || !isSelectable(state, selected)) return null;
  return [selected];
}

/** The first head stays in place, the rest become separate notes. */
export function keepAll(state: ConflictState): string[] | null {
  if (state.heads.length < 2 || state.heads.some((head) => !head.available)) return null;
  return expectedHeads(state);
}

export function conflictReducer(state: ConflictState, action: ConflictAction): ConflictState {
  switch (action.type) {
    case "select":
      if (state.busy || !isSelectable(state, action.versionUuid)) return state;
      return { ...state, selectedVersionUuid: action.versionUuid, notice: null };
    case "resolve_started":
      return { ...state, busy: true, notice: null };
    case "resolve_stale": {
      const selected =
        state.selectedVersionUuid &&
        action.heads.some(
          (head) => head.versionUuid === state.selectedVersionUuid && head.available,
        )
          ? state.selectedVersionUuid
          : null;
      return {
        heads: action.heads,
        selectedVersionUuid: selected,
        busy: false,
        notice: STALE_NOTICE,
      };
    }
    case "resolve_failed":
      return { ...state, busy: false, notice: action.message };
  }
}

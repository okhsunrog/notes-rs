import { saveHandwritingPatch } from "@/lib/api";
import type { InkDraftPatch, InkDraftSnapshot } from "@/lib/bindings";
import { DraftWriter, type DraftSaveState } from "./draft-writer";
import { incrementalDraftSaver } from "./ink-patch";

export type SavePatch = (
  pageUuid: string,
  patch: InkDraftPatch,
  expectedRevision: string | null,
) => Promise<string>;

/**
 * Editing state that must outlive the React tree: a note keeps writing and
 * publishing while the component that started it is being unmounted. Sessions
 * and completions are therefore keyed by page UUID at module scope, never in
 * component state.
 */
const writers = new Map<string, DraftWriter>();
const completions = new Map<string, Promise<void>>();
const editors = new Map<string, object>();

/** One editable view per note; a second view of the same UUID stays read-only. */
export function acquireEditor(pageUuid: string, owner: object): boolean {
  const current = editors.get(pageUuid);
  if (current && current !== owner) return false;
  editors.set(pageUuid, owner);
  return true;
}

export function releaseEditor(pageUuid: string, owner: object): void {
  if (editors.get(pageUuid) === owner) editors.delete(pageUuid);
}

/**
 * Bind a writer to one note. Call it only with a freshly read snapshot: the
 * previous writer's queue belongs to the revision that snapshot replaced.
 */
export function beginSession(
  pageUuid: string,
  snapshot: InkDraftSnapshot,
  onState: (state: DraftSaveState) => void,
  save: SavePatch = saveHandwritingPatch,
): DraftWriter {
  const writer = new DraftWriter(
    snapshot.revision,
    incrementalDraftSaver(snapshot.draft, (patch, revision) => save(pageUuid, patch, revision)),
    onState,
  );
  writers.set(pageUuid, writer);
  return writer;
}

export function getWriter(pageUuid: string): DraftWriter | null {
  return writers.get(pageUuid) ?? null;
}

/**
 * Run completion at most once per note at a time and keep the promise where a
 * later mount can wait for it. A failure is reported to the caller and leaves
 * the session — including its unacknowledged gestures — untouched.
 */
export function requestCompletion(pageUuid: string, run: () => Promise<unknown>): Promise<void> {
  const active = completions.get(pageUuid);
  if (active) return active;
  const entry: { promise?: Promise<void> } = {};
  const started = (async () => {
    await run();
  })();
  entry.promise = started.finally(() => {
    if (completions.get(pageUuid) === entry.promise) completions.delete(pageUuid);
  });
  completions.set(pageUuid, entry.promise);
  return entry.promise;
}

/**
 * Wait for a completion started by an earlier session before reading the note
 * again. The requester surfaces its own failure, so waiting never rejects.
 */
export async function awaitCompletion(pageUuid: string): Promise<void> {
  const pending = completions.get(pageUuid);
  if (!pending) return;
  try {
    await pending;
  } catch {
    // Reported where the completion was requested.
  }
}

export function hasPendingCompletion(pageUuid: string): boolean {
  return completions.has(pageUuid);
}

export function endSession(pageUuid: string): void {
  writers.delete(pageUuid);
}

export function openSessionUuids(): string[] {
  return [...writers.keys()];
}

/** Drain every open note's queue before a process-wide completion. */
export async function flushAllSessions(): Promise<boolean> {
  const results = await Promise.all([...writers.values()].map((writer) => writer.flush()));
  return results.every(Boolean);
}

/** The registry outlives React, so tests clear it between cases. */
export function resetHandwritingSessions(): void {
  writers.clear();
  completions.clear();
  editors.clear();
}

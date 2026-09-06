import { expect, it } from "vite-plus/test";
import type { InkNoteStatus, InkVersionInfo } from "@/lib/bindings";
import {
  STALE_NOTICE,
  conflictReducer,
  expectedHeads,
  hasConflict,
  initialConflictState,
  isSelectable,
  keepAll,
  keepOne,
  unsentChanges,
} from "./handwriting-conflict-model";

const head = (versionUuid: string, available = true): InkVersionInfo => ({
  versionUuid,
  parents: [],
  rootHash: `hash-${versionUuid}`,
  deviceName: `device-${versionUuid}`,
  deviceId: `id-${versionUuid}`,
  modifiedAtMs: 1_757_000_000_000,
  available,
});

const status = (heads: InkVersionInfo[], unpublished = false): InkNoteStatus => ({
  pageUuid: "page-a",
  revision: "revision-1",
  unpublishedChanges: unpublished,
  publicationRequested: false,
  baseVersion: null,
  heads,
});

it("reports a conflict only for more than one published head", () => {
  expect(hasConflict(status([]))).toBe(false);
  expect(hasConflict(status([head("a")]))).toBe(false);
  expect(hasConflict(status([head("a"), head("b"), head("c")]))).toBe(true);
  expect(hasConflict(null)).toBe(false);
});

it("reports unsent work while changes or a publication are outstanding", () => {
  expect(unsentChanges(status([]))).toBe(false);
  expect(unsentChanges(status([], true))).toBe(true);
  expect(unsentChanges({ ...status([]), publicationRequested: true })).toBe(true);
});

it("refuses to select or keep a branch that is not downloaded", () => {
  const state = initialConflictState([head("a"), head("b", false)]);

  expect(isSelectable(state, "b")).toBe(false);
  expect(conflictReducer(state, { type: "select", versionUuid: "b" })).toBe(state);
  expect(keepAll(state)).toBeNull();

  const chosen = conflictReducer(state, { type: "select", versionUuid: "a" });
  expect(keepOne(chosen)).toEqual(["a"]);
});

it("keeps every branch in order and reports all of them as expected", () => {
  const state = initialConflictState([head("a"), head("b"), head("c")]);

  expect(keepAll(state)).toEqual(["a", "b", "c"]);
  expect(expectedHeads(state)).toEqual(["a", "b", "c"]);
  expect(keepOne(state)).toBeNull();
});

it("re-reads the branches and asks again after a stale resolve", () => {
  const chosen = conflictReducer(initialConflictState([head("a"), head("b")]), {
    type: "select",
    versionUuid: "b",
  });
  const busy = conflictReducer(chosen, { type: "resolve_started" });
  expect(busy.busy).toBe(true);

  const stale = conflictReducer(busy, { type: "resolve_stale", heads: [head("a"), head("c")] });

  expect(stale.busy).toBe(false);
  expect(stale.notice).toBe(STALE_NOTICE);
  expect(stale.selectedVersionUuid).toBeNull();
  expect(expectedHeads(stale)).toEqual(["a", "c"]);

  const kept = conflictReducer(busy, { type: "resolve_stale", heads: [head("b"), head("c")] });
  expect(kept.selectedVersionUuid).toBe("b");
});

it("reports a failed resolve without dropping the choice", () => {
  const chosen = conflictReducer(initialConflictState([head("a"), head("b")]), {
    type: "select",
    versionUuid: "a",
  });
  const failed = conflictReducer(conflictReducer(chosen, { type: "resolve_started" }), {
    type: "resolve_failed",
    message: "server unreachable",
  });

  expect(failed.busy).toBe(false);
  expect(failed.notice).toBe("server unreachable");
  expect(failed.selectedVersionUuid).toBe("a");
});

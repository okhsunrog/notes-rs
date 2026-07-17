import { describe, expect, it } from "vite-plus/test";
import type { ImportDiagnostic, LogseqImportCommitResult, LogseqImportPreview } from "@/lib/api";
import {
  initialLogseqImportState,
  logseqImportHasMoreDiagnostics,
  logseqImportReducer,
} from "./logseq-import-state";

const warning: ImportDiagnostic = {
  severity: "warning",
  code: "unresolved_page_reference",
  relativePath: "pages/project.md",
  range: null,
  message: "The referenced page does not exist.",
  remediation: "Create the missing page or update the reference.",
};

function preview(overrides: Partial<LogseqImportPreview> = {}): LogseqImportPreview {
  return {
    sessionUuid: "019f0000-0000-7000-8000-000000000001",
    sourceName: "notes",
    manifestSha256: "a".repeat(64),
    destination: "empty",
    blockers: [],
    canCommit: true,
    report: {
      pageCount: 3,
      journalCount: 1,
      blockCount: 12,
      syntheticPreambleBlockCount: 0,
      taskCount: 2,
      referenceCount: 4,
      resolvedReferenceCount: 3,
      unresolvedReferenceCount: 1,
      mediaReferenceCount: 2,
      markdownImageCount: 1,
      drawingConversionState: "absent",
      preparedExcalidrawCount: 0,
      preservedExcalidrawCount: 1,
      localMediaReferenceCount: 1,
      inlineMediaReferenceCount: 0,
      blockedRemoteMediaReferenceCount: 1,
      missingMediaReferenceCount: 0,
      blockedUnsafeMediaReferenceCount: 0,
      unsupportedMediaReferenceCount: 0,
      unreferencedAssetCount: 0,
      unreferencedDrawingCount: 1,
      preservedBlockUuidCount: 10,
      derivedBlockUuidCount: 2,
      warningCount: 2,
      blockingDiagnosticCount: 0,
      diagnosticCount: 2,
      diagnosticsTruncated: true,
      diagnostics: [warning],
    },
    ...overrides,
  };
}

describe("Logseq import state machine", () => {
  it("moves through prepare, preview, commit, and result", () => {
    let state = logseqImportReducer(initialLogseqImportState, {
      type: "prepare_started",
      runId: 1,
    });
    state = logseqImportReducer(state, {
      type: "progress_received",
      runId: 1,
      progress: { progress: "items", stage: "parsing", completed: 2, total: 3 },
    });
    expect(state.phase).toBe("preparing");

    const prepared = preview();
    state = logseqImportReducer(state, { type: "preview_ready", runId: 1, preview: prepared });
    expect(state.phase).toBe("preview");
    expect(logseqImportHasMoreDiagnostics(state)).toBe(true);

    state = logseqImportReducer(state, {
      type: "commit_started",
      runId: 1,
      sessionUuid: prepared.sessionUuid,
    });
    expect(state.phase).toBe("committing");

    const result: LogseqImportCommitResult = {
      status: "applied",
      receiptUuid: "019f0000-0000-7000-8000-000000000002",
      pageCount: 3,
      blockCount: 12,
      aliasCount: 1,
      attachmentCount: 1,
      operationCount: 20,
      openPageUuid: "019f0000-0000-7000-8000-000000000003",
    };
    state = logseqImportReducer(state, {
      type: "commit_finished",
      runId: 1,
      sessionUuid: prepared.sessionUuid,
      result,
    });
    expect(state).toEqual({ phase: "result", runId: 1, result, error: null });
  });

  it("ignores stale runs, sessions, progress, and diagnostic pages", () => {
    let state = logseqImportReducer(initialLogseqImportState, {
      type: "prepare_started",
      runId: 4,
    });
    const beforeStaleProgress = state;
    state = logseqImportReducer(state, {
      type: "progress_received",
      runId: 3,
      progress: { progress: "indeterminate", stage: "scanning" },
    });
    expect(state).toBe(beforeStaleProgress);

    const prepared = preview();
    state = logseqImportReducer(state, { type: "preview_ready", runId: 4, preview: prepared });
    const previewState = state;
    state = logseqImportReducer(state, {
      type: "diagnostics_started",
      runId: 4,
      sessionUuid: "stale-session",
    });
    expect(state).toBe(previewState);

    state = logseqImportReducer(state, {
      type: "diagnostics_started",
      runId: 4,
      sessionUuid: prepared.sessionUuid,
    });
    const loadingState = state;
    state = logseqImportReducer(state, {
      type: "diagnostics_loaded",
      runId: 4,
      sessionUuid: prepared.sessionUuid,
      page: { offset: 0, total: 2, diagnostics: [warning] },
    });
    expect(state).toBe(loadingState);

    state = logseqImportReducer(state, {
      type: "diagnostics_loaded",
      runId: 4,
      sessionUuid: prepared.sessionUuid,
      page: { offset: 1, total: 2, diagnostics: [warning] },
    });
    expect(state.phase).toBe("preview");
    if (state.phase === "preview") expect(state.diagnostics).toHaveLength(2);
  });

  it("does not enter commit when preview has blockers", () => {
    let state = logseqImportReducer(initialLogseqImportState, {
      type: "prepare_started",
      runId: 1,
    });
    const blocked = preview({ blockers: ["non_empty_workspace"], canCommit: false });
    state = logseqImportReducer(state, { type: "preview_ready", runId: 1, preview: blocked });
    const beforeCommit = state;
    state = logseqImportReducer(state, {
      type: "commit_started",
      runId: 1,
      sessionUuid: blocked.sessionUuid,
    });
    expect(state).toBe(beforeCommit);
  });
});

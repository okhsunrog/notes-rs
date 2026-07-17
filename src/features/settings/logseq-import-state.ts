import type {
  ImportDiagnostic,
  LogseqImportCommitResult,
  LogseqImportDiagnosticPage,
  LogseqImportPreview,
  LogseqImportProgress,
} from "@/lib/api";

type ImportSession = {
  runId: number;
  preview: LogseqImportPreview;
  diagnostics: ImportDiagnostic[];
  diagnosticsTotal: number;
  diagnosticsLoading: boolean;
  discarding: boolean;
  error: string | null;
};

export type LogseqImportState =
  | { phase: "idle"; runId: number; error: string | null }
  | {
      phase: "preparing";
      runId: number;
      progress: LogseqImportProgress | null;
      error: null;
    }
  | ({ phase: "preview"; progress: LogseqImportProgress | null } & ImportSession)
  | ({ phase: "committing"; progress: LogseqImportProgress | null } & ImportSession)
  | {
      phase: "result";
      runId: number;
      result: LogseqImportCommitResult;
      error: null;
    };

export type LogseqImportAction =
  | { type: "prepare_started"; runId: number }
  | { type: "progress_received"; runId: number; progress: LogseqImportProgress }
  | { type: "prepare_cancelled"; runId: number }
  | { type: "prepare_failed"; runId: number; error: string }
  | { type: "preview_ready"; runId: number; preview: LogseqImportPreview }
  | { type: "diagnostics_started"; runId: number; sessionUuid: string }
  | {
      type: "diagnostics_loaded";
      runId: number;
      sessionUuid: string;
      page: LogseqImportDiagnosticPage;
    }
  | { type: "diagnostics_failed"; runId: number; sessionUuid: string; error: string }
  | { type: "discard_started"; runId: number; sessionUuid: string }
  | { type: "discard_failed"; runId: number; sessionUuid: string; error: string }
  | { type: "commit_started"; runId: number; sessionUuid: string }
  | {
      type: "commit_finished";
      runId: number;
      sessionUuid: string;
      result: LogseqImportCommitResult;
    }
  | { type: "commit_failed"; runId: number; sessionUuid: string; error: string }
  | { type: "reset"; runId: number };

export const initialLogseqImportState: LogseqImportState = {
  phase: "idle",
  runId: 0,
  error: null,
};

function hasSession(
  state: LogseqImportState,
): state is Extract<LogseqImportState, { phase: "preview" | "committing" }> {
  return state.phase === "preview" || state.phase === "committing";
}

function matchesSession(state: LogseqImportState, runId: number, sessionUuid: string) {
  return hasSession(state) && state.runId === runId && state.preview.sessionUuid === sessionUuid;
}

export function logseqImportReducer(
  state: LogseqImportState,
  action: LogseqImportAction,
): LogseqImportState {
  switch (action.type) {
    case "prepare_started":
      return { phase: "preparing", runId: action.runId, progress: null, error: null };

    case "progress_received":
      if (
        state.runId !== action.runId ||
        (state.phase !== "preparing" && state.phase !== "committing")
      ) {
        return state;
      }
      return { ...state, progress: action.progress };

    case "prepare_cancelled":
      return state.phase === "preparing" && state.runId === action.runId
        ? { phase: "idle", runId: action.runId, error: null }
        : state;

    case "prepare_failed":
      return state.phase === "preparing" && state.runId === action.runId
        ? { phase: "idle", runId: action.runId, error: action.error }
        : state;

    case "preview_ready":
      if (state.phase !== "preparing" || state.runId !== action.runId) return state;
      return {
        phase: "preview",
        runId: action.runId,
        preview: action.preview,
        progress: null,
        diagnostics: action.preview.report.diagnostics,
        diagnosticsTotal: action.preview.report.diagnosticCount,
        diagnosticsLoading: false,
        discarding: false,
        error: null,
      };

    case "diagnostics_started":
      return matchesSession(state, action.runId, action.sessionUuid) &&
        state.phase === "preview" &&
        !state.discarding
        ? { ...state, diagnosticsLoading: true, error: null }
        : state;

    case "diagnostics_loaded":
      if (
        !matchesSession(state, action.runId, action.sessionUuid) ||
        state.phase !== "preview" ||
        !state.diagnosticsLoading ||
        action.page.offset !== state.diagnostics.length
      ) {
        return state;
      }
      return {
        ...state,
        diagnostics: [...state.diagnostics, ...action.page.diagnostics],
        diagnosticsTotal: action.page.total,
        diagnosticsLoading: false,
        error: null,
      };

    case "diagnostics_failed":
      return matchesSession(state, action.runId, action.sessionUuid) && state.phase === "preview"
        ? { ...state, diagnosticsLoading: false, error: action.error }
        : state;

    case "discard_started":
      return matchesSession(state, action.runId, action.sessionUuid) && state.phase === "preview"
        ? { ...state, discarding: true, diagnosticsLoading: false, error: null }
        : state;

    case "discard_failed":
      return matchesSession(state, action.runId, action.sessionUuid) && state.phase === "preview"
        ? { ...state, discarding: false, error: action.error }
        : state;

    case "commit_started":
      if (
        !matchesSession(state, action.runId, action.sessionUuid) ||
        state.phase !== "preview" ||
        state.discarding ||
        !state.preview.canCommit ||
        state.preview.blockers.length > 0
      ) {
        return state;
      }
      return {
        ...state,
        phase: "committing",
        progress: null,
        diagnosticsLoading: false,
        error: null,
      };

    case "commit_finished":
      return matchesSession(state, action.runId, action.sessionUuid) && state.phase === "committing"
        ? {
            phase: "result",
            runId: action.runId,
            result: action.result,
            error: null,
          }
        : state;

    case "commit_failed":
      return matchesSession(state, action.runId, action.sessionUuid) && state.phase === "committing"
        ? { ...state, phase: "preview", progress: null, error: action.error }
        : state;

    case "reset":
      return { phase: "idle", runId: action.runId, error: null };
  }
}

export function logseqImportHasMoreDiagnostics(state: LogseqImportState) {
  return (
    (state.phase === "preview" || state.phase === "committing") &&
    state.diagnostics.length < state.diagnosticsTotal
  );
}

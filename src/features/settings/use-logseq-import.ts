import { useCallback, useEffect, useReducer, useRef } from "react";
import { Channel } from "@tauri-apps/api/core";
import {
  commitLogseqImport,
  discardLogseqImport,
  getLogseqImportDiagnostics,
  prepareLogseqImport,
  type LogseqImportProgress,
} from "@/lib/api";
import {
  initialLogseqImportState,
  logseqImportHasMoreDiagnostics,
  logseqImportReducer,
  type LogseqImportState,
} from "./logseq-import-state";

const DIAGNOSTICS_PAGE_SIZE = 50;

type Options = {
  onDataChanged: (openPageUuid: string | null) => void;
  onError: (message: string) => void;
  onMessage: (message: string) => void;
};

function sessionFromState(state: LogseqImportState) {
  return state.phase === "preview" || state.phase === "committing" ? state.preview : null;
}

export function useLogseqImport({ onDataChanged, onError, onMessage }: Options) {
  const [state, dispatch] = useReducer(logseqImportReducer, initialLogseqImportState);
  const stateRef = useRef(state);
  const activeRunRef = useRef(0);
  const mountedRef = useRef(true);
  const prepareGuardRef = useRef<number | null>(null);
  const commitGuardRef = useRef<string | null>(null);
  const discardGuardRef = useRef<string | null>(null);
  stateRef.current = state;

  const reportError = useCallback(
    (reason: unknown) => {
      const message = reason instanceof Error ? reason.message : String(reason);
      onError(message);
      return message;
    },
    [onError],
  );

  const discardPreview = useCallback(
    async (current: Extract<LogseqImportState, { phase: "preview" }>) => {
      const sessionUuid = current.preview.sessionUuid;
      if (discardGuardRef.current !== null) return false;
      discardGuardRef.current = sessionUuid;
      dispatch({ type: "discard_started", runId: current.runId, sessionUuid });
      try {
        await discardLogseqImport(sessionUuid);
        return true;
      } catch (reason) {
        const error = reportError(reason);
        dispatch({ type: "discard_failed", runId: current.runId, sessionUuid, error });
        return false;
      } finally {
        if (discardGuardRef.current === sessionUuid) discardGuardRef.current = null;
      }
    },
    [reportError],
  );

  const start = useCallback(async () => {
    if (prepareGuardRef.current !== null || commitGuardRef.current !== null) return;
    prepareGuardRef.current = 0;
    const current = stateRef.current;
    if (current.phase === "preparing" || current.phase === "committing") {
      prepareGuardRef.current = null;
      return;
    }
    if (current.phase === "preview" && !(await discardPreview(current))) {
      prepareGuardRef.current = null;
      return;
    }

    const runId = activeRunRef.current + 1;
    activeRunRef.current = runId;
    prepareGuardRef.current = runId;
    dispatch({ type: "prepare_started", runId });
    onError("");
    onMessage("");

    const channel = new Channel<LogseqImportProgress>((progress) => {
      dispatch({ type: "progress_received", runId, progress });
    });

    try {
      const preview = await prepareLogseqImport(channel);
      if (!mountedRef.current || activeRunRef.current !== runId) {
        if (preview) void discardLogseqImport(preview.sessionUuid).catch(() => undefined);
        return;
      }
      if (!preview) {
        dispatch({ type: "prepare_cancelled", runId });
        return;
      }
      dispatch({ type: "preview_ready", runId, preview });
    } catch (reason) {
      if (!mountedRef.current || activeRunRef.current !== runId) return;
      const error = reportError(reason);
      dispatch({ type: "prepare_failed", runId, error });
    } finally {
      if (prepareGuardRef.current === runId) prepareGuardRef.current = null;
    }
  }, [discardPreview, onError, onMessage, reportError]);

  const loadMoreDiagnostics = useCallback(async () => {
    const current = stateRef.current;
    if (
      current.phase !== "preview" ||
      current.diagnosticsLoading ||
      current.discarding ||
      !logseqImportHasMoreDiagnostics(current)
    ) {
      return;
    }

    const { runId, preview } = current;
    const offset = current.diagnostics.length;
    dispatch({ type: "diagnostics_started", runId, sessionUuid: preview.sessionUuid });
    try {
      const page = await getLogseqImportDiagnostics(
        preview.sessionUuid,
        offset,
        DIAGNOSTICS_PAGE_SIZE,
      );
      dispatch({
        type: "diagnostics_loaded",
        runId,
        sessionUuid: preview.sessionUuid,
        page,
      });
    } catch (reason) {
      const latest = stateRef.current;
      if (
        !mountedRef.current ||
        activeRunRef.current !== runId ||
        latest.phase !== "preview" ||
        latest.preview.sessionUuid !== preview.sessionUuid
      ) {
        return;
      }
      const error = reportError(reason);
      dispatch({
        type: "diagnostics_failed",
        runId,
        sessionUuid: preview.sessionUuid,
        error,
      });
    }
  }, [reportError]);

  const commit = useCallback(async () => {
    const current = stateRef.current;
    if (
      commitGuardRef.current !== null ||
      current.phase !== "preview" ||
      current.discarding ||
      !current.preview.canCommit ||
      current.preview.blockers.length > 0
    ) {
      return;
    }

    const { runId, preview } = current;
    commitGuardRef.current = preview.sessionUuid;
    dispatch({ type: "commit_started", runId, sessionUuid: preview.sessionUuid });
    onError("");
    onMessage("");
    const channel = new Channel<LogseqImportProgress>((progress) => {
      dispatch({ type: "progress_received", runId, progress });
    });

    try {
      const result = await commitLogseqImport(preview.sessionUuid, channel);
      if (!mountedRef.current || activeRunRef.current !== runId) return;
      dispatch({ type: "commit_finished", runId, sessionUuid: preview.sessionUuid, result });
      onMessage(
        result.status === "applied"
          ? `Imported ${result.pageCount} pages and ${result.blockCount} blocks from Logseq.`
          : "This Logseq graph was already imported and has not changed.",
      );
      onDataChanged(null);
    } catch (reason) {
      if (!mountedRef.current || activeRunRef.current !== runId) return;
      const error = reportError(reason);
      dispatch({ type: "commit_failed", runId, sessionUuid: preview.sessionUuid, error });
    } finally {
      if (commitGuardRef.current === preview.sessionUuid) commitGuardRef.current = null;
    }
  }, [onDataChanged, onError, onMessage, reportError]);

  const close = useCallback(async () => {
    const current = stateRef.current;
    if (current.phase === "committing") return;
    if (current.phase === "preview" && !(await discardPreview(current))) return;
    if (current.phase === "preparing" && prepareGuardRef.current === current.runId) {
      prepareGuardRef.current = null;
    }
    const runId = activeRunRef.current + 1;
    activeRunRef.current = runId;
    dispatch({ type: "reset", runId });
  }, [discardPreview]);

  const openResult = useCallback(() => {
    const current = stateRef.current;
    if (current.phase === "result" && current.result.openPageUuid) {
      onDataChanged(current.result.openPageUuid);
    }
  }, [onDataChanged]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      activeRunRef.current += 1;
      const session = sessionFromState(stateRef.current);
      if (stateRef.current.phase === "preview" && session) {
        void discardLogseqImport(session.sessionUuid).catch(() => undefined);
      }
    };
  }, []);

  return {
    state,
    start,
    commit,
    close,
    openResult,
    loadMoreDiagnostics,
    hasMoreDiagnostics: logseqImportHasMoreDiagnostics(state),
  };
}

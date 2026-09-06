import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Copy,
  Eraser,
  Grid2X2,
  Lasso,
  Minus,
  PenLine,
  Plus,
  Redo2,
  Square,
  Star,
  Trash2,
  Undo2,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { usePageNavigationStore } from "@/features/pages/page-navigation-store";
import { usePageTitleEditor } from "@/features/pages/use-page-title-editor";
import { currentDisposition, homeTarget, type PaneId } from "@/features/workspace/workspace-model";
import { useWorkspaceStore } from "@/features/workspace/workspace-store";
import {
  CommandFailure,
  completeHandwritingNote,
  handwritingHistory,
  handwritingNoteStatus,
  loadHandwritingNote,
  setHandwritingBackground,
  unknownErrorMessage,
  type Page,
} from "@/lib/api";
import type { InkDraft, InkHistorySnapshot } from "@/lib/bindings";
import { notifyError, notifyRetryableError } from "@/lib/notify";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import type { DraftSaveState } from "./draft-writer";
import { canDrawHandwriting, editorAccess, type EditorOwnership } from "./handwriting-access";
import { hasConflict, unsentChanges } from "./handwriting-conflict-model";
import { HandwritingConflictDialog } from "./handwriting-conflicts";
import {
  acquireEditor,
  awaitCompletion,
  beginSession,
  endSession,
  getWriter,
  releaseEditor,
  requestCompletion,
} from "./handwriting-session";
import { useResolvedInkColor } from "@/app/appearance";
import { InkCanvas, type InkTool } from "./ink-canvas";
import { moveSelection, scaleSelection, type EraserMode, type LassoMode } from "./ink-editing";
import { MAX_INK_POINTS } from "./ink-model";
import { applyHistoryUpdate } from "./ink-patch";
import { useHandwritingAvailability, useHandwritingPreference } from "./input-capabilities";
import type { OnyxInkStatus } from "./onyx-ink";

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.isContentEditable ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLInputElement
  );
}

/** The note itself is gone, so nothing is left to store, pack or retry. */
function noteIsGone(error: Error | null): boolean {
  return error instanceof CommandFailure && error.code === "not_found";
}

/**
 * Completion outlives the editor: it is requested detached, retried from a
 * toast, and never treated as server confirmation of the publication.
 */
function completeInBackground(pageUuid: string) {
  requestCompletion(pageUuid, () => completeHandwritingNote(pageUuid)).catch((error: unknown) => {
    // The note was deleted while it was being packed; there is nothing to send.
    if (error instanceof CommandFailure && error.code === "not_found") return;
    notifyRetryableError("Could not prepare sync", error, () => completeInBackground(pageUuid));
  });
}

type Props = {
  paneId: PaneId;
  page: Page;
  onSaved: (updated: Page) => void;
  onDelete: (page: Page) => void | Promise<void>;
};

/**
 * A handwritten note in a workspace pane. It owns the note's ink session for as
 * long as it is mounted; the text editor is never mounted for this page kind.
 */
export function HandwritingNoteView({ paneId, page, onSaved, onDelete }: Props) {
  const monoInk = useResolvedInkColor() === "mono";
  const uuid = page.uuid;
  const { available, capabilities } = useHandwritingAvailability();
  const mouseEnabled = useHandwritingPreference((state) => state.mouseEnabled);
  const queryClient = useQueryClient();
  const dispatch = useWorkspaceStore((state) => state.dispatch);
  const historyDepth = useWorkspaceStore((state) => state.panes[paneId]?.back.length ?? 0);
  const favorite = usePageNavigationStore((state) => state.favoritePageUuids.includes(uuid));
  const toggleFavoritePage = usePageNavigationStore((state) => state.toggleFavoritePage);

  const [draft, setDraft] = useState<InkDraft | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveState, setSaveState] = useState<DraftSaveState>("saved");
  const [tool, setTool] = useState<InkTool>("pen");
  const [width, setWidth] = useState(3);
  const [eraserMode, setEraserMode] = useState<EraserMode>("stroke");
  const [eraserRadius, setEraserRadius] = useState(12);
  const [lassoMode, setLassoMode] = useState<LassoMode>("free");
  const [selected, setSelected] = useState<string[]>([]);
  const [active, setActive] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [nativeStatus, setNativeStatus] = useState<OnyxInkStatus | null>(null);
  const [limit, setLimit] = useState(false);
  const [canUndo, setCanUndo] = useState(false);
  const [canRedo, setCanRedo] = useState(false);
  const [historyBusy, setHistoryBusy] = useState(false);
  const [ownership, setOwnership] = useState<EditorOwnership>("pending");
  // Input stays closed between a background transition and the re-read that
  // adopts whatever completion published while the window was hidden.
  const [suspended, setSuspended] = useState(false);
  const [conflictsOpen, setConflictsOpen] = useState(false);
  // Gestures an earlier mount could not store are shown and retried before any
  // snapshot from storage is allowed to replace them.
  const [recovering, setRecovering] = useState(false);
  const [comparing, setComparing] = useState(false);

  const latestDraft = useRef<InkDraft | null>(null);
  const historyBusyRef = useRef(false);
  const leavingRef = useRef(false);
  const ownerRef = useRef({});
  const generationRef = useRef(0);
  const suspendedRef = useRef(false);
  suspendedRef.current = suspended;
  // Whether the last read asked for an editable session, so a later upgrade is
  // distinguishable from an ordinary re-read.
  const openedForEditing = useRef(false);

  const access = editorAccess(ownership, canDrawHandwriting(available, mouseEnabled));
  const editing = access === "editable";
  const editingRef = useRef(editing);
  editingRef.current = editing;

  const statusQuery = useQuery({
    queryKey: queryKeys.handwritingStatus(uuid),
    queryFn: () => handwritingNoteStatus(uuid),
  });
  const status = statusQuery.data;

  const title = usePageTitleEditor(page, true, onSaved);
  const titleFlush = useRef(title.flush);
  titleFlush.current = title.flush;
  const busy = active || historyBusy || leaving;
  const editingBusy = busy || suspended;

  const adopt = useCallback(
    (history: InkHistorySnapshot) => {
      const { snapshot } = history;
      if (editingRef.current) beginSession(uuid, snapshot, setSaveState);
      latestDraft.current = snapshot.draft;
      setDraft(snapshot.draft);
      setCanUndo(history.canUndo);
      setCanRedo(history.canRedo);
      setSaveState("saved");
      setSelected([]);
      setLimit(false);
      setSuspended(false);
      setRecovering(false);
    },
    [uuid],
  );

  /**
   * Read the note only after any completion this UUID still owes has settled,
   * so a session started here never continues from a superseded revision.
   */
  const openNote = useCallback(async () => {
    const generation = ++generationRef.current;
    openedForEditing.current = editingRef.current;
    setLoadError(null);
    try {
      const carried = getWriter(uuid);
      if (carried?.hasPending()) {
        // Adopt the queue of the mount that could not store it, show its newest
        // state, and retry. Loading waits until storage has accepted it.
        carried.setOnState(setSaveState);
        const queued = carried.latestDraft();
        if (queued) {
          latestDraft.current = queued;
          setDraft(queued);
        }
        setSuspended(false);
        setRecovering(true);
        if (!(await carried.flush())) return;
        if (generation !== generationRef.current) return;
      }
      await awaitCompletion(uuid);
      const history = await loadHandwritingNote(uuid, editingRef.current);
      if (generation !== generationRef.current) return;
      // A gesture completed while the note was being read is newer than it.
      if (getWriter(uuid)?.hasPending()) {
        setRecovering(true);
        return;
      }
      adopt(history);
    } catch (error) {
      if (generation !== generationRef.current) return;
      setLoadError(unknownErrorMessage(error));
    }
  }, [adopt, uuid]);

  // Once the carried gestures are stored, continue with the read they blocked.
  useEffect(() => {
    if (recovering && saveState === "saved") void openNote();
  }, [openNote, recovering, saveState]);

  // Two editors of one note are not a shared session; the second view reads.
  useEffect(() => {
    const owner = ownerRef.current;
    setOwnership(acquireEditor(uuid, owner) ? "owned" : "taken");
    return () => {
      releaseEditor(uuid, owner);
      setOwnership("pending");
    };
  }, [uuid]);

  useEffect(() => {
    if (ownership === "pending") return;
    void openNote();
    return () => {
      generationRef.current += 1;
    };
  }, [openNote, ownership]);

  // A read-only open holds no session and stores nothing, so a pen observed
  // after the note was opened — or mouse drawing switched on in Settings — has
  // to re-read the note with `editing` set before the canvas accepts input.
  useEffect(() => {
    if (ownership === "pending" || !editing || openedForEditing.current) return;
    void openNote();
  }, [editing, openNote, ownership]);

  const suspend = useCallback(() => {
    if (!editingRef.current) return;
    setSuspended(true);
    void setHandwritingBackground(true);
    const writer = getWriter(uuid);
    if (!writer) return;
    void (async () => {
      if (await writer.flush()) completeInBackground(uuid);
    })();
  }, [uuid]);

  const resume = useCallback(() => {
    if (!editingRef.current) {
      setSuspended(false);
      return;
    }
    void setHandwritingBackground(false);
    void openNote();
  }, [openNote]);

  useEffect(() => {
    const visibility = () => {
      if (document.visibilityState === "hidden") suspend();
      else resume();
    };
    // Draining every session is registered once at process level, because it
    // has to run whether or not this view happens to be mounted.
    const leaveApp = () => {
      setSuspended(true);
      void setHandwritingBackground(true);
    };
    document.addEventListener("visibilitychange", visibility);
    window.addEventListener("pagehide", leaveApp);
    return () => {
      document.removeEventListener("visibilitychange", visibility);
      window.removeEventListener("pagehide", leaveApp);
    };
  }, [resume, suspend]);

  useEffect(() => {
    return () => {
      const writer = getWriter(uuid);
      if (!writer) return;
      void (async () => {
        // An unacknowledged gesture keeps the session alive so a later mount can
        // retry it; only a clean queue may be handed to completion and dropped.
        // A deleted note is the exception: its gestures can never be stored, so
        // holding the writer would strand it for the life of the process.
        const stored = await writer.flush();
        if (!stored && !noteIsGone(writer.lastError())) return;
        if (stored && !leavingRef.current) completeInBackground(uuid);
        endSession(uuid, writer);
      })();
    };
  }, [uuid]);

  /** Settle local work before comparing: a resolve must see the real heads. */
  const compareVersions = useCallback(async () => {
    if (busy || comparing) return;
    setComparing(true);
    try {
      const writer = getWriter(uuid);
      if (writer) {
        if (!(await writer.flush())) return;
        await requestCompletion(uuid, () => completeHandwritingNote(uuid));
      }
      await queryClient.invalidateQueries({ queryKey: queryKeys.handwritingStatus(uuid) });
      await statusQuery.refetch();
      setConflictsOpen(true);
    } catch (error) {
      notifyError("handwriting", error);
    } finally {
      setComparing(false);
    }
  }, [busy, comparing, queryClient, statusQuery, uuid]);

  const conflictResolved = useCallback(async () => {
    setConflictsOpen(false);
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.page(uuid) }),
      queryClient.invalidateQueries({ queryKey: queryKeys.pages }),
      queryClient.invalidateQueries({ queryKey: queryKeys.handwritingStatus(uuid) }),
    ]);
    await openNote();
  }, [openNote, queryClient, uuid]);

  const navigateAway = useCallback(() => {
    if (historyDepth > 0) dispatch({ type: "go_back", paneId });
    else dispatch({ type: "open_target", target: homeTarget, disposition: currentDisposition });
  }, [dispatch, historyDepth, paneId]);

  const leave = useCallback(async () => {
    if (busy || leavingRef.current) return;
    leavingRef.current = true;
    setLeaving(true);
    const writer = getWriter(uuid);
    if (writer && !(await writer.flush())) {
      leavingRef.current = false;
      setLeaving(false);
      return;
    }
    await titleFlush.current();
    navigateAway();
    if (writer) completeInBackground(uuid);
  }, [busy, navigateAway, uuid]);

  const deleteNote = useCallback(async () => {
    if (busy) return;
    // Store what is queued before the note goes away: the confirmation may be
    // declined, and a cancelled delete must not have cost a stroke. The session
    // is not closed here for the same reason — onDelete cannot report a
    // declined confirmation — so the unmount path stays in charge, and a
    // completion for a note that is gone is ignored rather than retried.
    const writer = getWriter(uuid);
    if (writer && !(await writer.flush()) && !noteIsGone(writer.lastError())) {
      // Storage refused the queue for a reason that is still on screen. Deleting
      // now would make a declined confirmation cost those gestures.
      return;
    }
    await onDelete(page);
  }, [busy, onDelete, page, uuid]);

  const change = (next: InkDraft) => {
    const previous = latestDraft.current;
    const writer = getWriter(uuid);
    if (
      !previous ||
      !writer ||
      next === previous ||
      historyBusyRef.current ||
      leavingRef.current ||
      suspendedRef.current
    ) {
      return;
    }
    latestDraft.current = next;
    setCanUndo(true);
    setCanRedo(false);
    setDraft(next);
    void writer.write(next);
  };

  const navigateHistory = async (redo: boolean) => {
    if (editingBusy || historyBusyRef.current || !(redo ? canRedo : canUndo)) return;
    historyBusyRef.current = true;
    setHistoryBusy(true);
    try {
      const writer = getWriter(uuid);
      if (!writer || !(await writer.flush())) return;
      const update = await handwritingHistory(uuid, redo, writer.getRevision());
      adopt(applyHistoryUpdate(latestDraft.current, writer.getRevision(), update));
    } catch (error) {
      setSaveState(error instanceof Error ? error : new Error(String(error)));
    } finally {
      historyBusyRef.current = false;
      setHistoryBusy(false);
    }
  };
  const undoStroke = () => void navigateHistory(false);
  const redoStroke = () => void navigateHistory(true);

  return (
    <div
      data-ink-editor
      className="flex h-full min-h-0 flex-col pb-[var(--safe-area-inset-bottom)]"
      onKeyDown={(event) => {
        // The title textarea is inside this container; its own undo stack must
        // keep working while the caret is in it.
        if (isEditableTarget(event.target)) return;
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
          event.preventDefault();
          event.stopPropagation();
          if (event.shiftKey) redoStroke();
          else undoStroke();
        }
      }}
    >
      <header className="flex shrink-0 items-center gap-2 border-b px-2 py-1.5">
        <Button
          type="button"
          variant="ghost"
          size="xs"
          onClick={() => void leave()}
          disabled={busy}
          aria-busy={leaving}
          className="shrink-0 gap-1 rounded-lg px-1.5"
        >
          <ArrowLeft className="size-3.5" />
          Back
        </Button>
        <textarea
          rows={1}
          value={title.title}
          onBlur={() => void title.flush()}
          onChange={(event) => title.edit(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.nativeEvent.isComposing) {
              event.preventDefault();
              event.currentTarget.blur();
            }
          }}
          placeholder="Untitled note"
          aria-label="Note title"
          className="min-w-0 flex-1 resize-none appearance-none overflow-hidden border-0 bg-transparent px-1 py-1 text-base leading-tight font-semibold text-foreground outline-none placeholder:text-muted-foreground/40"
        />
        {unsentChanges(status) && (
          <span className="shrink-0 text-[11px] text-muted-foreground">Unsent changes</span>
        )}
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={favorite ? "Remove note from favorites" : "Add note to favorites"}
          onClick={() => toggleFavoritePage(uuid)}
          className={cn("shrink-0 rounded-lg text-muted-foreground", favorite && "text-primary")}
        >
          <Star className={cn("size-4", favorite && "fill-current")} />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="Delete page"
          disabled={busy}
          onClick={() => void deleteNote()}
          className="shrink-0 rounded-lg text-muted-foreground hover:text-destructive"
        >
          <Trash2 className="size-4" />
        </Button>
      </header>
      {title.conflict && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-2 border-b bg-amber-500/10 px-3 py-2 text-xs"
        >
          <span className="mr-auto">
            This title changed on another replica. Choose which version to keep.
          </span>
          <Button type="button" variant="outline" size="xs" onClick={title.useRemote}>
            Use remote
          </Button>
          <Button type="button" size="xs" onClick={title.keepLocal}>
            Keep mine
          </Button>
        </div>
      )}
      {hasConflict(status) && (
        <div className="flex flex-wrap items-center gap-2 border-b bg-amber-500/10 px-3 py-2 text-xs">
          <span className="mr-auto">
            This note has {status?.heads.length} versions from other devices
          </span>
          <Button
            type="button"
            variant="outline"
            size="xs"
            disabled={busy || comparing}
            onClick={() => void compareVersions()}
          >
            Compare
          </Button>
        </div>
      )}
      {editing ? (
        <>
          <div
            role="toolbar"
            aria-label="Handwriting tools"
            className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2"
          >
            <Button
              type="button"
              variant={tool === "pen" ? "secondary" : "ghost"}
              aria-pressed={tool === "pen"}
              disabled={!draft || editingBusy}
              onClick={() => {
                setTool("pen");
                setSelected([]);
              }}
            >
              <PenLine className="size-4" />
              Pen
            </Button>
            <Button
              type="button"
              variant={tool === "eraser" ? "secondary" : "ghost"}
              aria-pressed={tool === "eraser"}
              disabled={!draft || editingBusy}
              onClick={() => {
                setTool("eraser");
                setSelected([]);
              }}
            >
              <Eraser className="size-4" />
              Eraser
            </Button>
            <Button
              type="button"
              variant={tool === "lasso" ? "secondary" : "ghost"}
              aria-pressed={tool === "lasso"}
              disabled={!draft || editingBusy}
              onClick={() => setTool("lasso")}
            >
              <Lasso className="size-4" /> Lasso
            </Button>
            <div className="mx-1 h-6 border-l" />
            <div className="flex gap-1" role="group" aria-label="Paper background">
              {(["plain", "grid"] as const).map((background) => (
                <Button
                  key={background}
                  variant={draft?.background === background ? "secondary" : "ghost"}
                  aria-label={background === "grid" ? "Grid paper" : "Plain paper"}
                  aria-pressed={draft?.background === background}
                  disabled={!draft || editingBusy}
                  onClick={() => {
                    if (draft && draft.background !== background) change({ ...draft, background });
                  }}
                >
                  {background === "grid" ? (
                    <Grid2X2 className="size-4" />
                  ) : (
                    <Square className="size-4" />
                  )}
                </Button>
              ))}
            </div>
            <div className="ml-auto flex gap-1">
              <Button
                type="button"
                variant="ghost"
                aria-label="Undo stroke"
                disabled={editingBusy || !canUndo}
                onClick={undoStroke}
              >
                <Undo2 className="size-4" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                aria-label="Redo stroke"
                disabled={editingBusy || !canRedo}
                onClick={redoStroke}
              >
                <Redo2 className="size-4" />
              </Button>
            </div>
          </div>
          <div
            role="toolbar"
            aria-label="Tool options"
            className="flex min-h-12 shrink-0 flex-wrap items-center gap-1 border-b px-3 py-1"
          >
            {tool === "pen" && (
              <>
                <span className="mr-2 text-xs text-muted-foreground">Pen width</span>
                {[2, 3, 5].map((value) => (
                  <Button
                    key={value}
                    variant={width === value ? "secondary" : "ghost"}
                    aria-label={`Pen width ${value}`}
                    aria-pressed={width === value}
                    disabled={editingBusy}
                    onClick={() => setWidth(value)}
                    className="w-10 px-0"
                  >
                    <span
                      className="block rounded-full bg-current"
                      style={{ width: value * 2, height: value * 2 }}
                    />
                  </Button>
                ))}
              </>
            )}
            {tool === "eraser" && (
              <>
                {(
                  [
                    ["stroke", "Stroke"],
                    ["pixel", "Pixel"],
                    ["lasso", "Lasso"],
                  ] as const
                ).map(([value, label]) => (
                  <Button
                    key={value}
                    variant={eraserMode === value ? "secondary" : "ghost"}
                    aria-pressed={eraserMode === value}
                    aria-label={`${label} eraser`}
                    disabled={editingBusy}
                    onClick={() => setEraserMode(value)}
                  >
                    {label}
                  </Button>
                ))}
                {eraserMode !== "lasso" && (
                  <div
                    className="flex items-center gap-1 border-l pl-2"
                    role="group"
                    aria-label="Eraser size"
                  >
                    {(
                      [
                        [6, "Small"],
                        [12, "Medium"],
                        [24, "Large"],
                      ] as const
                    ).map(([value, label]) => (
                      <Button
                        key={value}
                        variant={eraserRadius === value ? "secondary" : "ghost"}
                        aria-label={`${label} eraser size`}
                        aria-pressed={eraserRadius === value}
                        disabled={editingBusy}
                        onClick={() => setEraserRadius(value)}
                        className="w-9 px-0"
                      >
                        <span
                          className="rounded-full border border-current"
                          style={{ width: value, height: value }}
                        />
                      </Button>
                    ))}
                  </div>
                )}
                <Button
                  className="ml-auto"
                  variant="ghost"
                  disabled={editingBusy || !draft?.strokes.length}
                  onClick={() => {
                    if (draft) {
                      change({ ...draft, strokes: [] });
                      setSelected([]);
                      setLimit(false);
                    }
                  }}
                >
                  <Trash2 className="size-4" /> Clear sheet
                </Button>
              </>
            )}
            {tool === "lasso" && (
              <>
                {(
                  [
                    ["free", "Freehand"],
                    ["rectangle", "Rectangle"],
                  ] as const
                ).map(([value, label]) => (
                  <Button
                    key={value}
                    variant={lassoMode === value ? "secondary" : "ghost"}
                    aria-pressed={lassoMode === value}
                    disabled={editingBusy}
                    onClick={() => {
                      setLassoMode(value);
                      setSelected([]);
                    }}
                  >
                    {label}
                  </Button>
                ))}
                {selected.length > 0 ? (
                  <div className="flex flex-wrap items-center gap-1 border-l pl-2">
                    <span className="px-2 text-xs">{selected.length} selected · drag to move</span>
                    <Button
                      variant="ghost"
                      aria-label="Copy selection"
                      disabled={editingBusy}
                      onClick={() => {
                        if (!draft) return;
                        const source = draft.strokes.filter((stroke) =>
                          selected.includes(stroke.id),
                        );
                        if (
                          [...draft.strokes, ...source].reduce(
                            (n, stroke) => n + stroke.points.length,
                            0,
                          ) > MAX_INK_POINTS
                        ) {
                          setLimit(true);
                          return;
                        }
                        const copies = source.map((stroke) => ({
                          ...stroke,
                          id: crypto.randomUUID(),
                        }));
                        const ids = copies.map((stroke) => stroke.id);
                        change({
                          ...draft,
                          strokes: [...draft.strokes, ...moveSelection(copies, ids, 25, 25)],
                        });
                        setSelected(ids);
                      }}
                    >
                      <Copy className="size-4" />
                    </Button>
                    {(
                      [
                        [0.9, "Shrink selection", Minus],
                        [1.1, "Enlarge selection", Plus],
                      ] as const
                    ).map(([factor, label, Icon]) => (
                      <Button
                        key={label}
                        variant="ghost"
                        aria-label={label}
                        disabled={editingBusy}
                        onClick={() => {
                          if (draft) {
                            const strokes = scaleSelection(draft.strokes, selected, factor);
                            if (strokes !== draft.strokes) change({ ...draft, strokes });
                          }
                        }}
                      >
                        <Icon className="size-4" />
                      </Button>
                    ))}
                    <Button
                      variant="ghost"
                      aria-label="Delete selection"
                      disabled={editingBusy}
                      onClick={() => {
                        if (draft)
                          change({
                            ...draft,
                            strokes: draft.strokes.filter(
                              (stroke) => !selected.includes(stroke.id),
                            ),
                          });
                        setSelected([]);
                      }}
                    >
                      <Trash2 className="size-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      aria-label="Deselect"
                      disabled={editingBusy}
                      onClick={() => setSelected([])}
                    >
                      <X className="size-4" />
                    </Button>
                  </div>
                ) : (
                  <span className="px-2 text-xs text-muted-foreground">
                    Draw around handwriting to select it
                  </span>
                )}
              </>
            )}
          </div>
        </>
      ) : (
        <p role="status" className="shrink-0 border-b px-3 py-2 text-xs text-muted-foreground">
          {access === "other_pane"
            ? "This note is open for writing in another pane. Close it there to edit here."
            : "Connect a pen or enable mouse drawing in Settings to edit."}
        </p>
      )}
      <div
        data-ink-viewport
        className="min-h-0 flex-1 overflow-auto overscroll-contain bg-neutral-100 px-2 py-3 sm:px-6"
      >
        {loadError ? (
          <p role="alert" className="mx-auto max-w-xl p-6 text-red-800">
            Could not open this note. It has not been changed. {loadError}
          </p>
        ) : draft ? (
          <div className="mx-auto w-full max-w-[900px] border border-neutral-300 bg-white">
            <InkCanvas
              draft={draft}
              mono={monoInk}
              disabled={!editing || suspended || historyBusy || leaving}
              tool={tool}
              eraserMode={eraserMode}
              eraserRadius={eraserRadius}
              lassoMode={lassoMode}
              selected={selected}
              onSelectionChange={setSelected}
              width={width}
              mouseEnabled={mouseEnabled}
              nativeInk={capabilities.nativeDeviceEvents}
              onNativeStatus={setNativeStatus}
              onChange={change}
              onActiveChange={setActive}
              onLimit={() => setLimit(true)}
            />
          </div>
        ) : (
          <p role="status" className="p-6 text-center text-neutral-700">
            Opening your sheet…
          </p>
        )}
      </div>
      {(saveState instanceof Error || limit || nativeStatus?.error) && (
        <footer className="shrink-0 space-y-1 border-t px-4 py-2 text-xs">
          {saveState instanceof Error && (
            <div role="alert" className="text-destructive">
              Could not save your changes. {saveState.message}
              <button
                type="button"
                className="ml-2 underline"
                onClick={() => {
                  void getWriter(uuid)?.flush();
                }}
              >
                Retry saving
              </button>
            </div>
          )}
          {limit && (
            <p role="status">
              This sheet is full. Your completed strokes are kept; erase handwriting or undo to make
              room.
            </p>
          )}
          {nativeStatus?.error && (
            <p role="alert">Fast pen input is unavailable: {nativeStatus.error}</p>
          )}
        </footer>
      )}
      {conflictsOpen && status && (
        <HandwritingConflictDialog
          pageUuid={uuid}
          heads={status.heads}
          onClose={() => setConflictsOpen(false)}
          onResolved={conflictResolved}
        />
      )}
    </div>
  );
}

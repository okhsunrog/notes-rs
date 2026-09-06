import { useCallback, useEffect, useRef, useState } from "react";
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
  completeHandwritingNote,
  handwritingHistory,
  loadHandwritingNote,
  unknownErrorMessage,
  type Page,
} from "@/lib/api";
import type { InkDraft, InkHistorySnapshot } from "@/lib/bindings";
import { cn } from "@/lib/utils";
import type { DraftSaveState } from "./draft-writer";
import { beginSession, endSession, getWriter, requestCompletion } from "./handwriting-session";
import { InkCanvas, type InkTool } from "./ink-canvas";
import { moveSelection, scaleSelection, type EraserMode, type LassoMode } from "./ink-editing";
import { MAX_INK_POINTS } from "./ink-model";
import { applyHistoryUpdate } from "./ink-patch";
import { useHandwritingAvailability, useHandwritingPreference } from "./input-capabilities";
import type { OnyxInkStatus } from "./onyx-ink";

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
  const uuid = page.uuid;
  const { capabilities } = useHandwritingAvailability();
  const mouseEnabled = useHandwritingPreference((state) => state.mouseEnabled);
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

  const latestDraft = useRef<InkDraft | null>(null);
  const historyBusyRef = useRef(false);
  const leavingRef = useRef(false);

  const title = usePageTitleEditor(page, true, onSaved);
  const titleFlush = useRef(title.flush);
  titleFlush.current = title.flush;
  const busy = active || historyBusy || leaving;

  const adopt = useCallback(
    (history: InkHistorySnapshot) => {
      const { snapshot } = history;
      beginSession(uuid, snapshot, setSaveState);
      latestDraft.current = snapshot.draft;
      setDraft(snapshot.draft);
      setCanUndo(history.canUndo);
      setCanRedo(history.canRedo);
      setSaveState("saved");
      setSelected([]);
      setLimit(false);
    },
    [uuid],
  );

  useEffect(() => {
    let disposed = false;
    loadHandwritingNote(uuid, true)
      .then((history) => {
        if (!disposed) adopt(history);
      })
      .catch((error: unknown) => {
        if (!disposed) setLoadError(unknownErrorMessage(error));
      });
    return () => {
      disposed = true;
    };
  }, [adopt, uuid]);

  useEffect(() => {
    return () => {
      const writer = getWriter(uuid);
      if (!writer) return;
      void (async () => {
        // An unacknowledged gesture keeps the session alive so a later mount can
        // retry it; only a clean queue may be handed to completion and dropped.
        if (!(await writer.flush())) return;
        if (!leavingRef.current) {
          requestCompletion(uuid, () => completeHandwritingNote(uuid)).catch(() => {});
        }
        endSession(uuid);
      })();
    };
  }, [uuid]);

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
    if (writer) requestCompletion(uuid, () => completeHandwritingNote(uuid)).catch(() => {});
  }, [busy, navigateAway, uuid]);

  const change = (next: InkDraft) => {
    const previous = latestDraft.current;
    const writer = getWriter(uuid);
    if (!previous || !writer || next === previous || historyBusyRef.current || leavingRef.current) {
      return;
    }
    latestDraft.current = next;
    setCanUndo(true);
    setCanRedo(false);
    setDraft(next);
    void writer.write(next);
  };

  const navigateHistory = async (redo: boolean) => {
    if (busy || historyBusyRef.current || !(redo ? canRedo : canUndo)) return;
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
          onClick={() => void onDelete(page)}
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
      <div
        role="toolbar"
        aria-label="Handwriting tools"
        className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2"
      >
        <Button
          type="button"
          variant={tool === "pen" ? "secondary" : "ghost"}
          aria-pressed={tool === "pen"}
          disabled={!draft || busy}
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
          disabled={!draft || busy}
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
          disabled={!draft || busy}
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
              disabled={!draft || busy}
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
            disabled={busy || !canUndo}
            onClick={undoStroke}
          >
            <Undo2 className="size-4" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            aria-label="Redo stroke"
            disabled={busy || !canRedo}
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
                disabled={busy}
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
                disabled={busy}
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
                    disabled={busy}
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
              disabled={busy || !draft?.strokes.length}
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
                disabled={busy}
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
                  disabled={busy}
                  onClick={() => {
                    if (!draft) return;
                    const source = draft.strokes.filter((stroke) => selected.includes(stroke.id));
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
                    disabled={busy}
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
                  disabled={busy}
                  onClick={() => {
                    if (draft)
                      change({
                        ...draft,
                        strokes: draft.strokes.filter((stroke) => !selected.includes(stroke.id)),
                      });
                    setSelected([]);
                  }}
                >
                  <Trash2 className="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  aria-label="Deselect"
                  disabled={busy}
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
              disabled={historyBusy || leaving}
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
    </div>
  );
}

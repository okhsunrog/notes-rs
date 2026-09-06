import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Copy,
  Eraser,
  Grid2X2,
  Lasso,
  Minus,
  Plus,
  Square,
  PenLine,
  Redo2,
  Trash2,
  Undo2,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { compactHandwritingDraft, handwritingHistory, saveHandwritingPatch } from "@/lib/api";
import { notifyError } from "@/lib/notify";
import { completeDraft } from "./complete-draft";
import type { InkDraft, InkHistorySnapshot } from "@/lib/bindings";
import { registerBackOverlay } from "@/lib/back-overlays";
import { applyHistoryUpdate, incrementalDraftSaver } from "./ink-patch";
import { DraftWriter, type DraftSaveState } from "./draft-writer";
import { InkCanvas, type InkTool } from "./ink-canvas";
import { useHandwritingSession } from "./handwriting-session";
import { useHandwritingPreference, useHandwritingAvailability } from "./input-capabilities";
import { MAX_INK_POINTS } from "./ink-model";
import { moveSelection, scaleSelection, type EraserMode, type LassoMode } from "./ink-editing";
import type { OnyxInkStatus } from "./onyx-ink";

export function HandwritingSheet() {
  const setOpen = useHandwritingSession((state) => state.setOpen);
  const { capabilities } = useHandwritingAvailability();
  const [draft, setDraft] = useState<InkDraft | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveState, setSaveState] = useState<DraftSaveState>("saved");
  const [tool, setTool] = useState<InkTool>("pen");
  const [width, setWidth] = useState(3);
  const [eraserMode, setEraserMode] = useState<EraserMode>("stroke");
  const [eraserRadius, setEraserRadius] = useState(12);
  const [lassoMode, setLassoMode] = useState<LassoMode>("free");
  const [selected, setSelected] = useState<string[]>([]);
  const latestDraft = useRef<InkDraft | null>(null);
  const [active, setActive] = useState(false);
  const [closing, setClosing] = useState(false);
  const mouseEnabled = useHandwritingPreference((state) => state.mouseEnabled);
  const [nativeStatus, setNativeStatus] = useState<OnyxInkStatus | null>(null);
  const [limit, setLimit] = useState(false);
  const [canUndo, setCanUndo] = useState(false);
  const [historyBusy, setHistoryBusy] = useState(false);
  const historyBusyRef = useRef(false);
  const [canRedo, setCanRedo] = useState(false);
  const writer = useRef<DraftWriter | null>(null);

  const adopt = useCallback((history: InkHistorySnapshot) => {
    const { snapshot } = history;
    writer.current = new DraftWriter(
      snapshot.revision,
      incrementalDraftSaver(snapshot.draft, saveHandwritingPatch),
      setSaveState,
    );
    latestDraft.current = snapshot.draft;
    setDraft(snapshot.draft);
    setCanUndo(history.canUndo);
    setCanRedo(history.canRedo);
    setSaveState("saved");
  }, []);

  useEffect(() => {
    let disposed = false;
    void handwritingHistory(null, null)
      .then((history) => {
        if (!disposed) adopt(applyHistoryUpdate(null, null, history));
      })
      .catch((error: unknown) => {
        if (!disposed) setLoadError(String(error));
      });
    return () => {
      disposed = true;
    };
  }, [adopt]);

  const close = useCallback(async () => {
    if (active || closing || historyBusyRef.current) return;
    setClosing(true);
    const current = writer.current;
    if (!current) {
      setOpen(false);
      return;
    }
    try {
      const completed = await completeDraft(current, compactHandwritingDraft, () => setOpen(false));
      if (!completed) setClosing(false);
    } catch (error) {
      notifyError("Handwriting compaction", error);
      setClosing(false);
    }
  }, [active, closing, setOpen]);

  useEffect(() => {
    const complete = () => {
      const current = writer.current;
      if (current)
        void completeDraft(current, compactHandwritingDraft).catch((error: unknown) =>
          notifyError("Handwriting compaction", error),
        );
    };
    const visibility = () => {
      if (document.visibilityState === "hidden") complete();
    };
    document.addEventListener("visibilitychange", visibility);
    window.addEventListener("pagehide", complete);
    return () => {
      document.removeEventListener("visibilitychange", visibility);
      window.removeEventListener("pagehide", complete);
    };
  }, []);

  useEffect(
    () =>
      registerBackOverlay(() => {
        void close();
      }),
    [close],
  );

  const change = (next: InkDraft) => {
    const previous = latestDraft.current;
    if (!previous || !writer.current || next === previous || historyBusyRef.current || closing)
      return;
    latestDraft.current = next;
    setCanUndo(true);
    setCanRedo(false);
    setDraft(next);
    void writer.current.write(next);
  };

  const navigateHistory = async (redo: boolean) => {
    if (active || closing || historyBusyRef.current || !(redo ? canRedo : canUndo)) return;
    historyBusyRef.current = true;
    setHistoryBusy(true);
    try {
      const current = writer.current;
      if (!current || !(await current.flush())) return;
      const history = await handwritingHistory(redo, current.getRevision());
      adopt(applyHistoryUpdate(latestDraft.current, current.getRevision(), history));
      setSelected([]);
      setLimit(false);
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
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) void close();
      }}
    >
      <DialogContent
        data-fullscreen="true"
        showCloseButton={false}
        className="ink-dialog fixed inset-0 flex h-dvh w-screen max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none border-0 p-0 sm:max-w-none pb-[var(--safe-area-inset-bottom)]"
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
            event.preventDefault();
            event.stopPropagation();
            if (event.shiftKey) redoStroke();
            else undoStroke();
          }
        }}
      >
        <header className="flex shrink-0 items-center gap-3 border-b px-3 pt-[calc(0.5rem+var(--safe-area-inset-top))] pb-2">
          <Button
            type="button"
            variant="ghost"
            onClick={() => void close()}
            disabled={active || closing || historyBusy}
            aria-busy={closing}
          >
            <ArrowLeft className="size-4" />
            Back
          </Button>
          <div className="min-w-0">
            <DialogTitle>Handwriting</DialogTitle>
            <DialogDescription className="mt-1 text-xs">
              Local draft · not yet added to your notes
            </DialogDescription>
          </div>
        </header>
        <div
          role="toolbar"
          aria-label="Handwriting tools"
          className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2"
        >
          <Button
            type="button"
            variant={tool === "pen" ? "secondary" : "ghost"}
            aria-pressed={tool === "pen"}
            disabled={!draft || active || historyBusy || closing}
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
            disabled={!draft || active || historyBusy || closing}
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
            disabled={!draft || active || historyBusy || closing}
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
                disabled={!draft || active || historyBusy || closing}
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
              disabled={active || historyBusy || closing || !canUndo}
              onClick={undoStroke}
            >
              <Undo2 className="size-4" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              aria-label="Redo stroke"
              disabled={active || historyBusy || closing || !canRedo}
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
                  disabled={active || historyBusy || closing}
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
                  disabled={active || historyBusy || closing}
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
                      disabled={active || historyBusy || closing}
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
                disabled={active || historyBusy || closing || !draft?.strokes.length}
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
                  disabled={active || historyBusy || closing}
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
                    disabled={active || historyBusy || closing}
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
                      disabled={active || historyBusy || closing}
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
                    disabled={active || historyBusy || closing}
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
                    disabled={active || historyBusy || closing}
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
              Could not load the saved draft. It has not been changed. {loadError}
            </p>
          ) : draft ? (
            <div className="mx-auto w-full max-w-[900px] border border-neutral-300 bg-white">
              <InkCanvas
                draft={draft}
                disabled={historyBusy || closing}
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
                    void writer.current?.flush();
                  }}
                >
                  Retry saving
                </button>
              </div>
            )}
            {limit && (
              <p role="status">
                This sheet is full. Your completed strokes are kept; erase handwriting or undo to
                make room.
              </p>
            )}
            {nativeStatus?.error && (
              <p role="alert">Fast pen input is unavailable: {nativeStatus.error}</p>
            )}
          </footer>
        )}
      </DialogContent>
    </Dialog>
  );
}

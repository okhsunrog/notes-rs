import { useCallback, useEffect, useRef, useState } from "react";
import {
  Check,
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
import { loadHandwritingDraft, saveHandwritingPatch } from "@/lib/api";
import type { InkDraft } from "@/lib/bindings";
import { registerBackOverlay } from "@/lib/back-overlays";
import { incrementalDraftSaver } from "./ink-patch";
import { DraftWriter, type DraftSaveState } from "./draft-writer";
import { InkCanvas, type InkMetrics, type InkTool } from "./ink-canvas";
import { useHandwritingSession } from "./handwriting-session";
import { useHandwritingAvailability } from "./input-capabilities";
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
  const [mouseEnabled, setMouseEnabled] = useState(false);
  const [metrics, setMetrics] = useState<InkMetrics | null>(null);
  const [nativeStatus, setNativeStatus] = useState<OnyxInkStatus | null>(null);
  const [limit, setLimit] = useState(false);
  const [undo, setUndo] = useState<InkDraft[]>([]);
  const [redo, setRedo] = useState<InkDraft[]>([]);
  const writer = useRef<DraftWriter | null>(null);

  useEffect(() => {
    let disposed = false;
    void loadHandwritingDraft()
      .then((snapshot) => {
        if (disposed) return;
        writer.current = new DraftWriter(
          snapshot.revision,
          incrementalDraftSaver(snapshot.draft, saveHandwritingPatch),
          (state) => {
            if (!disposed) setSaveState(state);
          },
        );
        latestDraft.current = snapshot.draft;
        setDraft(snapshot.draft);
      })
      .catch((error: unknown) => {
        if (!disposed) setLoadError(String(error));
      });
    return () => {
      disposed = true;
    };
  }, []);

  const close = useCallback(async () => {
    if (active || closing) return;
    setClosing(true);
    const saved = writer.current ? await writer.current.flush() : true;
    if (saved) setOpen(false);
    else setClosing(false);
  }, [active, closing, setOpen]);

  useEffect(
    () =>
      registerBackOverlay(() => {
        void close();
      }),
    [close],
  );

  const change = (next: InkDraft) => {
    const previous = latestDraft.current;
    if (!previous || !writer.current || next === previous) return;
    latestDraft.current = next;
    setUndo((history) => [...history.slice(-49), previous]);
    setRedo([]);
    setDraft(next);
    void writer.current.write(next);
  };

  const undoStroke = () => {
    const previous = undo[undo.length - 1];
    if (!draft || !previous || active) return;
    setUndo(undo.slice(0, -1));
    setRedo([...redo, draft]);
    latestDraft.current = previous;
    setSelected([]);
    setDraft(previous);
    setLimit(false);
    void writer.current?.write(previous);
  };

  const redoStroke = () => {
    const next = redo[redo.length - 1];
    if (!draft || !next || active) return;
    setRedo(redo.slice(0, -1));
    setUndo([...undo, draft]);
    latestDraft.current = next;
    setSelected([]);
    setDraft(next);
    void writer.current?.write(next);
  };

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
        className="ink-dialog fixed inset-0 flex h-dvh w-screen max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none border-0 p-0 sm:max-w-none"
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
            event.preventDefault();
            event.stopPropagation();
            if (event.shiftKey) redoStroke();
            else undoStroke();
          }
        }}
      >
        <header className="flex shrink-0 items-center justify-between gap-3 border-b px-4 pt-[calc(0.75rem+var(--safe-area-inset-top))] pb-3">
          <div className="min-w-0">
            <DialogTitle>Handwriting</DialogTitle>
            <DialogDescription className="mt-1 text-xs">
              Local draft · not yet added to your notes
            </DialogDescription>
          </div>
          <Button
            type="button"
            variant="outline"
            onClick={() => void close()}
            disabled={active || closing}
          >
            {loadError ? <X className="size-4" /> : <Check className="size-4" />}
            {loadError ? "Close" : closing ? "Saving…" : "Done"}
          </Button>
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
            disabled={!draft || active}
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
            disabled={!draft || active}
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
            disabled={!draft || active}
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
                disabled={!draft || active}
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
              disabled={active || undo.length === 0}
              onClick={undoStroke}
            >
              <Undo2 className="size-4" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              aria-label="Redo stroke"
              disabled={active || redo.length === 0}
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
                  disabled={active}
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
                  disabled={active}
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
                      disabled={active}
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
                disabled={active || !draft?.strokes.length}
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
                  disabled={active}
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
                    disabled={active}
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
                      disabled={active}
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
                    disabled={active}
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
                    disabled={active}
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
                onMetrics={setMetrics}
                onLimit={() => setLimit(true)}
              />
            </div>
          ) : (
            <p role="status" className="p-6 text-center text-neutral-700">
              Opening your sheet…
            </p>
          )}
        </div>
        <footer className="shrink-0 space-y-1 border-t px-4 pt-2 pb-[calc(0.5rem+var(--safe-area-inset-bottom))] text-xs">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span role="status">
              {!draft
                ? "Opening draft…"
                : saveState instanceof Error
                  ? "Draft not saved"
                  : saveState === "saving"
                    ? "Saving on this device…"
                    : "Saved on this device"}
            </span>
            <span>{draft?.strokes.length ?? 0} strokes</span>
          </div>
          {saveState instanceof Error && (
            <div role="alert" className="text-destructive">
              {saveState.message}
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
              This test sheet has reached its point limit. Your completed strokes are kept; use the
              eraser or undo to make room.
            </p>
          )}
          <details>
            <summary className="cursor-pointer py-1 text-muted-foreground">Input details</summary>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 py-1 text-muted-foreground">
              <span>Pen: {capabilities.stylus.replace(/_/g, " ")}</span>
              <span>Ink: {nativeStatus?.available ? "BOOX Pen SDK" : "Web canvas"}</span>
              {nativeStatus?.error && (
                <span role="alert">BOOX ink unavailable: {nativeStatus.error}</span>
              )}
              <span>Pressure: {metrics ? `${Math.round(metrics.pressure * 100)}%` : "—"}</span>
              <span>Tilt: {metrics ? `${metrics.tiltX}°, ${metrics.tiltY}°` : "—"}</span>
              <span>Input: {metrics?.tool ?? "—"}</span>
              <span>Stroke samples: {metrics?.samples ?? 0}</span>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={mouseEnabled}
                  disabled={active}
                  onChange={(event) => setMouseEnabled(event.target.checked)}
                />
                Allow mouse drawing
              </label>
            </div>
          </details>
        </footer>
      </DialogContent>
    </Dialog>
  );
}

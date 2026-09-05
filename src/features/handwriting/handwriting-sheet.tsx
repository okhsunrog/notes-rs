import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Eraser, PenLine, Redo2, Undo2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { loadHandwritingDraft, saveHandwritingDraft } from "@/lib/api";
import type { InkDraft } from "@/lib/bindings";
import { registerBackOverlay } from "@/lib/back-overlays";
import { DraftWriter, type DraftSaveState } from "./draft-writer";
import { InkCanvas, type InkMetrics, type InkTool } from "./ink-canvas";
import { useHandwritingSession } from "./handwriting-session";
import { useHandwritingAvailability } from "./input-capabilities";

export function HandwritingSheet() {
  const setOpen = useHandwritingSession((state) => state.setOpen);
  const { capabilities } = useHandwritingAvailability();
  const [draft, setDraft] = useState<InkDraft | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveState, setSaveState] = useState<DraftSaveState>("saved");
  const [tool, setTool] = useState<InkTool>("pen");
  const [width, setWidth] = useState(3);
  const [active, setActive] = useState(false);
  const [closing, setClosing] = useState(false);
  const [mouseEnabled, setMouseEnabled] = useState(false);
  const [metrics, setMetrics] = useState<InkMetrics | null>(null);
  const [limit, setLimit] = useState(false);
  const [undo, setUndo] = useState<InkDraft[]>([]);
  const [redo, setRedo] = useState<InkDraft[]>([]);
  const writer = useRef<DraftWriter | null>(null);

  useEffect(() => {
    let disposed = false;
    void loadHandwritingDraft()
      .then((snapshot) => {
        if (disposed) return;
        writer.current = new DraftWriter(snapshot.revision, saveHandwritingDraft, (state) => {
          if (!disposed) setSaveState(state);
        });
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
    if (!draft || !writer.current) return;
    setUndo((history) => [...history.slice(-49), draft]);
    setRedo([]);
    setDraft(next);
    void writer.current.write(next);
  };

  const undoStroke = () => {
    const previous = undo[undo.length - 1];
    if (!draft || !previous || active) return;
    setUndo(undo.slice(0, -1));
    setRedo([...redo, draft]);
    setDraft(previous);
    setLimit(false);
    void writer.current?.write(previous);
  };

  const redoStroke = () => {
    const next = redo[redo.length - 1];
    if (!draft || !next || active) return;
    setRedo(redo.slice(0, -1));
    setUndo([...undo, draft]);
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
            onClick={() => setTool("pen")}
          >
            <PenLine className="size-4" />
            Pen
          </Button>
          <Button
            type="button"
            variant={tool === "eraser" ? "secondary" : "ghost"}
            aria-pressed={tool === "eraser"}
            disabled={!draft || active}
            onClick={() => setTool("eraser")}
          >
            <Eraser className="size-4" />
            Eraser
          </Button>
          <div className="mx-1 h-6 border-l" />
          {[2, 3, 5].map((value) => (
            <Button
              key={value}
              type="button"
              variant={width === value ? "secondary" : "ghost"}
              aria-label={`Pen width ${value}`}
              aria-pressed={width === value}
              disabled={active}
              onClick={() => {
                setWidth(value);
                setTool("pen");
              }}
              className="w-9 px-0"
            >
              <span
                className="block rounded-full bg-current"
                style={{ width: value * 2, height: value * 2 }}
              />
            </Button>
          ))}
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
        <div className="min-h-0 flex-1 overflow-auto overscroll-contain bg-neutral-100 px-2 py-3 sm:px-6">
          {loadError ? (
            <p role="alert" className="mx-auto max-w-xl p-6 text-red-800">
              Could not load the saved draft. It has not been changed. {loadError}
            </p>
          ) : draft ? (
            <div className="mx-auto w-full max-w-[900px] border border-neutral-300 bg-white">
              <InkCanvas
                draft={draft}
                tool={tool}
                width={width}
                mouseEnabled={mouseEnabled}
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

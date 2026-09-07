import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { addPluginListener, invoke } from "@tauri-apps/api/core";
import type { InkDraft, InkPoint } from "@/lib/bindings";
import type { InkMetrics, InkTool } from "./ink-canvas";
import type { Bounds, LassoMode } from "./ink-editing";
import { eraseAt, MAX_INK_POINTS } from "./ink-model";

export type OnyxInkEvent = {
  session: string;
  kind: "begin" | "end" | "cancel" | "stroke" | "preview";
  sequence: number;
  width: number;
  erasing: boolean;
  fastPreview?: boolean;
  points?: InkPoint[];
};
export type OnyxInkStatus = { available: boolean; active: boolean; error?: string };
/**
 * A DOM control floating inside the sheet, in CSS pixels relative to the canvas box. It reaches the
 * plugin in the canvas' own coordinate space, so scrolling needs no re-measurement. The plugin
 * swallows a pen gesture that starts on one instead of cutting the rectangle out of the handwriting
 * region: a hole in the region is also a hole in every trace crossing it.
 */
export type InkOverlayRect = { left: number; top: number; width: number; height: number };

export function applyOnyxStroke(draft: InkDraft, event: OnyxInkEvent): InkDraft {
  const points = event.points ?? [];
  if (!points.length) return draft;
  if (event.erasing) {
    const strokes = points.reduce((strokes, point) => eraseAt(strokes, point), draft.strokes);
    return strokes.length === draft.strokes.length ? draft : { ...draft, strokes };
  }
  const remaining =
    MAX_INK_POINTS - draft.strokes.reduce((count, stroke) => count + stroke.points.length, 0);
  if (remaining <= 0) return draft;
  return {
    ...draft,
    strokes: [
      ...draft.strokes,
      { id: crypto.randomUUID(), width: event.width, points: points.slice(0, remaining) },
    ],
  };
}

/** The SDK owns live pen input; the portable canvas owns completed strokes and persistence. */
export function useOnyxInk({
  canvasRef,
  enabled,
  draft,
  tool,
  width,
  onChange,
  onActiveChange,
  onMetrics,
  onLimit,
  onStatus,
  onInput,
  onStroke,
  decoration,
  lassoMode,
  selection,
  overlayRects,
  interacting = false,
  damage,
}: {
  canvasRef: RefObject<HTMLCanvasElement | null>;
  enabled: boolean;
  draft: InkDraft;
  tool: InkTool;
  width: number;
  onChange: (draft: InkDraft) => void;
  onActiveChange: (active: boolean) => void;
  onMetrics?: (metrics: InkMetrics) => void;
  onLimit: () => void;
  onStatus?: (status: OnyxInkStatus) => void;
  decoration?: string;
  lassoMode?: LassoMode;
  selection?: Bounds | null;
  overlayRects?: InkOverlayRect[];
  /** A local gesture owns the sheet: reconfiguring now would pause raw drawing and cancel it. */
  interacting?: boolean;
  damage?: { take: () => Bounds | null; invalidate: () => void };
  onInput?: (event: OnyxInkEvent, draft: InkDraft) => void;
  onStroke?: (draft: InkDraft, event: OnyxInkEvent) => InkDraft;
}) {
  const [native, setNative] = useState(false);
  const [frame, setFrame] = useState(0);
  const state = useRef({
    draft,
    tool,
    width,
    onChange,
    onActiveChange,
    onMetrics,
    onLimit,
    onStatus,
    onInput,
    onStroke,
    lassoMode,
    selection,
    overlayRects,
    interacting,
    damage,
  });
  const session = useRef<string | null>(null);
  const sequence = useRef(0);
  const update = useRef<(() => void) | null>(null);
  useLayoutEffect(() => {
    state.current = {
      draft,
      tool,
      width,
      onChange,
      onActiveChange,
      onMetrics,
      onLimit,
      onStatus,
      onInput,
      onStroke,
      lassoMode,
      selection,
      overlayRects,
      interacting,
      damage,
    };
  });

  useEffect(() => {
    if (!enabled) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const id = crypto.randomUUID();
    session.current = id;
    sequence.current = 0;
    let disposed = false;
    let registered = false;
    let unsupported = false;
    let remove: (() => Promise<void>) | undefined;
    let queue = Promise.resolve();
    let lastConfig = "";
    let previewFrame: number | undefined;
    let latestPreview: OnyxInkEvent | undefined;
    let gestureOpen = false;
    const cancelPreview = () => {
      if (previewFrame !== undefined) cancelAnimationFrame(previewFrame);
      previewFrame = undefined;
      latestPreview = undefined;
    };
    const status = (value: OnyxInkStatus) => {
      if (!disposed) {
        setNative(value.available);
        state.current.onStatus?.(value);
      }
    };
    let deferredConfigure = false;
    const configure = () => {
      if (disposed || !registered || unsupported) return;
      // Reconfiguring pauses raw drawing, which cancels the stroke in flight and truncates its
      // trace. A gesture owns the sheet until it lifts; the change waits for it. `interacting`
      // is the React prop and lags a render behind the pen; `gestureOpen` is set synchronously
      // by the native begin/end events, so it is what actually protects the trace.
      if (state.current.interacting || gestureOpen) {
        deferredConfigure = true;
        return;
      }
      const rect = canvas.getBoundingClientRect();
      const viewport = canvas.closest("[data-ink-viewport]")?.getBoundingClientRect();
      const overlays = (state.current.overlayRects ?? [])
        .filter((box) => box.width > 0 && box.height > 0)
        .map((box) => ({
          left: rect.left + box.left,
          top: rect.top + box.top,
          width: box.width,
          height: box.height,
        }));
      const args = {
        session: id,
        enabled: document.visibilityState !== "hidden" && rect.width > 0 && rect.height > 0,
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
        clipTop: Math.max(rect.top, viewport?.top ?? 0, 0),
        clipBottom: Math.min(
          rect.bottom,
          viewport?.bottom ?? window.innerHeight,
          window.innerHeight,
        ),
        viewportWidth: window.innerWidth,
        strokeWidth: state.current.width,
        eraser: state.current.tool === "eraser",
        interaction: state.current.tool === "lasso",
        fastLasso: state.current.tool === "lasso" && state.current.lassoMode === "free",
        hasSelection: !!state.current.selection,
        selectionLeft: state.current.selection?.left ?? 0,
        selectionTop: state.current.selection?.top ?? 0,
        selectionRight: state.current.selection?.right ?? 0,
        selectionBottom: state.current.selection?.bottom ?? 0,
        // Floating DOM controls inside the sheet, so a pen landing on one taps it instead of
        // inking. The handwriting region itself stays whole, or the trace would be cut with it.
        overlayRects: overlays,
      };
      const key = JSON.stringify(args);
      if (key === lastConfig) return;
      lastConfig = key;
      queue = queue
        .then(async () => {
          if (disposed) return;
          const result = await invoke<OnyxInkStatus>(
            "plugin:mobile-system|configure_onyx_ink",
            args,
          );
          unsupported = !result.available;
          status(result);
        })
        .catch(async (error: unknown) => {
          unsupported = true;
          await invoke("plugin:mobile-system|configure_onyx_ink", {
            session: id,
            enabled: false,
          }).catch(() => {});
          status({ available: false, active: false, error: String(error) });
        });
    };
    update.current = configure;
    void addPluginListener<OnyxInkEvent>("mobile-system", "onyxInk", (event) => {
      if (disposed || event.session !== id) return;
      const current = state.current;
      if (event.kind === "preview") {
        if (!gestureOpen) return;
        latestPreview = event;
        if (previewFrame === undefined)
          previewFrame = requestAnimationFrame(() => {
            previewFrame = undefined;
            const preview = latestPreview;
            latestPreview = undefined;
            if (!disposed && gestureOpen && preview)
              state.current.onInput?.(preview, state.current.draft);
          });
        return;
      }
      // Completed gestures use the full SDK point list, never a pending preview frame.
      cancelPreview();
      gestureOpen = event.kind === "begin";
      if (event.kind !== "stroke") current.onInput?.(event, current.draft);
      if (event.kind === "begin") current.onActiveChange(true);
      else if (event.kind === "end" || event.kind === "cancel") {
        current.onActiveChange(false);
        setFrame((n) => n + 1);
        // A geometry/overlay change that arrived mid-gesture was held back; apply it now.
        if (deferredConfigure) {
          deferredConfigure = false;
          configure();
        }
      } else if (event.kind === "stroke" && event.sequence > sequence.current) {
        sequence.current = event.sequence;
        const next = current.onStroke
          ? current.onStroke(current.draft, event)
          : applyOnyxStroke(current.draft, event);
        if (next !== current.draft) {
          // Native batches can arrive before React renders the previous one.
          current.draft = next;
          current.onChange(next);
        }
        const last = event.points?.[event.points.length - 1];
        if (last)
          current.onMetrics?.({
            tool: "BOOX Pen SDK",
            pressure: last.pressure,
            tiltX: last.tiltX,
            tiltY: last.tiltY,
            samples: event.points!.length,
          });
        if (
          next.strokes.reduce((count, stroke) => count + stroke.points.length, 0) >= MAX_INK_POINTS
        )
          current.onLimit();
        setFrame((n) => n + 1);
      }
    })
      .then((listener) => {
        if (disposed) void listener.unregister();
        else {
          remove = () => listener.unregister();
          registered = true;
          configure();
        }
      })
      .catch((error: unknown) => status({ available: false, active: false, error: String(error) }));
    const resize = new ResizeObserver(configure);
    resize.observe(canvas);
    const viewport = canvas.closest("[data-ink-viewport]");
    if (viewport) resize.observe(viewport);
    window.addEventListener("scroll", configure, true);
    window.addEventListener("resize", configure);
    document.addEventListener("visibilitychange", configure);
    return () => {
      disposed = true;
      cancelPreview();
      session.current = null;
      update.current = null;
      resize.disconnect();
      window.removeEventListener("scroll", configure, true);
      window.removeEventListener("resize", configure);
      document.removeEventListener("visibilitychange", configure);
      void remove?.();
      void queue
        .then(() =>
          invoke("plugin:mobile-system|configure_onyx_ink", { session: id, enabled: false }),
        )
        .catch(console.error);
    };
  }, [canvasRef, enabled]);

  const overlayKey = JSON.stringify(overlayRects ?? []);
  useEffect(() => {
    update.current?.();
  }, [
    tool,
    width,
    lassoMode,
    overlayKey,
    interacting,
    selection?.left,
    selection?.top,
    selection?.right,
    selection?.bottom,
  ]);
  useEffect(() => {
    if (!native || !session.current) return;
    const id = session.current;
    const currentSequence = sequence.current;
    // The canvas draws in a layout effect, before this compositor acknowledgement.
    const handle = requestAnimationFrame(() => {
      const damage = state.current.damage;
      const bounds = damage?.take();
      void invoke("plugin:mobile-system|commit_onyx_frame", {
        session: id,
        sequence: currentSequence,
        partial: !!damage,
        left: bounds?.left ?? 0,
        top: bounds?.top ?? 0,
        right: bounds?.right ?? 0,
        bottom: bounds?.bottom ?? 0,
      }).catch((error: unknown) => {
        damage?.invalidate();
        console.error(error);
      });
    });
    return () => cancelAnimationFrame(handle);
  }, [draft, frame, native, decoration, tool]);
  return enabled && native;
}

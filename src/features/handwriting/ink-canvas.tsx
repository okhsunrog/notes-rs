import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import type { InkDraft, InkPoint, InkStroke } from "@/lib/bindings";
import { Copy, Minus, Plus, Trash2, X } from "lucide-react";
import { drawSegment, drawSheet, inkPoint, MAX_INK_POINTS } from "./ink-model";
import { changedInkBounds, drawInkRegion, unionBounds, SHEET_BOUNDS } from "./ink-region";
import {
  eraseGesture,
  lassoPolygon,
  moveSelection,
  scaleSelection,
  selectionBounds,
  selectionFrame,
  selectionMenuPosition,
  selectLasso,
  SELECTION_DASH,
  SELECTION_FRAME_PADDING,
  SELECTION_FRAME_WIDTH,
  SELECTION_TRACE_WIDTH,
  type Bounds,
  type EraserMode,
  type LassoMode,
} from "./ink-editing";
import { applyOnyxStroke, useOnyxInk, type OnyxInkEvent, type OnyxInkStatus } from "./onyx-ink";

export type InkMetrics = {
  tool: string;
  pressure: number;
  tiltX: number;
  tiltY: number;
  samples: number;
};
export type InkTool = "pen" | "eraser" | "lasso";
type Gesture = {
  id: number;
  action: "pen" | "erase" | "select" | "move";
  points: InkPoint[];
  base: InkStroke[];
  preview: InkStroke[];
  ids: string[];
  count: number;
  rect?: DOMRect;
  lastMetrics: number;
  nativeFast: boolean;
  /** A finger drag of the selection, which a second pointer abandons. */
  finger?: boolean;
  backdrop?: InkStroke[];
  movingInk?: InkStroke[];
  movingBitmap?: HTMLCanvasElement;
};
const NO_SELECTION: string[] = [];
/** Grab margin around the selection, kept equal to `movingSelection` in `OnyxInk.kt`. */
const SELECTION_GRAB = 12;

/** What the popover used to offer, within reach of the selection itself. */
const SELECTION_ACTIONS = [
  { id: "delete", label: "Delete selection", Icon: Trash2 },
  { id: "copy", label: "Copy selection", Icon: Copy },
  { id: "shrink", label: "Shrink selection", Icon: Minus },
  { id: "enlarge", label: "Enlarge selection", Icon: Plus },
  { id: "deselect", label: "Deselect", Icon: X },
] as const;
type SelectionAction = (typeof SELECTION_ACTIONS)[number]["id"];

function withinSelection(bounds: Bounds | null, point: InkPoint): boolean {
  return (
    !!bounds &&
    point.x >= bounds.left - SELECTION_GRAB &&
    point.x <= bounds.right + SELECTION_GRAB &&
    point.y >= bounds.top - SELECTION_GRAB &&
    point.y <= bounds.bottom + SELECTION_GRAB
  );
}

export function InkCanvas({
  draft,
  disabled = false,
  tool,
  width,
  mouseEnabled,
  onChange,
  onActiveChange,
  onMetrics,
  onLimit,
  nativeInk = false,
  onNativeStatus,
  eraserMode = "stroke",
  eraserRadius = 12,
  lassoMode = "free",
  selected = NO_SELECTION,
  onSelectionChange,
  mono = false,
}: {
  draft: InkDraft;
  disabled?: boolean;
  tool: InkTool;
  width: number;
  mouseEnabled: boolean;
  onChange: (draft: InkDraft) => void;
  onActiveChange: (active: boolean) => void;
  onMetrics?: (metrics: InkMetrics) => void;
  onLimit: () => void;
  nativeInk?: boolean;
  onNativeStatus?: (status: OnyxInkStatus) => void;
  eraserMode?: EraserMode;
  eraserRadius?: number;
  lassoMode?: LassoMode;
  selected?: string[];
  onSelectionChange?: (ids: string[]) => void;
  /** Grayscale panel: strokes are mapped onto the gray axis instead of drawn in their color. */
  mono?: boolean;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bufferRef = useRef<HTMLCanvasElement | null>(null);
  const active = useRef<Gesture | null>(null);
  const sceneRef = useRef<{
    canvas: HTMLCanvasElement;
    strokes: InkStroke[];
    background: InkDraft["background"];
  } | null>(null);
  const damageRef = useRef<Bounds | null>(SHEET_BOUNDS);
  const publishedRef = useRef<{ strokes: InkStroke[]; overlay: Bounds | null } | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const menuRef = useRef<HTMLDivElement>(null);
  const [menuBox, setMenuBox] = useState({ width: 0, height: 0 });
  // A gesture owns the sheet: the floating menu follows a drag and steps out of a new lasso, and
  // the firmware region must not be reconfigured until the pen lifts.
  const [gestureAction, setGestureAction] = useState<Gesture["action"] | null>(null);
  const followFrame = useRef<number | null>(null);
  const followTarget = useRef<Bounds | null>(null);
  /**
   * Stock repositions its selection popup every ~10 ms. Going through React state would put the
   * menu a render behind the ink, which on this panel reads as the menu trailing the selection —
   * so the drag moves the element itself, in the same frame the preview is drawn, and React takes
   * the position back when the gesture commits.
   */
  const followSelection = (bounds: Bounds | null) => {
    followTarget.current = bounds;
    if (followFrame.current !== null) return;
    followFrame.current = requestAnimationFrame(() => {
      followFrame.current = null;
      const menu = menuRef.current;
      const target = followTarget.current;
      if (!menu || !target || !size.width) return;
      const at = selectionMenuPosition(target, size, menuBox);
      menu.style.left = `${at.left}px`;
      menu.style.top = `${at.top}px`;
    });
  };
  const endInteraction = () => {
    if (followFrame.current !== null) cancelAnimationFrame(followFrame.current);
    followFrame.current = null;
    followTarget.current = null;
    setGestureAction(null);
    // Hand the position back to the render, which now sees the committed selection.
    const menu = menuRef.current;
    if (menu) {
      menu.style.removeProperty("left");
      menu.style.removeProperty("top");
    }
  };

  const publish = (strokes: InkStroke[]) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    const buffer = (bufferRef.current ??= document.createElement("canvas"));
    if (buffer.width !== canvas.width) buffer.width = canvas.width;
    if (buffer.height !== canvas.height) buffer.height = canvas.height;
    const staging = buffer.getContext("2d");
    if (!ctx || !staging) return;
    staging.setTransform(canvas.width / 1000, 0, 0, canvas.height / 1400, 0, 0);
    const gesture = active.current;
    const sceneStrokes = gesture?.backdrop ?? strokes;
    const cached = sceneRef.current;
    let scene = cached?.canvas;
    if (
      !scene ||
      cached?.strokes !== sceneStrokes ||
      cached.background !== draft.background ||
      scene.width !== canvas.width ||
      scene.height !== canvas.height
    ) {
      scene ??= document.createElement("canvas");
      const reusable =
        cached &&
        cached.background === draft.background &&
        scene.width === canvas.width &&
        scene.height === canvas.height;
      if (!reusable) {
        damageRef.current = SHEET_BOUNDS;
        scene.width = canvas.width;
        scene.height = canvas.height;
      }
      const sceneContext = scene.getContext("2d");
      if (!sceneContext) return;
      sceneContext.setTransform(canvas.width / 1000, 0, 0, canvas.height / 1400, 0, 0);
      if (reusable) {
        const dirty = changedInkBounds(cached.strokes, sceneStrokes);
        if (dirty)
          drawInkRegion(
            sceneContext,
            sceneStrokes,
            draft.background ?? "plain",
            dirty,
            staging,
            mono,
          );
      } else drawSheet(sceneContext, sceneStrokes, draft.background, mono);
      sceneRef.current = { canvas: scene, strokes: sceneStrokes, background: draft.background };
    }
    staging.setTransform(1, 0, 0, 1, 0, 0);
    staging.globalCompositeOperation = "copy";
    staging.drawImage(scene, 0, 0);
    staging.globalCompositeOperation = "source-over";
    staging.setTransform(canvas.width / 1000, 0, 0, canvas.height / 1400, 0, 0);
    staging.setLineDash([]);
    // Frame and ink share the same staging bitmap, so e-ink cannot present them separately.
    const ids =
      gesture?.action === "move"
        ? gesture.ids
        : gesture?.action === "select" || gesture?.action === "erase"
          ? NO_SELECTION
          : selected;
    const bounds = selectionBounds(strokes, ids);
    if (gesture?.movingInk && bounds) {
      let layer = gesture.movingBitmap;
      if (!layer || layer.width !== canvas.width || layer.height !== canvas.height) {
        layer ??= document.createElement("canvas");
        layer.width = canvas.width;
        layer.height = canvas.height;
        const inkContext = layer.getContext("2d");
        if (!inkContext) return;
        inkContext.setTransform(canvas.width / 1000, 0, 0, canvas.height / 1400, 0, 0);
        for (const stroke of gesture.movingInk) {
          for (let i = 0; i < stroke.points.length; i++)
            drawSegment(
              inkContext,
              stroke.points[Math.max(0, i - 1)]!,
              stroke.points[i]!,
              stroke.width,
              mono,
            );
        }
        gesture.movingBitmap = layer;
      }
      const original = selectionBounds(gesture.movingInk, ids)!;
      staging.drawImage(layer, bounds.left - original.left, bounds.top - original.top, 1000, 1400);
    }
    // Pure black: an e-ink panel renders any grey as a dither pattern.
    staging.strokeStyle = "#000000";
    staging.lineWidth = SELECTION_FRAME_WIDTH;
    staging.setLineDash(SELECTION_DASH);
    if (bounds) {
      const frame = selectionFrame(bounds);
      staging.beginPath();
      staging.moveTo(frame.left, frame.top);
      staging.lineTo(frame.right, frame.top);
      staging.lineTo(frame.right, frame.bottom);
      staging.lineTo(frame.left, frame.bottom);
      staging.lineTo(frame.left, frame.top);
      staging.stroke();
    }
    staging.lineWidth = SELECTION_TRACE_WIDTH;
    if (gesture?.action === "select" || (gesture?.action === "erase" && eraserMode === "lasso")) {
      const polygon = lassoPolygon(
        gesture.points,
        gesture.action === "select" ? lassoMode : "free",
      );
      if (polygon.length > 1) {
        staging.beginPath();
        staging.moveTo(polygon[0]!.x, polygon[0]!.y);
        for (const p of polygon.slice(1)) staging.lineTo(p.x, p.y);
        staging.lineTo(polygon[0]!.x, polygon[0]!.y);
        staging.stroke();
      }
    }
    staging.setLineDash([]);
    staging.lineWidth = SELECTION_FRAME_WIDTH;
    if (gesture?.action === "erase" && eraserMode !== "lasso" && gesture.points.length) {
      const p = gesture.points[gesture.points.length - 1]!;
      staging.beginPath();
      staging.arc(p.x, p.y, eraserRadius, 0, Math.PI * 2);
      staging.stroke();
    }
    let overlay: Bounds | null = bounds
      ? selectionFrame(bounds, SELECTION_FRAME_PADDING + SELECTION_FRAME_WIDTH)
      : null;
    // The menu is a DOM overlay the panel repaints with the sheet, so a menu that moved with the
    // selection has to be inside the damage or its old position ghosts.
    if (bounds && !disabled && size.width && size.height && menuBox.height) {
      const at = selectionMenuPosition(bounds, size, menuBox);
      const x = 1000 / size.width,
        y = 1400 / size.height;
      overlay = unionBounds(overlay, {
        left: at.left * x,
        top: at.top * y,
        right: (at.left + menuBox.width) * x,
        bottom: (at.top + menuBox.height) * y,
      });
    }
    if (gesture?.action === "select" || gesture?.action === "erase") {
      const padding = gesture.action === "erase" ? eraserRadius + 2 : 2;
      for (const p of gesture.points)
        overlay = unionBounds(overlay, {
          left: p.x - padding,
          top: p.y - padding,
          right: p.x + padding,
          bottom: p.y + padding,
        });
    }
    const previous = publishedRef.current;
    const inkDamage = previous ? changedInkBounds(previous.strokes, strokes) : SHEET_BOUNDS;
    damageRef.current = unionBounds(
      damageRef.current,
      unionBounds(inkDamage, unionBounds(previous?.overlay ?? null, overlay)),
    );
    publishedRef.current = { strokes, overlay };
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalCompositeOperation = "copy";
    ctx.drawImage(buffer, 0, 0);
    ctx.globalCompositeOperation = "source-over";
    ctx.setTransform(canvas.width / 1000, 0, 0, canvas.height / 1400, 0, 0);
  };
  const beginGesture = (
    point: InkPoint,
    erasing: boolean,
    id: number,
    base: InkDraft,
    nativeFast = false,
    forced?: Gesture["action"],
  ) => {
    const hit = withinSelection(selectionBounds(base.strokes, selected), point);
    const action =
      forced ?? (erasing ? "erase" : tool === "lasso" ? (hit ? "move" : "select") : "pen");
    const count = base.strokes.reduce((sum, s) => sum + s.points.length, 0);
    if (action === "pen" && count >= MAX_INK_POINTS) {
      onLimit();
      return;
    }
    active.current = {
      id,
      action,
      points: [],
      base: base.strokes,
      preview: base.strokes,
      ids: selected,
      count,
      lastMetrics: -Infinity,
      nativeFast,
      backdrop:
        action === "move" ? base.strokes.filter((s) => !selected.includes(s.id)) : undefined,
      movingInk:
        action === "move" ? base.strokes.filter((s) => selected.includes(s.id)) : undefined,
    };
    setGestureAction(action);
    sampleGesture(point);
    return active.current;
  };
  const sampleGesture = (point: InkPoint, end = false) => {
    const current = active.current;
    if (!current) return;
    const last = current.points[current.points.length - 1];
    if (end && last) point = { ...point, pressure: last.pressure };
    if (last && last.x === point.x && last.y === point.y && last.pressure === point.pressure)
      return;
    if (current.points.length >= MAX_INK_POINTS) return;
    if (current.action === "pen" && current.count + current.points.length >= MAX_INK_POINTS) return;
    current.points.push(point);
    if (current.nativeFast) return;
    if (current.action === "pen") {
      const ctx = canvasRef.current?.getContext("2d");
      if (ctx) drawSegment(ctx, last ?? point, point, width, mono);
    } else if (current.action === "move") {
      const start = current.points[0]!;
      current.preview = moveSelection(
        current.base,
        current.ids,
        point.x - start.x,
        point.y - start.y,
      );
      followSelection(selectionBounds(current.preview, current.ids));
      publish(current.preview);
    } else if (current.action === "erase" && eraserMode !== "lasso") {
      current.preview = eraseGesture(
        current.preview,
        last ? [last, point] : [point],
        eraserMode,
        eraserRadius,
      );
      publish(current.preview);
    } else publish(current.preview);
  };
  const finishGesture = (base: InkDraft, points?: InkPoint[]): InkDraft => {
    const current = active.current;
    if (!current) return base;
    const gesture = points ?? current.points;
    let strokes = current.base;
    if (current.action === "select")
      onSelectionChange?.(selectLasso(strokes, lassoPolygon(gesture, lassoMode)));
    else if (current.action === "move" && gesture.length) {
      const first = current.points[0] ?? gesture[0]!,
        last = gesture[gesture.length - 1]!;
      strokes = moveSelection(strokes, current.ids, last.x - first.x, last.y - first.y);
    } else if (current.action === "erase") {
      strokes = eraseGesture(strokes, gesture, eraserMode, eraserRadius);
      onSelectionChange?.([]);
    } else if (current.action === "pen" && gesture.length)
      strokes = [...strokes, { id: crypto.randomUUID(), width, points: gesture }];
    active.current = null;
    endInteraction();
    if (strokes.reduce((count, s) => count + s.points.length, 0) > MAX_INK_POINTS) {
      onLimit();
      publish(base.strokes);
      return base;
    }
    publish(strokes);
    return strokes === base.strokes ? base : { ...base, strokes };
  };
  const nativeInput = (event: OnyxInkEvent, base: InkDraft) => {
    const point = event.points?.[0];
    if (event.kind === "begin" && point && (tool !== "pen" || event.erasing))
      beginGesture(point, event.erasing, -1, base, event.fastPreview);
    else if (event.kind === "preview" && point) sampleGesture(point);
    else if (event.kind === "cancel" || event.kind === "end") {
      if (active.current) {
        if (event.kind === "cancel") onSelectionChange?.(active.current.ids);
        active.current = null;
        endInteraction();
        publish(base.strokes);
      }
    }
  };
  const nativeStroke = (base: InkDraft, event: OnyxInkEvent) => {
    if (tool === "pen" && !event.erasing) return applyOnyxStroke(base, event);
    if (!event.points?.length) return base;
    if (!active.current) beginGesture(event.points[0]!, event.erasing, -1, base);
    return finishGesture(base, event.points);
  };
  const selection = selectionBounds(draft.strokes, selected);
  // The menu is a DOM overlay over the firmware's drawing region, anchored to the selection the way
  // stock Notes anchors its selection popup: it follows a drag frame by frame and steps aside while
  // a new lasso is being drawn.
  const menuBounds = gestureAction === "select" ? null : selection;
  const menuAt =
    menuBounds && !disabled && size.width ? selectionMenuPosition(menuBounds, size, menuBox) : null;
  const onyx = useOnyxInk({
    canvasRef,
    enabled: nativeInk && !disabled,
    draft,
    tool,
    width,
    onChange,
    onActiveChange,
    onMetrics,
    onLimit,
    onStatus: onNativeStatus,
    decoration: selected.join(","),
    lassoMode,
    selection,
    overlayRects: menuAt ? [{ ...menuAt, ...menuBox }] : undefined,
    interacting: gestureAction !== null,
    onInput: nativeInput,
    onStroke: nativeStroke,
    damage: {
      take: () => {
        const bounds = damageRef.current;
        damageRef.current = null;
        return bounds;
      },
      invalidate: () => {
        damageRef.current = SHEET_BOUNDS;
      },
    },
  });

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setSize({ width: entry.contentRect.width, height: entry.contentRect.height });
    });
    observer.observe(canvas);
    return () => {
      observer.disconnect();
      if (followFrame.current !== null) cancelAnimationFrame(followFrame.current);
    };
  }, []);
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !size.width) return;
    const ratio = Math.min(window.devicePixelRatio || 1, 3);
    const pixelWidth = Math.round(size.width * ratio),
      pixelHeight = Math.round(size.height * ratio);
    if (canvas.width !== pixelWidth) canvas.width = pixelWidth;
    if (canvas.height !== pixelHeight) canvas.height = pixelHeight;
    const current = active.current;
    publish(
      current
        ? current.action === "pen"
          ? [...current.base, { id: "preview", width, points: current.points }]
          : current.preview
        : draft.strokes,
    );
    // Redraw on document/size changes only; pointer previews publish directly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft, size, selected, tool]);
  // The menu is laid out by its own content, so its box is known only once it is on the page.
  useLayoutEffect(() => {
    const box = menuRef.current?.getBoundingClientRect();
    const width = Math.round(box?.width ?? 0),
      height = Math.round(box?.height ?? 0);
    if (width !== menuBox.width || height !== menuBox.height) setMenuBox({ width, height });
  });

  const runSelectionAction = (action: SelectionAction) => {
    if (action === "deselect") return onSelectionChange?.([]);
    if (action === "delete") {
      onChange({ ...draft, strokes: draft.strokes.filter((s) => !selected.includes(s.id)) });
      return onSelectionChange?.([]);
    }
    if (action === "copy") {
      const source = draft.strokes.filter((s) => selected.includes(s.id));
      const points = [...draft.strokes, ...source].reduce((n, s) => n + s.points.length, 0);
      if (points > MAX_INK_POINTS) return onLimit();
      const copies = source.map((s) => ({ ...s, id: crypto.randomUUID() }));
      const ids = copies.map((s) => s.id);
      onChange({ ...draft, strokes: [...draft.strokes, ...moveSelection(copies, ids, 25, 25)] });
      return onSelectionChange?.(ids);
    }
    const strokes = scaleSelection(draft.strokes, selected, action === "shrink" ? 0.9 : 1.1);
    if (strokes !== draft.strokes) onChange({ ...draft, strokes });
  };

  const sample = (event: PointerEvent, end = false) => {
    const current = active.current;
    if (!current?.rect) return;
    const p = inkPoint(event, current.rect);
    sampleGesture(p, end);
    if (event.timeStamp - current.lastMetrics > 120 || end) {
      current.lastMetrics = event.timeStamp;
      onMetrics?.({
        tool: event.pointerType,
        pressure: p.pressure,
        tiltX: p.tiltX,
        tiltY: p.tiltY,
        samples: current.points.length,
      });
    }
  };
  /** Abandon a gesture no pointer-up will ever complete, leaving the handwriting untouched. */
  const cancelGesture = () => {
    const current = active.current;
    if (!current) return;
    active.current = null;
    endInteraction();
    onSelectionChange?.(current.ids);
    publish(draft.strokes);
    onActiveChange(false);
    const canvas = canvasRef.current;
    if (canvas?.hasPointerCapture(current.id)) canvas.releasePointerCapture(current.id);
  };
  /**
   * A finger reaches the WebView even while the firmware owns the pen, and it may only drag a
   * selection that is already there. Every other touch stays a palm the sheet ignores.
   */
  const startFinger = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    if (active.current) {
      // Two fingers are a pan or a pinch, never a move.
      if (active.current.finger) cancelGesture();
      return;
    }
    if (!selected.length) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const point = inkPoint(event.nativeEvent, rect);
    if (!withinSelection(selectionBounds(draft.strokes, selected), point)) return;
    event.preventDefault();
    const started = beginGesture(point, false, event.pointerId, draft, false, "move");
    if (!started) return;
    started.rect = rect;
    started.finger = true;
    event.currentTarget.setPointerCapture(event.pointerId);
    onActiveChange(true);
  };
  const start = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    if (disabled) return;
    if (event.pointerType === "touch") return startFinger(event);
    if (
      active.current ||
      (onyx && event.pointerType === "pen") ||
      (event.pointerType !== "pen" && !(mouseEnabled && event.pointerType === "mouse"))
    )
      return;
    if (event.pointerType === "mouse" && event.button !== 0) return;
    event.preventDefault();
    const rect = event.currentTarget.getBoundingClientRect();
    const started = beginGesture(
      inkPoint(event.nativeEvent, rect),
      tool === "eraser" || event.button === 5 || (event.buttons & 32) !== 0,
      event.pointerId,
      draft,
    );
    if (!started) return;
    started.rect = rect;
    event.currentTarget.setPointerCapture(event.pointerId);
    onActiveChange(true);
  };
  const move = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    if (event.pointerId !== active.current?.id) return;
    event.preventDefault();
    const coalesced = event.nativeEvent.getCoalescedEvents?.() ?? [];
    for (const point of coalesced.length ? coalesced : [event.nativeEvent]) sample(point);
  };
  const finish = (event: ReactPointerEvent<HTMLCanvasElement>, cancelled: boolean) => {
    const current = active.current;
    if (!current || current.id !== event.pointerId) return;
    if (cancelled) {
      active.current = null;
      endInteraction();
      onSelectionChange?.(current.ids);
      publish(draft.strokes);
    } else {
      sample(event.nativeEvent, true);
      const next = finishGesture(draft);
      if (next !== draft) onChange(next);
    }
    onActiveChange(false);
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId);
  };
  return (
    <div className="relative">
      <canvas
        ref={canvasRef}
        aria-label="Handwriting sheet"
        role="img"
        className="block aspect-[5/7] w-full touch-none select-none bg-white"
        style={{ cursor: tool === "eraser" ? "cell" : "crosshair" }}
        onContextMenu={(event) => event.preventDefault()}
        onPointerDown={start}
        onPointerMove={move}
        onPointerUp={(event) => finish(event, false)}
        onPointerCancel={(event) => finish(event, true)}
        onLostPointerCapture={(event) => finish(event, true)}
      />
      {menuAt && (
        <div
          ref={menuRef}
          data-ink-selection-menu
          role="toolbar"
          aria-label="Selection actions"
          // Flat black on white, and inverted while held: an e-ink panel has no other contrast.
          className="absolute z-10 flex touch-none items-center gap-px border border-black bg-white p-px select-none"
          style={{ left: menuAt.left, top: menuAt.top }}
        >
          {SELECTION_ACTIONS.map(({ id, label, Icon }) => (
            <button
              key={id}
              type="button"
              aria-label={label}
              className="flex size-9 items-center justify-center bg-white text-black active:bg-black active:text-white"
              onClick={() => runSelectionAction(id)}
            >
              <Icon className="size-4" />
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

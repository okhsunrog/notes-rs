import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import type { InkDraft, InkPoint, InkStroke } from "@/lib/bindings";
import { drawSegment, drawSheet, inkPoint, MAX_INK_POINTS } from "./ink-model";
import {
  eraseGesture,
  lassoPolygon,
  moveSelection,
  selectionBounds,
  selectLasso,
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
  backdrop?: InkStroke[];
  movingInk?: InkStroke[];
  movingBitmap?: HTMLCanvasElement;
};
const NO_SELECTION: string[] = [];

export function InkCanvas({
  draft,
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
}: {
  draft: InkDraft;
  tool: InkTool;
  width: number;
  mouseEnabled: boolean;
  onChange: (draft: InkDraft) => void;
  onActiveChange: (active: boolean) => void;
  onMetrics: (metrics: InkMetrics) => void;
  onLimit: () => void;
  nativeInk?: boolean;
  onNativeStatus?: (status: OnyxInkStatus) => void;
  eraserMode?: EraserMode;
  eraserRadius?: number;
  lassoMode?: LassoMode;
  selected?: string[];
  onSelectionChange?: (ids: string[]) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bufferRef = useRef<HTMLCanvasElement | null>(null);
  const active = useRef<Gesture | null>(null);
  const sceneRef = useRef<{
    canvas: HTMLCanvasElement;
    strokes: InkStroke[];
    background: InkDraft["background"];
  } | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });

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
      scene.width = canvas.width;
      scene.height = canvas.height;
      const sceneContext = scene.getContext("2d");
      if (!sceneContext) return;
      sceneContext.setTransform(canvas.width / 1000, 0, 0, canvas.height / 1400, 0, 0);
      drawSheet(sceneContext, sceneStrokes, draft.background);
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
            );
        }
        gesture.movingBitmap = layer;
      }
      const original = selectionBounds(gesture.movingInk, ids)!;
      staging.drawImage(layer, bounds.left - original.left, bounds.top - original.top, 1000, 1400);
    }
    staging.strokeStyle = "#111111";
    staging.lineWidth = 1.5;
    staging.setLineDash([7, 5]);
    if (bounds) {
      staging.beginPath();
      staging.moveTo(bounds.left - 6, bounds.top - 6);
      staging.lineTo(bounds.right + 6, bounds.top - 6);
      staging.lineTo(bounds.right + 6, bounds.bottom + 6);
      staging.lineTo(bounds.left - 6, bounds.bottom + 6);
      staging.lineTo(bounds.left - 6, bounds.top - 6);
      staging.stroke();
    }
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
    if (gesture?.action === "erase" && eraserMode !== "lasso" && gesture.points.length) {
      const p = gesture.points[gesture.points.length - 1]!;
      staging.beginPath();
      staging.arc(p.x, p.y, eraserRadius, 0, Math.PI * 2);
      staging.stroke();
    }
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
  ) => {
    const bounds = selectionBounds(base.strokes, selected);
    const hit =
      bounds &&
      point.x >= bounds.left - 12 &&
      point.x <= bounds.right + 12 &&
      point.y >= bounds.top - 12 &&
      point.y <= bounds.bottom + 12;
    const action = erasing ? "erase" : tool === "lasso" ? (hit ? "move" : "select") : "pen";
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
      if (ctx) drawSegment(ctx, last ?? point, point, width);
    } else if (current.action === "move") {
      const start = current.points[0]!;
      current.preview = moveSelection(
        current.base,
        current.ids,
        point.x - start.x,
        point.y - start.y,
      );
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
    if (strokes.reduce((count, s) => count + s.points.length, 0) > MAX_INK_POINTS) {
      onLimit();
      publish(base.strokes);
      return base;
    }
    publish(strokes);
    return strokes === base.strokes ? base : { ...base, strokes };
  };
  const nativeInput = (event: OnyxInkEvent) => {
    const point = event.points?.[0];
    if (event.kind === "begin" && point && (tool !== "pen" || event.erasing))
      beginGesture(point, event.erasing, -1, draft, event.fastPreview);
    else if (event.kind === "preview" && point) sampleGesture(point);
    else if (event.kind === "cancel" || event.kind === "end") {
      if (active.current) {
        if (event.kind === "cancel") onSelectionChange?.(active.current.ids);
        active.current = null;
        publish(draft.strokes);
      }
    }
  };
  const nativeStroke = (base: InkDraft, event: OnyxInkEvent) => {
    if (tool === "pen" && !event.erasing) return applyOnyxStroke(base, event);
    if (!event.points?.length) return base;
    if (!active.current) beginGesture(event.points[0]!, event.erasing, -1, base);
    return finishGesture(base, event.points);
  };
  const onyx = useOnyxInk({
    canvasRef,
    enabled: nativeInk,
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
    selection: selectionBounds(draft.strokes, selected),
    onInput: nativeInput,
    onStroke: nativeStroke,
  });

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setSize({ width: entry.contentRect.width, height: entry.contentRect.height });
    });
    observer.observe(canvas);
    return () => observer.disconnect();
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

  const sample = (event: PointerEvent, end = false) => {
    const current = active.current;
    if (!current?.rect) return;
    const p = inkPoint(event, current.rect);
    sampleGesture(p, end);
    if (event.timeStamp - current.lastMetrics > 120 || end) {
      current.lastMetrics = event.timeStamp;
      onMetrics({
        tool: event.pointerType,
        pressure: p.pressure,
        tiltX: p.tiltX,
        tiltY: p.tiltY,
        samples: current.points.length,
      });
    }
  };
  const start = (event: ReactPointerEvent<HTMLCanvasElement>) => {
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
    </div>
  );
}

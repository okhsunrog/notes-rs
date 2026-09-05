import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import type { InkDraft, InkStroke } from "@/lib/bindings";
import { drawSegment, drawSheet, eraseAt, inkPoint, MAX_INK_POINTS } from "./ink-model";
import { useOnyxInk, type OnyxInkStatus } from "./onyx-ink";

export type InkMetrics = {
  tool: string;
  pressure: number;
  tiltX: number;
  tiltY: number;
  samples: number;
};
export type InkTool = "pen" | "eraser";

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
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bufferRef = useRef<HTMLCanvasElement | null>(null);
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
  });
  const active = useRef<{
    id: number;
    stroke: InkStroke;
    erasing: boolean;
    strokes: InkStroke[];
    count: number;
    rect: DOMRect;
    lastMetrics: number;
  } | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });

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
    const pixelWidth = Math.round(size.width * ratio);
    const pixelHeight = Math.round(size.height * ratio);
    // Assigning even the same dimensions clears the visible backing store.
    if (canvas.width !== pixelWidth) canvas.width = pixelWidth;
    if (canvas.height !== pixelHeight) canvas.height = pixelHeight;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const buffer = (bufferRef.current ??= document.createElement("canvas"));
    if (buffer.width !== pixelWidth) buffer.width = pixelWidth;
    if (buffer.height !== pixelHeight) buffer.height = pixelHeight;
    const staging = buffer.getContext("2d");
    if (!staging) return;
    staging.setTransform(pixelWidth / 1000, 0, 0, pixelHeight / 1400, 0, 0);
    const current = active.current;
    drawSheet(
      staging,
      current
        ? current.erasing
          ? current.strokes
          : [...current.strokes, current.stroke]
        : draft.strokes,
      draft.background,
    );
    // Publish a complete image in one operation, including erasure/undo, with no blank frame.
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalCompositeOperation = "copy";
    ctx.drawImage(buffer, 0, 0);
    ctx.globalCompositeOperation = "source-over";
    ctx.setTransform(pixelWidth / 1000, 0, 0, pixelHeight / 1400, 0, 0);
  }, [draft, size]);

  const sample = (event: PointerEvent, end = false) => {
    const current = active.current;
    const ctx = canvasRef.current?.getContext("2d");
    if (!current || !ctx) return;
    const p = inkPoint(event, current.rect);
    if (current.erasing) {
      const next = eraseAt(current.strokes, p);
      if (next.length !== current.strokes.length) {
        current.strokes = next;
        drawSheet(ctx, next, draft.background);
      }
    } else {
      if (current.count + current.stroke.points.length >= MAX_INK_POINTS) return;
      const last = current.stroke.points[current.stroke.points.length - 1];
      // Pointerup usually reports pressure zero. Preserve the final contact width.
      if (end && last) p.pressure = last.pressure;
      if (last && p.x === last.x && p.y === last.y && p.pressure === last.pressure) return;
      current.stroke.points.push(p);
      drawSegment(ctx, last ?? p, p, current.stroke.width);
    }
    if (event.timeStamp - current.lastMetrics > 120 || end) {
      current.lastMetrics = event.timeStamp;
      onMetrics({
        tool: event.pointerType,
        pressure: p.pressure,
        tiltX: p.tiltX,
        tiltY: p.tiltY,
        samples: current.stroke.points.length,
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
    const count = draft.strokes.reduce((sum, stroke) => sum + stroke.points.length, 0);
    const erasing = tool === "eraser" || event.button === 5 || (event.buttons & 32) !== 0;
    if (!erasing && count >= MAX_INK_POINTS) {
      onLimit();
      return;
    }
    active.current = {
      id: event.pointerId,
      erasing,
      count,
      strokes: draft.strokes,
      stroke: { id: crypto.randomUUID(), width, points: [] },
      rect: event.currentTarget.getBoundingClientRect(),
      lastMetrics: -Infinity,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
    onActiveChange(true);
    sample(event.nativeEvent);
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
    if (!cancelled) sample(event.nativeEvent, true);
    active.current = null;
    onActiveChange(false);
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId);
    if (cancelled) {
      const ctx = event.currentTarget.getContext("2d");
      if (ctx) drawSheet(ctx, draft.strokes, draft.background);
    } else {
      const strokes = current.erasing
        ? current.strokes
        : current.stroke.points.length
          ? [...current.strokes, current.stroke]
          : current.strokes;
      if (strokes !== draft.strokes) onChange({ ...draft, strokes });
      if (!current.erasing && current.count + current.stroke.points.length >= MAX_INK_POINTS)
        onLimit();
    }
  };

  return (
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
  );
}

import type { InkBackground, InkStroke } from "@/lib/bindings";
import type { Bounds } from "./ink-editing";
import { drawSheet } from "./ink-model";

const boundsCache = new WeakMap<InkStroke, Bounds | null>();
export const SHEET_BOUNDS: Bounds = { left: 0, top: 0, right: 1000, bottom: 1400 };

export function unionBounds(a: Bounds | null, b: Bounds | null): Bounds | null {
  if (!a) return b;
  if (!b) return a;
  return {
    left: Math.min(a.left, b.left),
    top: Math.min(a.top, b.top),
    right: Math.max(a.right, b.right),
    bottom: Math.max(a.bottom, b.bottom),
  };
}

/** Stroke objects are immutable; include maximum pressure width and antialiasing. */
export function inkBounds(stroke: InkStroke): Bounds | null {
  if (boundsCache.has(stroke)) return boundsCache.get(stroke)!;
  let bounds: Bounds | null = null;
  for (const p of stroke.points)
    bounds = unionBounds(bounds, { left: p.x, top: p.y, right: p.x, bottom: p.y });
  if (bounds) {
    const padding = (stroke.width * 1.75) / 2 + 2;
    bounds = {
      left: bounds.left - padding,
      top: bounds.top - padding,
      right: bounds.right + padding,
      bottom: bounds.bottom + padding,
    };
  }
  boundsCache.set(stroke, bounds);
  return bounds;
}

export function changedInkBounds(before: InkStroke[], after: InkStroke[]): Bounds | null {
  if (before === after) return null;
  const oldSet = new Set(before),
    newSet = new Set(after);
  // Reordering retained strokes can change their compositing even without point edits.
  const retained = before.filter((s) => newSet.has(s));
  const nextRetained = after.filter((s) => oldSet.has(s));
  if (retained.some((s, i) => s !== nextRetained[i])) return SHEET_BOUNDS;
  let bounds: Bounds | null = null;
  for (const stroke of before)
    if (!newSet.has(stroke)) bounds = unionBounds(bounds, inkBounds(stroke));
  for (const stroke of after)
    if (!oldSet.has(stroke)) bounds = unionBounds(bounds, inkBounds(stroke));
  return bounds;
}

/** Restore background and overlapping strokes in canonical order, without bitmap resampling. */
export function drawInkRegion(
  ctx: CanvasRenderingContext2D,
  strokes: InkStroke[],
  background: InkBackground,
  bounds: Bounds,
  scratch: CanvasRenderingContext2D,
  mono?: boolean,
) {
  const { a: sx, d: sy } = ctx.getTransform();
  // Pixel-aligned clipping prevents seams from repeatedly blending a fractional clip edge.
  const region = {
    left: Math.floor(bounds.left * sx) / sx,
    top: Math.floor(bounds.top * sy) / sy,
    right: Math.ceil(bounds.right * sx) / sx,
    bottom: Math.ceil(bounds.bottom * sy) / sy,
  };
  const visible = strokes.filter((stroke) => {
    const b = inkBounds(stroke);
    return (
      b &&
      b.left <= region.right &&
      b.right >= region.left &&
      b.top <= region.bottom &&
      b.bottom >= region.top
    );
  });
  // A clip can change Skia's edge rasterization for crossing strokes. Render uncut
  // vectors into the existing staging canvas, then copy only whole damaged pixels.
  scratch.setTransform(sx, 0, 0, sy, 0, 0);
  drawSheet(scratch, visible, background, mono);
  const x = Math.max(0, Math.floor(bounds.left * sx));
  const y = Math.max(0, Math.floor(bounds.top * sy));
  const right = Math.min(ctx.canvas.width, Math.ceil(bounds.right * sx));
  const bottom = Math.min(ctx.canvas.height, Math.ceil(bounds.bottom * sy));
  if (right <= x || bottom <= y) return;
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.globalCompositeOperation = "source-over";
  ctx.drawImage(scratch.canvas, x, y, right - x, bottom - y, x, y, right - x, bottom - y);
  ctx.restore();
}

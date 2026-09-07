import type { InkPoint, InkStroke } from "@/lib/bindings";

export type EraserMode = "stroke" | "pixel" | "lasso";
export type LassoMode = "free" | "rectangle";
export type XY = { x: number; y: number };
export type Bounds = { left: number; top: number; right: number; bottom: number };
const EPS = 1e-8;
const strokeBoundsCache = new WeakMap<InkStroke, Bounds | null>();
export function strokeBounds(stroke: InkStroke): Bounds | null {
  if (!strokeBoundsCache.has(stroke)) strokeBoundsCache.set(stroke, boundsOf(stroke.points));
  return strokeBoundsCache.get(stroke)!;
}
function overlaps(a: Bounds | null, b: Bounds, margin = 0): boolean {
  return (
    !!a &&
    a.left <= b.right + margin &&
    a.right >= b.left - margin &&
    a.top <= b.bottom + margin &&
    a.bottom >= b.top - margin
  );
}
const mix = (a: number, b: number, t: number) => a + (b - a) * t;
function interpolate(a: InkPoint, b: InkPoint, t: number): InkPoint {
  return {
    x: mix(a.x, b.x, t),
    y: mix(a.y, b.y, t),
    pressure: mix(a.pressure, b.pressure, t),
    tiltX: mix(a.tiltX, b.tiltX, t),
    tiltY: mix(a.tiltY, b.tiltY, t),
    time: mix(a.time, b.time, t),
  };
}
function distance(p: XY, a: XY, b: XY) {
  const dx = b.x - a.x,
    dy = b.y - a.y;
  const t = Math.max(
    0,
    Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy || 1)),
  );
  return Math.hypot(p.x - mix(a.x, b.x, t), p.y - mix(a.y, b.y, t));
}

/** Simplify only the temporary gesture, never the stored handwriting. */
export function simplifyGesture(points: XY[], tolerance = 0.5): XY[] {
  if (points.length < 3) return points;
  const keep = new Set([0, points.length - 1]);
  const stack = [[0, points.length - 1]];
  while (stack.length) {
    const [start, end] = stack.pop()!;
    let farthest = tolerance,
      index = -1;
    for (let i = start! + 1; i < end!; i++) {
      const d = distance(points[i]!, points[start!]!, points[end!]!);
      if (d > farthest) {
        farthest = d;
        index = i;
      }
    }
    if (index >= 0) {
      keep.add(index);
      stack.push([start!, index], [index, end!]);
    }
  }
  return [...keep].sort((a, b) => a - b).map((i) => points[i]!);
}
export function lassoPolygon(path: XY[], mode: LassoMode): XY[] {
  if (mode === "free") return simplifyGesture(path);
  const a = path[0],
    b = path[path.length - 1];
  return a && b ? [a, { x: b.x, y: a.y }, b, { x: a.x, y: b.y }] : [];
}
export function inside(p: XY, polygon: XY[]) {
  let found = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i++) {
    const a = polygon[i]!,
      b = polygon[j]!;
    if (distance(p, a, b) < EPS) return true;
    if (a.y > p.y !== b.y > p.y && p.x < ((b.x - a.x) * (p.y - a.y)) / (b.y - a.y) + a.x)
      found = !found;
  }
  return found;
}
function crosses(a: XY, b: XY, c: XY, d: XY) {
  const cross = (p: XY, q: XY, r: XY) => (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
  const x = cross(a, b, c),
    y = cross(a, b, d),
    z = cross(c, d, a),
    w = cross(c, d, b);
  if (x * y < 0 && z * w < 0) return true;
  return (
    (Math.abs(x) < EPS && distance(c, a, b) < EPS) ||
    (Math.abs(y) < EPS && distance(d, a, b) < EPS) ||
    (Math.abs(z) < EPS && distance(a, c, d) < EPS) ||
    (Math.abs(w) < EPS && distance(b, c, d) < EPS)
  );
}
export function selectLasso(strokes: InkStroke[], polygon: XY[]): string[] {
  if (polygon.length < 3) return [];
  const bounds = boundsOf(polygon)!;
  if (bounds.right - bounds.left < 2 || bounds.bottom - bounds.top < 2) return [];
  return strokes
    .filter(
      (stroke) =>
        overlaps(strokeBounds(stroke), bounds) &&
        stroke.points.some(
          (p, i) =>
            inside(p, polygon) ||
            (i > 0 &&
              polygon.some((q, j) =>
                crosses(stroke.points[i - 1]!, p, q, polygon[(j + 1) % polygon.length]!),
              )),
        ),
    )
    .map((s) => s.id);
}
export function boundsOf(points: XY[]): Bounds | null {
  if (!points.length) return null;
  let left = Infinity,
    top = Infinity,
    right = -Infinity,
    bottom = -Infinity;
  for (const p of points) {
    left = Math.min(left, p.x);
    top = Math.min(top, p.y);
    right = Math.max(right, p.x);
    bottom = Math.max(bottom, p.y);
  }
  return { left, top, right, bottom };
}
export function selectionBounds(strokes: InkStroke[], ids: string[]) {
  const selected = new Set(ids);
  return boundsOf(strokes.filter((s) => selected.has(s.id)).flatMap((s) => s.points));
}
/**
 * Anchor a floating toolbar over the sheet: just above the selection, below it when the top of
 * the sheet has no room, and never past an edge. Sizes are CSS pixels of the rendered sheet.
 */
export function selectionMenuPosition(
  bounds: Bounds,
  sheet: { width: number; height: number },
  menu: { width: number; height: number },
  gap = 8,
): { left: number; top: number } {
  const clamp = (value: number, room: number) =>
    Math.max(gap, Math.min(value, Math.max(gap, room - gap)));
  const above = (bounds.top / 1400) * sheet.height - gap - menu.height;
  return {
    left: clamp(
      ((bounds.left + bounds.right) / 2000) * sheet.width - menu.width / 2,
      sheet.width - menu.width,
    ),
    top:
      above >= gap
        ? above
        : clamp((bounds.bottom / 1400) * sheet.height + gap, sheet.height - menu.height),
  };
}
export function moveSelection(
  strokes: InkStroke[],
  ids: string[],
  dx: number,
  dy: number,
): InkStroke[] {
  const bounds = selectionBounds(strokes, ids);
  if (!bounds) return strokes;
  dx = Math.max(-bounds.left, Math.min(1000 - bounds.right, dx));
  dy = Math.max(-bounds.top, Math.min(1400 - bounds.bottom, dy));
  if (Math.abs(dx) < EPS && Math.abs(dy) < EPS) return strokes;
  const selected = new Set(ids);
  return strokes.map((s) =>
    selected.has(s.id)
      ? { ...s, points: s.points.map((p) => ({ ...p, x: p.x + dx, y: p.y + dy })) }
      : s,
  );
}
export function scaleSelection(strokes: InkStroke[], ids: string[], factor: number): InkStroke[] {
  const bounds = selectionBounds(strokes, ids);
  if (!bounds) return strokes;
  const cx = (bounds.left + bounds.right) / 2,
    cy = (bounds.top + bounds.bottom) / 2;
  const halfW = (bounds.right - bounds.left) / 2,
    halfH = (bounds.bottom - bounds.top) / 2;
  const selected = new Set(ids);
  for (const s of strokes)
    if (selected.has(s.id)) factor = Math.max(0.5 / s.width, Math.min(20 / s.width, factor));
  factor = Math.min(
    factor,
    halfW ? Math.min(cx, 1000 - cx) / halfW : Infinity,
    halfH ? Math.min(cy, 1400 - cy) / halfH : Infinity,
  );
  if (Math.abs(factor - 1) < EPS) return strokes;
  return strokes.map((s) =>
    selected.has(s.id)
      ? {
          ...s,
          width: Math.max(0.5, Math.min(20, s.width * factor)),
          points: s.points.map((p) => ({
            ...p,
            x: Math.max(0, Math.min(1000, cx + (p.x - cx) * factor)),
            y: Math.max(0, Math.min(1400, cy + (p.y - cy) * factor)),
          })),
        }
      : s,
  );
}

type Interval = [number, number];
function circleInterval(a: XY, b: XY, center: XY, radius: number): Interval | null {
  const dx = b.x - a.x,
    dy = b.y - a.y,
    x = a.x - center.x,
    y = a.y - center.y;
  const aa = dx * dx + dy * dy,
    bb = 2 * (x * dx + y * dy),
    cc = x * x + y * y - radius * radius;
  if (aa < EPS) return cc <= 0 ? [0, 1] : null;
  const discriminant = bb * bb - 4 * aa * cc;
  if (discriminant <= 0) return null;
  const lo = Math.max(0, (-bb - Math.sqrt(discriminant)) / (2 * aa));
  const hi = Math.min(1, (-bb + Math.sqrt(discriminant)) / (2 * aa));
  return hi > lo ? [lo, hi] : null;
}
/** Intersect a line with the swept round eraser, including between sparse pointer samples. */
function cutIntervals(a: XY, b: XY, path: XY[], radius: number): Interval[] {
  const intervals: Interval[] = [];
  const left = Math.min(a.x, b.x),
    right = Math.max(a.x, b.x);
  const top = Math.min(a.y, b.y),
    bottom = Math.max(a.y, b.y);
  for (let i = 0; i < path.length; i++) {
    const c = path[i]!,
      previous = path[Math.max(0, i - 1)]!;
    // Exact broad-phase rejection of the swept capsule, including sparse samples.
    if (
      left > Math.max(c.x, previous.x) + radius ||
      right < Math.min(c.x, previous.x) - radius ||
      top > Math.max(c.y, previous.y) + radius ||
      bottom < Math.min(c.y, previous.y) - radius
    )
      continue;
    const circle = circleInterval(a, b, c, radius);
    if (circle) intervals.push(circle);
    if (!i) continue;
    const d = path[i - 1]!,
      dx = d.x - c.x,
      dy = d.y - c.y,
      length = Math.hypot(dx, dy);
    if (length < EPS) continue;
    const ux = dx / length,
      uy = dy / length;
    const ax = (a.x - c.x) * ux + (a.y - c.y) * uy,
      ay = -(a.x - c.x) * uy + (a.y - c.y) * ux;
    const bx = (b.x - c.x) * ux + (b.y - c.y) * uy,
      by = -(b.x - c.x) * uy + (b.y - c.y) * ux;
    let lo = 0,
      hi = 1;
    for (const [v, delta, min, max] of [
      [ax, bx - ax, 0, length],
      [ay, by - ay, -radius, radius],
    ]) {
      if (Math.abs(delta!) < EPS) {
        if (v! < min! || v! > max!) hi = -1;
      } else {
        const t1 = (min! - v!) / delta!,
          t2 = (max! - v!) / delta!;
        lo = Math.max(lo, Math.min(t1, t2));
        hi = Math.min(hi, Math.max(t1, t2));
      }
    }
    if (hi > lo) intervals.push([lo, hi]);
  }
  const merged: Interval[] = [];
  for (const interval of intervals.sort((x, y) => x[0] - y[0])) {
    const last = merged[merged.length - 1];
    if (last && interval[0] <= last[1] + EPS) last[1] = Math.max(last[1], interval[1]);
    else merged.push([...interval]);
  }
  return merged;
}
export function eraseGesture(
  strokes: InkStroke[],
  gesture: XY[],
  mode: EraserMode,
  radius: number,
): InkStroke[] {
  if (!gesture.length) return strokes;
  const path = simplifyGesture(gesture);
  if (mode === "lasso") {
    const ids = new Set(selectLasso(strokes, path));
    return ids.size ? strokes.filter((s) => !ids.has(s.id)) : strokes;
  }
  let changed = false;
  const result: InkStroke[] = [];
  const pathBounds = boundsOf(path)!;
  for (const stroke of strokes) {
    if (!overlaps(strokeBounds(stroke), pathBounds, radius + (stroke.width * 1.75) / 2)) {
      result.push(stroke);
      continue;
    }
    let touched = false;
    const fragments: InkPoint[][] = [];
    let fragment: InkPoint[] = [];
    const flush = () => {
      if (fragment.length) fragments.push(fragment);
      fragment = [];
    };
    const push = (p: InkPoint) => {
      const last = fragment[fragment.length - 1];
      if (!last || last.x !== p.x || last.y !== p.y || last.pressure !== p.pressure)
        fragment.push(p);
    };
    for (let i = 0; i < stroke.points.length; i++) {
      const a = stroke.points[Math.max(0, i - 1)]!,
        b = stroke.points[i]!;
      // Include the rendered stroke's radius; partial erasing removes vector segments.
      const cuts = cutIntervals(
        a,
        b,
        path,
        radius + (stroke.width * (0.25 + 1.5 * Math.max(a.pressure, b.pressure))) / 2,
      );
      if (!cuts.length) {
        if (mode === "pixel") {
          push(a);
          push(b);
        }
        continue;
      }
      touched = true;
      if (mode === "stroke") break;
      let start = 0;
      for (const [lo, hi] of cuts) {
        if (lo > start + EPS) {
          push(interpolate(a, b, start));
          push(interpolate(a, b, lo));
        }
        flush();
        start = hi;
      }
      if (start < 1 - EPS) {
        push(interpolate(a, b, start));
        push(b);
      }
    }
    flush();
    if (!touched) result.push(stroke);
    else {
      changed = true;
      if (mode === "pixel")
        fragments.forEach((points, i) =>
          result.push({ ...stroke, id: i === 0 ? stroke.id : crypto.randomUUID(), points }),
        );
    }
  }
  return changed ? result : strokes;
}

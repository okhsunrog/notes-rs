import type { InkDraft, InkPoint, InkStroke } from "@/lib/bindings";

export const MAX_INK_POINTS = 150_000;
export const emptyDraft = (): InkDraft => ({ version: 1, width: 1000, height: 1400, strokes: [] });

export function inkPoint(
  event: Pick<
    PointerEvent,
    "clientX" | "clientY" | "pressure" | "tiltX" | "tiltY" | "timeStamp" | "pointerType"
  >,
  rect: Pick<DOMRect, "left" | "top" | "width" | "height">,
): InkPoint {
  return {
    x: Math.max(0, Math.min(1000, ((event.clientX - rect.left) / rect.width) * 1000)),
    y: Math.max(0, Math.min(1400, ((event.clientY - rect.top) / rect.height) * 1400)),
    pressure: event.pointerType === "pen" ? Math.max(0, Math.min(1, event.pressure)) : 0.5,
    tiltX: event.tiltX || 0,
    tiltY: event.tiltY || 0,
    time: event.timeStamp,
  };
}

function segmentDistance(point: InkPoint, a: InkPoint, b: InkPoint) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const length = dx * dx + dy * dy;
  const t =
    length === 0
      ? 0
      : Math.max(0, Math.min(1, ((point.x - a.x) * dx + (point.y - a.y) * dy) / length));
  return Math.hypot(point.x - a.x - t * dx, point.y - a.y - t * dy);
}

export function eraseAt(strokes: InkStroke[], point: InkPoint, radius = 12): InkStroke[] {
  return strokes.filter(
    (stroke) =>
      !stroke.points.some(
        (p, index) =>
          segmentDistance(point, stroke.points[Math.max(0, index - 1)]!, p) <=
          radius + stroke.width / 2,
      ),
  );
}

export function drawSegment(
  ctx: CanvasRenderingContext2D,
  a: InkPoint,
  b: InkPoint,
  width: number,
) {
  ctx.strokeStyle = "#111111";
  ctx.fillStyle = "#111111";
  ctx.lineWidth = width * (0.25 + 1.5 * ((a.pressure + b.pressure) / 2));
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.beginPath();
  if (a.x === b.x && a.y === b.y) {
    ctx.arc(a.x, a.y, ctx.lineWidth / 2, 0, Math.PI * 2);
    ctx.fill();
  } else {
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();
  }
}

export function drawSheet(ctx: CanvasRenderingContext2D, strokes: InkStroke[]) {
  ctx.clearRect(0, 0, 1000, 1400);
  for (const stroke of strokes) {
    for (let i = 0; i < stroke.points.length; i++) {
      drawSegment(ctx, stroke.points[Math.max(0, i - 1)]!, stroke.points[i]!, stroke.width);
    }
  }
}

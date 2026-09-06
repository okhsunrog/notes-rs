import type { InkBackground, InkDraft, InkPoint, InkStroke } from "@/lib/bindings";

export const MAX_INK_POINTS = 150_000;
export const emptyDraft = (): InkDraft => ({
  version: 1,
  width: 1000,
  height: 1400,
  strokes: [],
  background: "plain",
});

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

/** The single ink color strokes are drawn with today. Stroke data carries no color yet. */
export const INK_COLOR = "#111111";

/**
 * Maps an ink color onto the gray axis by relative luminance, for panels that cannot render
 * hue. Anything that is not a six-digit hex color is returned unchanged.
 */
export function inkStrokeColor(color: string, mono?: boolean): string {
  if (!mono) return color;
  const match = /^#([0-9a-f]{6})$/i.exec(color.trim());
  if (!match) return color;
  const value = Number.parseInt(match[1]!, 16);
  const channel = (shift: number) => ((value >> shift) & 0xff) / 255;
  // WCAG relative luminance: linearize each channel, then weight it by eye sensitivity.
  const linear = (raw: number) => (raw <= 0.04045 ? raw / 12.92 : ((raw + 0.055) / 1.055) ** 2.4);
  const luminance =
    0.2126 * linear(channel(16)) + 0.7152 * linear(channel(8)) + 0.0722 * linear(channel(0));
  // Back to sRGB so the gray reads as the same brightness the color had.
  const encoded =
    luminance <= 0.0031308 ? luminance * 12.92 : 1.055 * luminance ** (1 / 2.4) - 0.055;
  const level = Math.max(0, Math.min(255, Math.round(encoded * 255)))
    .toString(16)
    .padStart(2, "0");
  return `#${level}${level}${level}`;
}

export function drawSegment(
  ctx: CanvasRenderingContext2D,
  a: InkPoint,
  b: InkPoint,
  width: number,
  mono?: boolean,
) {
  const color = inkStrokeColor(INK_COLOR, mono);
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
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

export function drawSheet(
  ctx: CanvasRenderingContext2D,
  strokes: InkStroke[],
  background: InkBackground = "plain",
  mono?: boolean,
) {
  ctx.clearRect(0, 0, 1000, 1400);
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, 1000, 1400);
  if (background === "grid") {
    // Thin light gray lines can disappear under e-ink contrast processing.
    ctx.strokeStyle = "#777777";
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let x = 25; x < 1000; x += 25) {
      ctx.moveTo(x, 0);
      ctx.lineTo(x, 1400);
    }
    for (let y = 25; y < 1400; y += 25) {
      ctx.moveTo(0, y);
      ctx.lineTo(1000, y);
    }
    ctx.stroke();
  }
  for (const stroke of strokes) {
    for (let i = 0; i < stroke.points.length; i++) {
      drawSegment(ctx, stroke.points[Math.max(0, i - 1)]!, stroke.points[i]!, stroke.width, mono);
    }
  }
}

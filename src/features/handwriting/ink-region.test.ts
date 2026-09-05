import { expect, it, vi } from "vitest";
import type { InkStroke } from "@/lib/bindings";
import { changedInkBounds, drawInkRegion, inkBounds, SHEET_BOUNDS } from "./ink-region";

const dot = (id: string, x: number): InkStroke => ({
  id,
  width: 4,
  points: [{ x, y: 100, pressure: 1, tiltX: 0, tiltY: 0, time: 0 }],
});

it("covers both the old and new pressure-expanded ink, deletion and reorder", () => {
  const a = dot("a", 100),
    moved = dot("a", 300),
    b = dot("b", 800);
  expect(changedInkBounds([a, b], [moved, b])).toEqual({
    left: 94.5,
    right: 305.5,
    top: 94.5,
    bottom: 105.5,
  });
  expect(changedInkBounds([a, b], [b])).toEqual(inkBounds(a));
  expect(changedInkBounds([a, b], [a, b])).toBeNull();
  expect(changedInkBounds([a, b], [b, a])).toEqual(SHEET_BOUNDS);
});

it("redraws overlapping ink in document order, skips distant ink and clips to whole pixels", () => {
  const ctx = {
    canvas: { width: 500, height: 700 },
    setTransform: vi.fn(),
    drawImage: vi.fn(),
    getTransform: () => ({ a: 0.5, d: 0.5 }),
    save: vi.fn(),
    restore: vi.fn(),
    beginPath: vi.fn(),
    rect: vi.fn(),
    clip: vi.fn(),
    clearRect: vi.fn(),
    fillRect: vi.fn(),
    arc: vi.fn(),
    fill: vi.fn(),
  };
  drawInkRegion(
    ctx as unknown as CanvasRenderingContext2D,
    [dot("a", 102), dot("distant", 800), dot("overlap", 100)],
    "plain",
    { left: 97.3, top: 95.1, right: 105.5, bottom: 109.1 },
    ctx as unknown as CanvasRenderingContext2D,
  );
  expect(ctx.drawImage).toHaveBeenCalledWith(ctx.canvas, 48, 47, 5, 8, 48, 47, 5, 8);
  expect(ctx.arc.mock.calls.map((args) => args[0])).toEqual([102, 100]);
  expect(ctx.restore).toHaveBeenCalledOnce();
});

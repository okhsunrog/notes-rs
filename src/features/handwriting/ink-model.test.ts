import { describe, expect, it } from "vitest";
import { eraseAt, INK_COLOR, inkStrokeColor, inkPoint } from "./ink-model";
import type { InkPoint, InkStroke } from "@/lib/bindings";

const point = (x: number, y: number): InkPoint => ({
  x,
  y,
  pressure: 0.5,
  tiltX: 0,
  tiltY: 0,
  time: 1,
});
const stroke: InkStroke = { id: "one", width: 3, points: [point(0, 100), point(100, 100)] };

describe("ink geometry", () => {
  it("erases between sparse samples and leaves distant strokes intact", () => {
    expect(eraseAt([stroke], point(50, 105))).toEqual([]);
    expect(eraseAt([stroke], point(50, 140))).toEqual([stroke]);
    expect(eraseAt([{ ...stroke, points: [point(10, 10)] }], point(10, 10))).toEqual([]);
  });
  it("maps screen coordinates independently of display scale and preserves real pen pressure", () => {
    const event = {
      clientX: 260,
      clientY: 370,
      pressure: 0.73,
      tiltX: 12,
      tiltY: -30,
      timeStamp: 3,
      pointerType: "pen",
    };
    const mapped = inkPoint(event, { left: 10, top: 20, width: 500, height: 700 });
    expect(mapped).toEqual({ x: 500, y: 700, pressure: 0.73, tiltX: 12, tiltY: -30, time: 3 });
    expect(
      inkPoint(
        { ...event, pointerType: "mouse", pressure: 0 },
        { left: 10, top: 20, width: 500, height: 700 },
      ).pressure,
    ).toBe(0.5);
  });
});

describe("monochrome ink", () => {
  it("leaves colors untouched while the panel renders color", () => {
    expect(inkStrokeColor("#c64c7b")).toBe("#c64c7b");
    expect(inkStrokeColor("#c64c7b", false)).toBe("#c64c7b");
  });

  it("maps a color onto the gray axis by relative luminance", () => {
    expect(inkStrokeColor("#000000", true)).toBe("#000000");
    expect(inkStrokeColor("#ffffff", true)).toBe("#ffffff");
    // Green weighs far more than blue, so the same channel value lands much lighter.
    const green = inkStrokeColor("#00ff00", true);
    const blue = inkStrokeColor("#0000ff", true);
    const level = (color: string) => Number.parseInt(color.slice(1, 3), 16);
    expect(level(green)).toBeGreaterThan(level(blue));
    expect(green).toMatch(/^#([0-9a-f]{2})\1\1$/);
    expect(blue).toMatch(/^#([0-9a-f]{2})\1\1$/);
  });

  it("keeps today's near-black ink near black", () => {
    expect(level(inkStrokeColor(INK_COLOR, true))).toBeLessThan(0x30);
  });

  it("returns anything that is not a six-digit hex color unchanged", () => {
    expect(inkStrokeColor("rebeccapurple", true)).toBe("rebeccapurple");
    expect(inkStrokeColor("#abc", true)).toBe("#abc");
    expect(inkStrokeColor("", true)).toBe("");
  });
});

function level(color: string): number {
  return Number.parseInt(color.slice(1, 3), 16);
}

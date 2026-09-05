import { describe, expect, it } from "vitest";
import { eraseAt, inkPoint } from "./ink-model";
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

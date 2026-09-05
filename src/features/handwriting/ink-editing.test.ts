import { expect, it } from "vitest";
import type { InkPoint, InkStroke } from "@/lib/bindings";
import {
  eraseGesture,
  lassoPolygon,
  moveSelection,
  scaleSelection,
  selectLasso,
} from "./ink-editing";
const point = (x: number, y: number, time = x): InkPoint => ({
  x,
  y,
  time,
  pressure: 0.5,
  tiltX: 12,
  tiltY: -4,
});
const stroke: InkStroke = { id: "a", width: 2, points: [point(0, 100), point(200, 100)] };
it("cuts the middle of a sparse stroke and interpolates metadata without changing the original", () => {
  const result = eraseGesture([stroke], [point(100, 50), point(100, 150)], "pixel", 9);
  expect(result).toHaveLength(2);
  expect(result[0]!.points[result[0]!.points.length - 1]).toEqual(point(90, 100));
  expect(result[1]!.points[0]).toMatchObject({ y: 100, pressure: 0.5, tiltX: 12, tiltY: -4 });
  expect(result[1]!.points[0]!.x).toBeCloseTo(110);
  expect(result[1]!.points[0]!.time).toBeCloseTo(110);
  expect(result[1]!.id).not.toBe(stroke.id);
  expect(stroke.points).toHaveLength(2);
});
it("whole-stroke erasing covers the space between eraser samples and keeps unrelated ink", () => {
  const other = { ...stroke, id: "b", points: [point(800, 900)] };
  expect(eraseGesture([stroke, other], [point(100, 0), point(100, 200)], "stroke", 9)).toEqual([
    other,
  ]);
  expect(eraseGesture([other], [point(100, 0)], "pixel", 9)[0]).toBe(other);
});
it("handles dots, fully erased strokes and repeated erasure without zero-length fragments", () => {
  expect(
    eraseGesture([{ ...stroke, points: [point(100, 100)] }], [point(100, 100)], "pixel", 9),
  ).toEqual([]);
  expect(eraseGesture([stroke], [point(0, 100), point(200, 100)], "pixel", 9)).toEqual([]);
  const split = eraseGesture([stroke], [point(100, 100)], "pixel", 9);
  const again = eraseGesture(split, [point(100, 100)], "pixel", 9);
  expect(again).toEqual(split);
});
it("lasso selects crossing segments with both endpoints outside, supports rectangles and rejects taps", () => {
  const polygon = lassoPolygon([point(80, 80), point(120, 120)], "rectangle");
  expect(selectLasso([stroke], polygon)).toEqual(["a"]);
  expect(eraseGesture([stroke], polygon, "lasso", 9)).toEqual([]);
  expect(selectLasso([stroke], [point(100, 100)])).toEqual([]);
  expect(
    selectLasso([stroke], [point(80, 80), point(120, 80), point(120, 90), point(80, 90)]),
  ).toEqual([]);
});
it("moves the whole selection together at sheet edges and preserves input metadata", () => {
  const moved = moveSelection([stroke], ["a"], 900, -200);
  expect(moved[0]!.points).toEqual([
    { ...point(0, 100), x: 800, y: 0 },
    { ...point(200, 100), x: 1000, y: 0 },
  ]);
  expect(scaleSelection(moved, ["a"], 2)[0]!.points).toEqual(moved[0]!.points);
});

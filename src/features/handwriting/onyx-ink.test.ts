import { expect, it } from "vitest";
import { applyOnyxStroke, type OnyxInkEvent } from "./onyx-ink";
import { emptyDraft, MAX_INK_POINTS } from "./ink-model";

const point = { x: 100, y: 200, pressure: 0.8, tiltX: 12, tiltY: -5, time: 10 };
const event: OnyxInkEvent = {
  session: "test",
  kind: "stroke",
  sequence: 1,
  width: 3,
  erasing: false,
  points: [point],
};

it("preserves portable pressure and tilt across consecutive native batches", () => {
  const first = applyOnyxStroke(emptyDraft(), event);
  const second = applyOnyxStroke(first, { ...event, sequence: 2 });
  expect(second.strokes).toHaveLength(2);
  expect(second.strokes[0]!.points[0]).toEqual(point);
  expect(second.strokes[0]!.id).not.toBe(second.strokes[1]!.id);
  expect(first.strokes).toHaveLength(1);
});

it("uses whole-stroke erasing and preserves unrelated handwriting", () => {
  const first = applyOnyxStroke(emptyDraft(), event);
  const second = applyOnyxStroke(first, { ...event, points: [{ ...point, x: 800 }] });
  const erased = applyOnyxStroke(second, { ...event, erasing: true });
  expect(erased.strokes).toEqual([second.strokes[1]]);
});

it("bounds native batches by the remaining shared point budget without deleting earlier strokes", () => {
  const draft = applyOnyxStroke(emptyDraft(), {
    ...event,
    points: Array(MAX_INK_POINTS - 1).fill(point),
  });
  const full = applyOnyxStroke(draft, { ...event, points: [point, point, point] });
  expect(full.strokes[1]!.points).toHaveLength(1);
  expect(applyOnyxStroke(full, event)).toBe(full);
  expect(applyOnyxStroke(full, { ...event, erasing: true }).strokes).toHaveLength(0);
});

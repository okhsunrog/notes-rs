import { expect, it, vi } from "vitest";
import { incrementalDraftSaver } from "./ink-patch";
import { DraftWriter } from "./draft-writer";
import { emptyDraft } from "./ink-model";
import type { InkStroke } from "@/lib/bindings";
const a: InkStroke = {
  id: "a",
  width: 3,
  points: [{ x: 10, y: 20, pressure: 0.5, tiltX: 0, tiltY: 0, time: 0 }],
};
const b: InkStroke = { ...a, id: "b" };

it("sends no points for deletion, retains order, and resends ink restored by Undo", async () => {
  const save = vi.fn().mockResolvedValue("next");
  const draft = { ...emptyDraft(), strokes: [a, b] };
  const writer = new DraftWriter("initial", incrementalDraftSaver(draft, save), vi.fn());
  await writer.write({ ...draft, strokes: [b], background: "grid" });
  expect(save).toHaveBeenLastCalledWith(
    { order: ["b"], upserts: [], background: "grid" },
    "initial",
  );
  await writer.write(draft);
  expect(save).toHaveBeenLastCalledWith(
    { order: ["a", "b"], upserts: [a], background: "plain" },
    "next",
  );
});

it("uses the last successful snapshot after failure and preserves each queued gesture", async () => {
  let resolve!: (revision: string) => void;
  const save = vi
    .fn()
    .mockRejectedValueOnce(new Error("disk full"))
    .mockImplementationOnce(
      () =>
        new Promise<string>((done) => {
          resolve = done;
        }),
    )
    .mockResolvedValue("third");
  const writer = new DraftWriter(null, incrementalDraftSaver(emptyDraft(), save), vi.fn());
  const first = { ...emptyDraft(), strokes: [a] };
  expect(await writer.write(first)).toBe(false);
  const retry = writer.flush();
  void writer.write({ ...first, strokes: [a, b] });
  void writer.write({ ...first, strokes: [b] });
  expect(save.mock.calls[0]![0]).toEqual(save.mock.calls[1]![0]);
  resolve("second");
  await retry;
  expect(save).toHaveBeenCalledTimes(4);
  expect(save).toHaveBeenNthCalledWith(
    3,
    { order: ["a", "b"], upserts: [b], background: "plain" },
    "second",
  );
  expect(save).toHaveBeenLastCalledWith(
    { order: ["b"], upserts: [], background: "plain" },
    "third",
  );
});

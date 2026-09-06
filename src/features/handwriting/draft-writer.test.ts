import { expect, it, vi } from "vitest";
import { DraftWriter } from "./draft-writer";
import { emptyDraft } from "./ink-model";

it("serializes in-flight saves using their acknowledged revision", async () => {
  let resolve!: (revision: string) => void;
  const first = new Promise<string>((done) => {
    resolve = done;
  });
  const save = vi.fn().mockReturnValueOnce(first).mockResolvedValueOnce("second");
  const writer = new DraftWriter(null, save, vi.fn());
  const a = emptyDraft();
  const b = emptyDraft();
  void writer.write(a);
  void writer.write(b);
  expect(save).toHaveBeenCalledTimes(1);
  resolve("first");
  expect(await writer.flush()).toBe(true);
  expect(save).toHaveBeenNthCalledWith(2, b, "first");
});

it("keeps a failed draft for explicit retry", async () => {
  const save = vi.fn().mockRejectedValueOnce(new Error("disk full")).mockResolvedValueOnce("ok");
  const writer = new DraftWriter("previous", save, vi.fn());
  const draft = emptyDraft();
  expect(await writer.write(draft)).toBe(false);
  expect(await writer.flush()).toBe(true);
  expect(save).toHaveBeenNthCalledWith(2, draft, "previous");
});

it("keeps every queued gesture and retries a failure before later edits", async () => {
  let reject!: (error: Error) => void;
  const first = new Promise<string>((_, fail) => {
    reject = fail;
  });
  const save = vi
    .fn()
    .mockReturnValueOnce(first)
    .mockResolvedValueOnce("a")
    .mockResolvedValueOnce("b")
    .mockResolvedValueOnce("c");
  const writer = new DraftWriter(null, save, vi.fn());
  const a = emptyDraft(),
    b = emptyDraft(),
    c = emptyDraft();
  const saving = writer.write(a);
  void writer.write(b);
  void writer.write(c);
  reject(new Error("disk full"));
  expect(await saving).toBe(false);
  expect(await writer.flush()).toBe(true);
  expect(save.mock.calls).toEqual([
    [a, null],
    [a, null],
    [b, "a"],
    [c, "b"],
  ]);
  expect(writer.getRevision()).toBe("c");
});

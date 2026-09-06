import { expect, it, vi } from "vitest";
import { completeDraft } from "./complete-draft";
import { DraftWriter } from "./draft-writer";
import { emptyDraft } from "./ink-model";

it("waits for saved gestures, permits navigation, then awaits compaction", async () => {
  let saved!: (revision: string) => void;
  let packed!: () => void;
  const events: string[] = [];
  const writer = new DraftWriter(
    null,
    () =>
      new Promise((r) => {
        saved = r;
      }),
    vi.fn(),
  );
  void writer.write(emptyDraft());
  const compact = vi.fn(
    () =>
      new Promise<void>((r) => {
        events.push("compact");
        packed = r;
      }),
  );
  const completion = completeDraft(writer, compact, () => events.push("navigate"));
  let done = false;
  void completion.then(() => {
    done = true;
  });
  expect(compact).not.toHaveBeenCalled();
  saved("revision");
  await writer.flush();
  await Promise.resolve();
  expect(events).toEqual(["navigate", "compact"]);
  expect(done).toBe(false);
  packed();
  expect(await completion).toBe(true);
});

it("does not compact or navigate when saving failed", async () => {
  const writer = new DraftWriter(
    null,
    async () => {
      throw new Error("disk full");
    },
    vi.fn(),
  );
  await writer.write(emptyDraft());
  const compact = vi.fn(),
    navigate = vi.fn();
  expect(await completeDraft(writer, compact, navigate)).toBe(false);
  expect(compact).not.toHaveBeenCalled();
  expect(navigate).not.toHaveBeenCalled();
});

it("propagates a compaction failure rather than declaring completion", async () => {
  const writer = new DraftWriter(null, async () => "saved", vi.fn());
  await writer.write(emptyDraft());
  await expect(
    completeDraft(writer, async () => {
      throw new Error("pack failed");
    }),
  ).rejects.toThrow("pack failed");
  expect(writer.getRevision()).toBe("saved");
});

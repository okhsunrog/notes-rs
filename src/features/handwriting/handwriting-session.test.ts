import { afterEach, expect, it, vi } from "vite-plus/test";
import {
  acquireEditor,
  awaitCompletion,
  beginSession,
  endSession,
  flushAllSessions,
  getWriter,
  hasPendingCompletion,
  openSessionUuids,
  releaseEditor,
  requestCompletion,
  resetHandwritingSessions,
} from "./handwriting-session";
import { emptyDraft } from "./ink-model";

afterEach(() => resetHandwritingSessions());

const snapshot = () => ({ draft: emptyDraft(), revision: null });

it("keeps one writer per note and binds saves to that note", async () => {
  const save = vi.fn(async () => "revision-1");
  const writer = beginSession("page-a", snapshot(), () => {}, save);
  expect(getWriter("page-a")).toBe(writer);
  expect(getWriter("page-b")).toBeNull();

  await writer.write({ ...emptyDraft(), background: "grid" });

  expect(save).toHaveBeenCalledWith(
    "page-a",
    expect.objectContaining({ background: "grid" }),
    null,
  );
  expect(writer.getRevision()).toBe("revision-1");
});

it("runs one completion at a time and clears it once it settles", async () => {
  let finish!: () => void;
  const run = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );

  const first = requestCompletion("page-a", run);
  const second = requestCompletion("page-a", run);

  expect(second).toBe(first);
  expect(run).toHaveBeenCalledTimes(1);
  expect(hasPendingCompletion("page-a")).toBe(true);

  finish();
  await first;

  expect(hasPendingCompletion("page-a")).toBe(false);
  await requestCompletion("page-a", async () => undefined);
  expect(run).toHaveBeenCalledTimes(1);
});

it("lets a later mount wait for the completion an earlier session started", async () => {
  const order: string[] = [];
  let finish!: () => void;
  const completion = requestCompletion(
    "page-a",
    () =>
      new Promise<void>((resolve) => {
        finish = () => {
          order.push("completed");
          resolve();
        };
      }),
  );

  const remount = awaitCompletion("page-a").then(() => order.push("reopened"));
  finish();
  await completion;
  await remount;

  expect(order).toEqual(["completed", "reopened"]);
});

it("does not reject the wait when the completion failed", async () => {
  const completion = requestCompletion("page-a", async () => {
    throw new Error("outbox unavailable");
  });

  await expect(completion).rejects.toThrow("outbox unavailable");
  await expect(awaitCompletion("page-a")).resolves.toBeUndefined();
});

it("keeps the writer and its pending gestures when completion fails", async () => {
  let allow = false;
  const save = vi.fn(async () => {
    if (!allow) throw new Error("disk full");
    return "revision-1";
  });
  const writer = beginSession("page-a", snapshot(), () => {}, save);
  await writer.write({ ...emptyDraft(), background: "grid" });

  await expect(
    requestCompletion("page-a", async () => {
      throw new Error("packing failed");
    }),
  ).rejects.toThrow("packing failed");

  expect(getWriter("page-a")).toBe(writer);
  allow = true;
  expect(await writer.flush()).toBe(true);
  expect(save).toHaveBeenLastCalledWith(
    "page-a",
    expect.objectContaining({ background: "grid" }),
    null,
  );
});

it("flushes every open session before a process-wide completion", async () => {
  const saved: string[] = [];
  const save = vi.fn(async (pageUuid: string) => {
    saved.push(pageUuid);
    return "revision-1";
  });
  const first = beginSession("page-a", snapshot(), () => {}, save);
  const second = beginSession("page-b", snapshot(), () => {}, save);
  void first.write({ ...emptyDraft(), background: "grid" });
  void second.write({ ...emptyDraft(), background: "grid" });

  expect(openSessionUuids()).toEqual(["page-a", "page-b"]);
  expect(await flushAllSessions()).toBe(true);
  expect([...saved].sort()).toEqual(["page-a", "page-b"]);

  endSession("page-a", first);
  expect(openSessionUuids()).toEqual(["page-b"]);
});

it("grants the editable view to one owner until it is released", () => {
  const first = {};
  const second = {};

  expect(acquireEditor("page-a", first)).toBe(true);
  expect(acquireEditor("page-a", first)).toBe(true);
  expect(acquireEditor("page-a", second)).toBe(false);
  expect(acquireEditor("page-b", second)).toBe(true);

  releaseEditor("page-a", second);
  expect(acquireEditor("page-a", second)).toBe(false);

  releaseEditor("page-a", first);
  expect(acquireEditor("page-a", second)).toBe(true);
});

it("refuses to replace a writer that still owes gestures", async () => {
  let allow = false;
  const save = vi.fn(async () => {
    if (!allow) throw new Error("disk full");
    return "revision-2";
  });
  const writer = beginSession("page-a", snapshot(), () => {}, save);
  await writer.write({ ...emptyDraft(), background: "grid" });

  expect(writer.hasPending()).toBe(true);
  expect(() => beginSession("page-a", snapshot(), () => {}, save)).toThrow(
    "Unsaved handwriting is still queued",
  );
  expect(getWriter("page-a")).toBe(writer);

  allow = true;
  expect(await writer.flush()).toBe(true);
  expect(writer.hasPending()).toBe(false);
  expect(beginSession("page-a", snapshot(), () => {}, save)).not.toBe(writer);
});

it("carries the newest queued gesture and its reporting into the next mount", async () => {
  const save = vi.fn(async () => {
    throw new Error("disk full");
  });
  const first = vi.fn();
  const writer = beginSession("page-a", snapshot(), first, save);
  const queued = { ...emptyDraft(), background: "grid" as const };
  await writer.write(queued);

  expect(writer.latestDraft()).toBe(queued);

  const second = vi.fn();
  first.mockClear();
  writer.setOnState(second);
  await writer.flush();

  // The retry reports to the mount that adopted the queue, not the dead one.
  expect(second).toHaveBeenCalledWith(expect.any(Error));
  expect(first).not.toHaveBeenCalled();
});

it("closes only the session the caller still owns", async () => {
  const save = vi.fn(async () => "revision-1");
  const stale = beginSession("page-a", snapshot(), () => {}, save);
  await stale.flush();
  const reopened = beginSession("page-a", snapshot(), () => {}, save);

  // The unmount of the previous view settles after the note was reopened.
  endSession("page-a", stale);

  expect(getWriter("page-a")).toBe(reopened);

  endSession("page-a", reopened);
  expect(getWriter("page-a")).toBeNull();
});

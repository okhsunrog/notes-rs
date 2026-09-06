// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vite-plus/test";
import {
  beginSession,
  resetHandwritingSessions,
  type SavePatch,
} from "@/features/handwriting/handwriting-session";
import { emptyDraft } from "@/features/handwriting/ink-model";
import { PageSessionRegistry } from "@/features/pages/page-session";
import { registerLifecycleFlush } from "./lifecycle-flush";

const api = vi.hoisted(() => ({ completeAllHandwriting: vi.fn() }));

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return { ...actual, ...api };
});

afterEach(() => {
  resetHandwritingSessions();
  api.completeAllHandwriting.mockReset();
});

it("drains a retained session when the window goes away with no view mounted", async () => {
  api.completeAllHandwriting.mockResolvedValue(undefined);
  registerLifecycleFlush(new PageSessionRegistry());
  const save = vi
    .fn<SavePatch>()
    .mockRejectedValueOnce(new Error("database is locked"))
    .mockResolvedValue("revision-2");
  const writer = beginSession(
    "019f0000-0000-7000-8000-00000000000a",
    { draft: emptyDraft(), revision: "revision-1" },
    () => {},
    save,
  );

  await writer.write({ ...emptyDraft(), background: "grid" });
  expect(writer.hasPending()).toBe(true);

  // Nothing is mounted; the process-level listener is the only thing that can
  // still store this gesture before the window is destroyed.
  window.dispatchEvent(new Event("pagehide"));

  await vi.waitFor(() => {
    expect(save).toHaveBeenCalledTimes(2);
    expect(api.completeAllHandwriting).toHaveBeenCalledTimes(1);
  });
});

it("stores a dirty text draft when the window is hidden", async () => {
  api.completeAllHandwriting.mockResolvedValue(undefined);
  const sessions = new PageSessionRegistry();
  registerLifecycleFlush(sessions);
  const save = vi.fn().mockResolvedValue(undefined);
  sessions.registerFlush(save);

  Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
  document.dispatchEvent(new Event("visibilitychange"));

  // Autosave is debounced, so without this the last keystrokes before the app
  // is backgrounded are never written.
  await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
});

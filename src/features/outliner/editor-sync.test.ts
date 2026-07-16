import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { DebouncedAction } from "@/lib/debounced-action";
import { reconcileRemoteDraft } from "./editor-sync";

afterEach(() => vi.useRealTimers());

describe("editor synchronization", () => {
  it("cancels a pending autosave before an immediate blur save", () => {
    vi.useFakeTimers();
    const debounced = new DebouncedAction();
    const save = vi.fn();

    debounced.schedule(save, 400);
    debounced.cancel();
    save();
    vi.advanceTimersByTime(400);

    expect(save).toHaveBeenCalledTimes(1);
    expect(debounced.pending).toBe(false);
  });

  it("accepts remote content only when the local draft is clean", () => {
    expect(reconcileRemoteDraft("old", "old", "remote")).toBe("accept_remote");
    expect(reconcileRemoteDraft("old", "local draft", "remote")).toBe("conflict");
    expect(reconcileRemoteDraft("old", "local draft", "old")).toBe("unchanged");
  });
});

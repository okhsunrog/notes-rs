import { describe, expect, it } from "vite-plus/test";
import type { SyncStatus } from "@/lib/api";
import { presentSyncStatus } from "./sync-status-presentation";

function status(state: SyncStatus["state"], pendingOperations = 0): SyncStatus {
  return {
    state,
    serverUrl: "https://notes.example.test/",
    lastServerSeq: 42,
    pendingOperations,
    message: null,
  };
}

describe("presentSyncStatus", () => {
  it("presents a caught-up online connection as synced", () => {
    expect(presentSyncStatus(status("online"))).toMatchObject({
      label: "Synced",
      tone: "success",
      animated: false,
      canRetry: false,
    });
  });

  it("presents an online connection with local work as syncing", () => {
    expect(presentSyncStatus(status("online", 2))).toMatchObject({
      label: "Syncing",
      description: "2 local changes waiting to sync.",
      tone: "progress",
      animated: true,
    });
  });

  it("separates transient offline state from terminal failures", () => {
    expect(presentSyncStatus(status("offline"))).toMatchObject({
      label: "Offline",
      tone: "warning",
      canRetry: true,
    });
    expect(presentSyncStatus(status("error"))).toMatchObject({
      label: "Sync error",
      tone: "danger",
      canRetry: true,
    });
    expect(presentSyncStatus(status("conflict"))).toMatchObject({
      label: "Sync conflict",
      tone: "danger",
      canRetry: true,
    });
  });
});

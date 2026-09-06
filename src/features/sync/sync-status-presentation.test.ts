import { describe, expect, it } from "vite-plus/test";
import type { SyncStatus } from "@/lib/api";
import { presentSyncStatus, rejectedChangesLabel } from "./sync-status-presentation";

function status(state: SyncStatus["state"], pendingOperations = 0): SyncStatus {
  return {
    state,
    serverUrl: "https://notes.example.test/",
    lastServerSeq: 42,
    pendingOperations,
    message: null,
    serverFormatVersion: null,
    quarantinedOperations: 0,
    quarantineReason: null,
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

  it("asks for an app update instead of offering a retry that cannot help", () => {
    expect(presentSyncStatus(status("update_required"))).toMatchObject({
      label: "Update required",
      description: "The sync server uses a newer data format. Update the app to keep syncing.",
      tone: "danger",
      animated: false,
      canRetry: false,
    });
    expect(
      presentSyncStatus({ ...status("update_required"), serverFormatVersion: 9 }).description,
    ).toBe("The sync server uses data format 9. Update the app to keep syncing.");
  });

  it("counts rejected changes without claiming they were lost", () => {
    expect(rejectedChangesLabel(1)).toBe("1 change could not be sent");
    expect(rejectedChangesLabel(3)).toBe("3 changes could not be sent");
  });
});

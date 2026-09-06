import type { SyncStatus } from "@/lib/api";

export type SyncStatusTone = "success" | "progress" | "warning" | "danger" | "muted";

export type SyncStatusPresentation = Readonly<{
  label: string;
  description: string;
  tone: SyncStatusTone;
  animated: boolean;
  canRetry: boolean;
}>;

export function presentSyncStatus(status: SyncStatus): SyncStatusPresentation {
  switch (status.state) {
    case "online":
      if (status.pendingOperations > 0) {
        return {
          label: "Syncing",
          description: pendingDescription(status.pendingOperations),
          tone: "progress",
          animated: true,
          canRetry: false,
        };
      }
      return {
        label: "Synced",
        description: "This device is up to date.",
        tone: "success",
        animated: false,
        canRetry: false,
      };
    case "connecting":
      return {
        label: "Connecting",
        description: "Connecting to the sync server…",
        tone: "progress",
        animated: true,
        canRetry: false,
      };
    case "syncing":
      return {
        label: "Syncing",
        description:
          status.pendingOperations > 0
            ? pendingDescription(status.pendingOperations)
            : "Checking for changes…",
        tone: "progress",
        animated: true,
        canRetry: false,
      };
    case "offline":
      return {
        label: "Offline",
        description: "Changes stay on this device. Reconnecting automatically…",
        tone: "warning",
        animated: false,
        canRetry: true,
      };
    case "conflict":
      return {
        label: "Sync conflict",
        description: "Sync needs your attention before it can continue.",
        tone: "danger",
        animated: false,
        canRetry: true,
      };
    case "update_required":
      return {
        label: "Update required",
        description:
          status.serverFormatVersion === null
            ? "The sync server uses a newer data format. Update the app to keep syncing."
            : `The sync server uses data format ${status.serverFormatVersion}. Update the app to keep syncing.`,
        tone: "danger",
        animated: false,
        canRetry: false,
      };
    case "error":
      return {
        label: "Sync error",
        description: "Sync stopped because of a configuration or server error.",
        tone: "danger",
        animated: false,
        canRetry: true,
      };
    case "disabled":
      return {
        label: "Sync off",
        description: "No sync server is configured for this device.",
        tone: "muted",
        animated: false,
        canRetry: false,
      };
  }
}

/**
 * Shown alongside the connection state: these changes are stored and will be
 * sent on retry, so the wording says "could not be sent", never "lost".
 */
export function rejectedChangesLabel(count: number): string {
  return `${count} ${count === 1 ? "change" : "changes"} could not be sent`;
}

function pendingDescription(count: number): string {
  return `${count} local ${count === 1 ? "change" : "changes"} waiting to sync.`;
}

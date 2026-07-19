import { describe, expect, it } from "vite-plus/test";
import type { SettingsSnapshot } from "@/lib/api";
import { toSettingsUpdate } from "./settings-update";

describe("settings update mapping", () => {
  it("keeps the persisted settings contract in one place", () => {
    const snapshot = {
      windowDecorationMode: "native",
      syncServerUrl: "https://notes.example.test",
      aiSearchEnabled: true,
      aiSearchTrigger: "enter_only",
      aiSearchRerank: false,
      searchDebugSources: true,
      configuredKeys: [],
      configPath: "/tmp/settings",
    } satisfies SettingsSnapshot;

    const update = toSettingsUpdate(snapshot, { SYNC_TOKEN: "secret" }, ["SYNC_TOKEN"]);

    expect(update).not.toHaveProperty("configuredKeys");
    expect(update).not.toHaveProperty("configPath");
    expect(update.apiKeys).toEqual({ SYNC_TOKEN: "secret" });
    expect(update.clearKeys).toEqual(["SYNC_TOKEN"]);
    expect(update.syncServerUrl).toBe(snapshot.syncServerUrl);
    expect(update.aiSearchEnabled).toBe(true);
    expect(update.aiSearchTrigger).toBe("enter_only");
    expect(update.aiSearchRerank).toBe(false);
    expect(update.searchDebugSources).toBe(true);
  });
});

import { describe, expect, it } from "vite-plus/test";
import type { SettingsSnapshot } from "@/lib/api";
import { toSettingsUpdate } from "./settings-update";

describe("settings update mapping", () => {
  it("keeps the persisted settings contract in one place", () => {
    const snapshot = {
      windowCornerRadius: 10,
      windowCornerRoundingSupported: true,
      windowDecorationMode: "native",
      activeWindowDecorationMode: "borderless",
      windowDecorationsRequireRestart: true,
      startupView: "specific_page",
      startupPageUuid: "019cfa51-8d73-7b53-b090-cdb945bb1b4d",
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
    expect(update).not.toHaveProperty("activeWindowDecorationMode");
    expect(update).not.toHaveProperty("windowDecorationsRequireRestart");
    expect(update.windowDecorationMode).toBe("native");
    expect(update.apiKeys).toEqual({ SYNC_TOKEN: "secret" });
    expect(update.clearKeys).toEqual(["SYNC_TOKEN"]);
    expect(update.syncServerUrl).toBe(snapshot.syncServerUrl);
    expect(update.startupView).toBe("specific_page");
    expect(update.startupPageUuid).toBe(snapshot.startupPageUuid);
    expect(update.aiSearchEnabled).toBe(true);
    expect(update.aiSearchTrigger).toBe("enter_only");
    expect(update.aiSearchRerank).toBe(false);
    expect(update.searchDebugSources).toBe(true);
  });
});

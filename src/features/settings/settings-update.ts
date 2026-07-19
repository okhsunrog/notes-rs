import type { SecretKey, SettingsSnapshot, SettingsUpdate } from "@/lib/api";

export function toSettingsUpdate(
  settings: SettingsSnapshot,
  apiKeys: Partial<Record<SecretKey, string>> = {},
  clearKeys: SecretKey[] = [],
): SettingsUpdate {
  return {
    windowDecorationMode: settings.windowDecorationMode,
    syncServerUrl: settings.syncServerUrl,
    aiSearchEnabled: settings.aiSearchEnabled,
    aiSearchTrigger: settings.aiSearchTrigger,
    aiSearchRerank: settings.aiSearchRerank,
    searchDebugSources: settings.searchDebugSources,
    apiKeys,
    clearKeys,
  };
}

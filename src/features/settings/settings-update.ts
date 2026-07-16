import type { SecretKey, SettingsSnapshot, SettingsUpdate } from "@/lib/api";

export function toSettingsUpdate(
  settings: SettingsSnapshot,
  apiKeys: Partial<Record<SecretKey, string>> = {},
  clearKeys: SecretKey[] = [],
): SettingsUpdate {
  return {
    localOnly: settings.localOnly,
    entityExtractionEnabled: settings.entityExtractionEnabled,
    queryRewritingEnabled: settings.queryRewritingEnabled,
    chatModel: settings.chatModel,
    chatProtocol: settings.chatProtocol,
    chatBaseUrl: settings.chatBaseUrl,
    extractionModel: settings.extractionModel,
    extractionProtocol: settings.extractionProtocol,
    extractionBaseUrl: settings.extractionBaseUrl,
    embeddingProvider: settings.embeddingProvider,
    embeddingModel: settings.embeddingModel,
    embeddingNdims: settings.embeddingNdims,
    rerankProvider: settings.rerankProvider,
    rerankModel: settings.rerankModel,
    openrouterBaseUrl: settings.openrouterBaseUrl,
    openaiBaseUrl: settings.openaiBaseUrl,
    windowDecorationMode: settings.windowDecorationMode,
    syncDirectory: settings.syncDirectory,
    syncServerUrl: settings.syncServerUrl,
    apiKeys,
    clearKeys,
  };
}

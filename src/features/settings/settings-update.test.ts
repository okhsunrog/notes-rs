import { describe, expect, it } from "vite-plus/test";
import type { SettingsSnapshot } from "@/lib/api";
import { toSettingsUpdate } from "./settings-update";

describe("settings update mapping", () => {
  it("keeps the persisted settings contract in one place", () => {
    const snapshot = {
      localOnly: false,
      entityExtractionEnabled: true,
      queryRewritingEnabled: true,
      chatModel: "chat",
      chatProtocol: "openai",
      chatBaseUrl: "https://chat.example/v1",
      extractionModel: "extract",
      extractionProtocol: "inherit",
      extractionBaseUrl: null,
      embeddingProvider: "openai",
      embeddingModel: "embed",
      embeddingNdims: 1536,
      rerankProvider: "openrouter",
      rerankModel: "rerank",
      openrouterBaseUrl: "https://openrouter.ai/api/v1",
      openaiBaseUrl: "https://api.openai.com/v1",
      windowDecorationMode: "native",
      syncDirectory: null,
      syncServerUrl: "https://notes.example.test",
      configuredKeys: [],
      localModelsAvailable: false,
      configPath: "/tmp/settings",
    } satisfies SettingsSnapshot;

    const update = toSettingsUpdate(snapshot, { CHAT_API_KEY: "secret" }, ["OPENAI_API_KEY"]);

    expect(update).not.toHaveProperty("configuredKeys");
    expect(update).not.toHaveProperty("configPath");
    expect(update.apiKeys).toEqual({ CHAT_API_KEY: "secret" });
    expect(update.clearKeys).toEqual(["OPENAI_API_KEY"]);
    expect(update.chatBaseUrl).toBe(snapshot.chatBaseUrl);
    expect(update.syncServerUrl).toBe(snapshot.syncServerUrl);
  });
});

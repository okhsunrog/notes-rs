import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTheme } from "next-themes";
import {
  ArrowLeft,
  Check,
  Loader2,
  RotateCcw,
  Save,
  Trash2,
  Laptop,
  Moon,
  PlugZap,
  Sparkles,
  Sun,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { WindowControls } from "@/app/window-controls";
import { PALETTES, useAppearance } from "@/app/appearance";
import { cn } from "@/lib/utils";
import {
  createBackup,
  chooseSyncDirectory,
  clearBackgroundJobs,
  exportData,
  importData,
  getBackgroundStatus,
  getSyncStatus,
  loadSettings,
  restartApp,
  retryBackgroundJobs,
  saveSettings,
  setBackgroundPaused,
  testCompletionProvider,
  syncPull,
  syncPush,
  type SettingsSnapshot,
  type EmbeddingProvider,
  type RerankProvider,
  type SecretKey,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { Field, FieldGroup, ModeButton, SettingsSection, ToggleField } from "./settings-controls";
import { toSettingsUpdate } from "./settings-update";
import { DataSettingsSections } from "./data-settings-sections";

type Props = {
  onBack: () => void;
  onDecorationModeChanged: (mode: "native" | "borderless") => void;
  dataAvailable: boolean;
  onDataChanged: () => void;
};

const API_KEYS = [
  ["CHAT_API_KEY", "Chat API key"],
  ["EXTRACT_API_KEY", "Separate extraction API key"],
  ["OPENROUTER_API_KEY", "OpenRouter API key"],
  ["OPENAI_API_KEY", "OpenAI API key"],
  ["COHERE_API_KEY", "Cohere API key"],
  ["VOYAGE_API_KEY", "Voyage AI API key"],
  ["GEMINI_API_KEY", "Gemini API key"],
] as const satisfies readonly (readonly [SecretKey, string])[];

export function SettingsPage({
  onBack,
  onDecorationModeChanged,
  dataAvailable,
  onDataChanged,
}: Props) {
  const { theme, setTheme } = useTheme();
  const { palette, setPalette } = useAppearance();
  const queryClient = useQueryClient();
  const [settings, setSettings] = useState<SettingsSnapshot | null>(null);
  const [secrets, setSecrets] = useState<Partial<Record<SecretKey, string>>>({});
  const [clearKeys, setClearKeys] = useState<SecretKey[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [testingProvider, setTestingProvider] = useState<"chat" | "extraction" | null>(null);
  const [providerReports, setProviderReports] = useState<
    Record<
      "chat" | "extraction",
      Array<{ name: string; ok: boolean; latencyMs: number; detail: string }> | undefined
    >
  >({ chat: undefined, extraction: undefined });

  const settingsQuery = useQuery({ queryKey: queryKeys.settings, queryFn: loadSettings });
  const backgroundQuery = useQuery({
    queryKey: queryKeys.backgroundStatus,
    queryFn: getBackgroundStatus,
    enabled: dataAvailable,
  });
  const syncQuery = useQuery({
    queryKey: queryKeys.syncStatus,
    queryFn: getSyncStatus,
  });
  const background = backgroundQuery.data ?? null;

  useEffect(() => {
    if (settingsQuery.data) setSettings(settingsQuery.data);
  }, [settingsQuery.data]);

  useEffect(() => {
    const reason = settingsQuery.error ?? backgroundQuery.error;
    if (reason) setError(String(reason));
  }, [backgroundQuery.error, settingsQuery.error]);

  const update = <Key extends keyof SettingsSnapshot>(key: Key, value: SettingsSnapshot[Key]) => {
    setSettings((current) => (current ? { ...current, [key]: value } : current));
    setMessage("");
  };

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!settings) return;
    setBusy(true);
    setError("");
    try {
      const saved = await saveSettings(toSettingsUpdate(settings, secrets, clearKeys));
      setSettings(saved);
      queryClient.setQueryData(queryKeys.settings, saved);
      onDecorationModeChanged(saved.windowDecorationMode);
      setSecrets({});
      setClearKeys([]);
      setMessage("Saved. Restart the app to apply provider changes.");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function dataAction(action: "export" | "import" | "backup") {
    if (!dataAvailable) return;
    if (
      action === "import" &&
      !window.confirm(
        "Replace the current database with the selected archive? A backup will be created first.",
      )
    )
      return;
    setBusy(true);
    setError("");
    try {
      const path =
        action === "export"
          ? await exportData()
          : action === "import"
            ? await importData()
            : await createBackup();
      if (path) {
        setMessage(
          `${action === "backup" ? "Backup created" : action === "export" ? "Exported" : "Imported"}: ${path}`,
        );
        if (action === "import") onDataChanged();
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function chooseSync() {
    try {
      const directory = await chooseSyncDirectory();
      if (directory) update("syncDirectory", directory);
    } catch (reason) {
      setError(String(reason));
    }
  }

  async function runSync(direction: "push" | "pull") {
    const current = settings;
    if (!dataAvailable || !current) return;
    if (
      direction === "pull" &&
      !window.confirm(
        "Replace local data from the sync snapshot? A local backup will be created first.",
      )
    )
      return;
    setBusy(true);
    setError("");
    try {
      // Persist a newly selected directory before using it.
      const saved = await saveSettings(toSettingsUpdate(current));
      setSettings(saved);
      queryClient.setQueryData(queryKeys.settings, saved);
      const path = direction === "push" ? await syncPush() : await syncPull();
      setMessage(`${direction === "push" ? "Pushed" : "Pulled"} sync snapshot: ${path}`);
      if (direction === "pull") onDataChanged();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function backgroundAction(action: "pause" | "retry" | "clear") {
    if (!background) return;
    try {
      if (action === "pause") await setBackgroundPaused(!background.paused);
      if (action === "retry") await retryBackgroundJobs();
      if (
        action === "clear" &&
        window.confirm(
          "Cancel every pending embedding and entity-extraction job? Future note edits will enqueue fresh work.",
        )
      )
        await clearBackgroundJobs();
      await queryClient.invalidateQueries({ queryKey: queryKeys.backgroundStatus });
    } catch (reason) {
      setError(String(reason));
    }
  }

  async function testProvider(scope: "chat" | "extraction") {
    if (!settings) return;
    setTestingProvider(scope);
    setError("");
    setMessage("");
    try {
      const inherited = scope === "extraction" && settings.extractionProtocol === "inherit";
      const protocol =
        scope === "chat" || inherited ? settings.chatProtocol : settings.extractionProtocol;
      if (protocol === "inherit") throw new Error("invalid inherited extraction protocol");
      const result = await testCompletionProvider({
        protocol,
        baseUrl:
          scope === "chat" || inherited ? settings.chatBaseUrl : (settings.extractionBaseUrl ?? ""),
        model: scope === "chat" ? settings.chatModel : settings.extractionModel,
        apiKey:
          scope === "extraction" && !inherited
            ? secrets.EXTRACT_API_KEY || undefined
            : secrets.CHAT_API_KEY || undefined,
        keyScope: scope === "extraction" && !inherited ? "extraction" : "chat",
      });
      setProviderReports((current) => ({ ...current, [scope]: result.capabilities }));
    } catch (reason) {
      setError(
        `${scope === "chat" ? "Chat" : "Extraction"} provider test failed: ${String(reason)}`,
      );
    } finally {
      setTestingProvider(null);
    }
  }

  if (!settings) {
    return (
      <div className="flex h-screen items-center justify-center bg-background text-foreground">
        {error ? (
          <p className="text-sm text-destructive">{error}</p>
        ) : (
          <Loader2 className="animate-spin" />
        )}
      </div>
    );
  }

  return (
    <div className="app-shell h-screen overflow-y-auto bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="sticky top-0 z-20 flex h-16 items-center justify-between border-b border-border/50 bg-background/80 px-5 backdrop-blur-xl"
      >
        <div className="flex items-center gap-3">
          <Button
            variant="ghost"
            size="icon-sm"
            className="rounded-xl"
            onClick={onBack}
            aria-label="Back to notes"
          >
            <ArrowLeft className="size-4" />
          </Button>
          <div className="brand-mark flex size-9 items-center justify-center rounded-xl text-primary-foreground shadow-sm">
            <Sparkles className="size-4" />
          </div>
          <div>
            <h1 className="font-semibold tracking-tight">Settings</h1>
            <p className="text-xs text-muted-foreground">Make notes-rs feel like your own</p>
          </div>
        </div>
        {settings.windowDecorationMode === "borderless" && <WindowControls />}
      </header>

      <form onSubmit={submit} className="mx-auto max-w-4xl space-y-7 p-6 pb-24 sm:p-10">
        <SettingsSection
          title="Appearance"
          description="Choose a brightness mode and a color atmosphere. Every palette has a tuned light and dark version."
        >
          <FieldGroup label="Brightness">
            <div className="grid grid-cols-3 gap-2 rounded-2xl bg-muted/70 p-1.5">
              <ModeButton
                active={theme === "system"}
                icon={<Laptop className="size-4" />}
                label="System"
                onClick={() => setTheme("system")}
              />
              <ModeButton
                active={theme === "light"}
                icon={<Sun className="size-4" />}
                label="Light"
                onClick={() => setTheme("light")}
              />
              <ModeButton
                active={theme === "dark"}
                icon={<Moon className="size-4" />}
                label="Dark"
                onClick={() => setTheme("dark")}
              />
            </div>
          </FieldGroup>
          <FieldGroup label="Color palette" hint="Applied instantly and saved on this device.">
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {PALETTES.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  aria-pressed={palette === item.id}
                  onClick={() => setPalette(item.id)}
                  className={cn(
                    "group rounded-2xl border p-3 text-left transition hover:-translate-y-0.5 hover:shadow-md",
                    palette === item.id
                      ? "border-primary/50 bg-primary/8 ring-3 ring-primary/10"
                      : "border-border/60 bg-background/55 hover:border-primary/25",
                  )}
                >
                  <span className="mb-3 flex h-8 overflow-hidden rounded-xl ring-1 ring-black/5">
                    {item.swatches.map((color) => (
                      <span key={color} className="flex-1" style={{ backgroundColor: color }} />
                    ))}
                  </span>
                  <span className="flex items-center gap-2 text-sm font-semibold">
                    {item.name}
                    {palette === item.id && <Check className="ml-auto size-3.5 text-primary" />}
                  </span>
                  <span className="mt-1 block text-[11px] text-muted-foreground">
                    {item.description}
                  </span>
                </button>
              ))}
            </div>
          </FieldGroup>
        </SettingsSection>

        <SettingsSection
          title="AI & privacy"
          description="Control which note contents may leave this device. Provider changes apply after restart."
        >
          <ToggleField
            checked={settings.localOnly}
            disabled={!settings.localModelsAvailable}
            label="Local-only mode"
            description={
              settings.localModelsAvailable
                ? "Forces local embeddings and reranking, and disables cloud chat, rewriting, and extraction."
                : "This build does not include local models. Rebuild with the local-models feature to enable it."
            }
            onChange={(checked) =>
              setSettings((current) =>
                current
                  ? {
                      ...current,
                      localOnly: checked,
                      ...(checked
                        ? {
                            entityExtractionEnabled: false,
                            embeddingProvider: "local",
                            embeddingModel: "bge-m3",
                            embeddingNdims: 1024,
                            rerankProvider: "local",
                            rerankModel: "bge-reranker-v2-m3",
                          }
                        : {}),
                    }
                  : current,
              )
            }
          />
          <ToggleField
            checked={settings.entityExtractionEnabled}
            disabled={settings.localOnly}
            label="Automatic entity extraction"
            description="Sends changed note text to the configured extraction endpoint to build entity and relation edges."
            onChange={(checked) => update("entityExtractionEnabled", checked)}
          />
          <ToggleField
            checked={settings.queryRewritingEnabled}
            disabled={settings.localOnly}
            label="Conversational query rewriting"
            description="May make an extra model call when a short search query refers to earlier chat messages."
            onChange={(checked) => update("queryRewritingEnabled", checked)}
          />
          <Field label="Chat model" hint="Used by Chat and query rewriting.">
            <Input
              value={settings.chatModel}
              disabled={settings.localOnly}
              onChange={(event) => update("chatModel", event.currentTarget.value)}
            />
          </Field>
          <Field label="Chat API protocol">
            <select
              value={settings.chatProtocol}
              disabled={settings.localOnly}
              onChange={(event) => {
                const protocol = event.currentTarget.value as SettingsSnapshot["chatProtocol"];
                setSettings((current) =>
                  current
                    ? {
                        ...current,
                        chatProtocol: protocol,
                        chatBaseUrl:
                          protocol === "anthropic"
                            ? "https://api.anthropic.com"
                            : "https://api.openai.com/v1",
                      }
                    : current,
                );
              }}
              className="h-10 w-full rounded-xl border border-border/70 bg-background/70 px-3 text-sm shadow-none"
            >
              <option value="openai">OpenAI-compatible (Chat Completions)</option>
              <option value="anthropic">Anthropic-compatible (Messages)</option>
            </select>
          </Field>
          <Field
            label="Chat API base URL"
            hint="Any compatible HTTP(S) server, including OpenRouter, Ollama, vLLM, llama.cpp, LM Studio, or a private proxy."
          >
            <Input
              value={settings.chatBaseUrl}
              disabled={settings.localOnly}
              onChange={(event) => update("chatBaseUrl", event.currentTarget.value)}
              placeholder={
                settings.chatProtocol === "anthropic"
                  ? "https://api.anthropic.com"
                  : "http://localhost:11434/v1"
              }
            />
          </Field>
          <Field label="Extraction API protocol" hint="Inherit uses the Chat server and key.">
            <select
              value={settings.extractionProtocol}
              disabled={settings.localOnly || !settings.entityExtractionEnabled}
              onChange={(event) => {
                const protocol = event.currentTarget
                  .value as SettingsSnapshot["extractionProtocol"];
                setSettings((current) =>
                  current
                    ? {
                        ...current,
                        extractionProtocol: protocol,
                        extractionBaseUrl:
                          protocol === "inherit"
                            ? ""
                            : protocol === "anthropic"
                              ? "https://api.anthropic.com"
                              : "https://api.openai.com/v1",
                      }
                    : current,
                );
              }}
              className="h-10 w-full rounded-xl border border-border/70 bg-background/70 px-3 text-sm shadow-none"
            >
              <option value="inherit">Inherit Chat provider</option>
              <option value="openai">Separate OpenAI-compatible server</option>
              <option value="anthropic">Separate Anthropic-compatible server</option>
            </select>
          </Field>
          {settings.extractionProtocol !== "inherit" && (
            <Field label="Extraction API base URL">
              <Input
                value={settings.extractionBaseUrl ?? ""}
                disabled={settings.localOnly || !settings.entityExtractionEnabled}
                onChange={(event) => update("extractionBaseUrl", event.currentTarget.value || null)}
              />
            </Field>
          )}
          <Field label="Extraction model">
            <Input
              value={settings.extractionModel}
              disabled={settings.localOnly || !settings.entityExtractionEnabled}
              onChange={(event) => update("extractionModel", event.currentTarget.value)}
            />
          </Field>
          <div className="space-y-3">
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                disabled={settings.localOnly || testingProvider !== null}
                onClick={() => void testProvider("chat")}
              >
                {testingProvider === "chat" ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <PlugZap className="size-4" />
                )}
                Test Chat capabilities
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={settings.localOnly || testingProvider !== null}
                onClick={() => void testProvider("extraction")}
              >
                {testingProvider === "extraction" ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <Sparkles className="size-4" />
                )}
                Test extraction capabilities
              </Button>
            </div>
            <p className="mt-2 text-xs text-muted-foreground">
              Tests completion, streaming, required tools, and strict structured output using the
              unsaved values above.
            </p>
            {(["chat", "extraction"] as const).map((scope) => {
              const report = providerReports[scope];
              if (!report) return null;
              return (
                <div
                  key={scope}
                  className="rounded-xl border border-border/60 bg-background/60 p-3"
                >
                  <p className="mb-2 text-xs font-semibold tracking-wide text-foreground uppercase">
                    {scope} capabilities
                  </p>
                  <div className="grid gap-2 sm:grid-cols-2">
                    {report.map((capability) => (
                      <div
                        key={capability.name}
                        className="rounded-lg border border-border/50 px-3 py-2 text-xs"
                      >
                        <div className="flex items-center justify-between gap-2">
                          <span className="font-medium">{capability.name.replace(/_/g, " ")}</span>
                          <span className={capability.ok ? "text-emerald-600" : "text-destructive"}>
                            {capability.ok ? "supported" : "failed"} · {capability.latencyMs} ms
                          </span>
                        </div>
                        <p className="mt-1 line-clamp-3 text-muted-foreground">
                          {capability.detail}
                        </p>
                      </div>
                    ))}
                  </div>
                </div>
              );
            })}
          </div>
        </SettingsSection>

        <SettingsSection
          title="Window"
          description="Choose the native window frame or a borderless notes-rs frame."
        >
          <Field label="Decoration mode">
            <select
              value={settings.windowDecorationMode}
              onChange={(event) =>
                update(
                  "windowDecorationMode",
                  event.currentTarget.value as SettingsSnapshot["windowDecorationMode"],
                )
              }
              className="h-10 w-full rounded-xl border border-border/70 bg-background/70 px-3 text-sm shadow-none"
            >
              <option value="native">Native (compositor decorations)</option>
              <option value="borderless">Borderless (notes-rs controls)</option>
            </select>
          </Field>
          <p className="text-xs text-muted-foreground">
            Both modes apply immediately. On KDE Plasma (Wayland) the native mode uses KWin&apos;s
            server-side decorations.
          </p>
        </SettingsSection>

        <SettingsSection
          title="Embeddings"
          description="Used for semantic and hybrid search. Changing provider, model, or dimensions rebuilds the vector index on restart."
        >
          <Field label="Provider">
            <select
              value={settings.embeddingProvider}
              disabled={settings.localOnly}
              onChange={(event) => {
                const provider = event.currentTarget.value as EmbeddingProvider;
                const defaults: Record<EmbeddingProvider, [string, number]> = {
                  openrouter: ["qwen/qwen3-embedding-8b", 4096],
                  openai: ["text-embedding-3-small", 1536],
                  cohere: ["embed-multilingual-v3.0", 1024],
                  voyageai: ["voyage-3-large", 1024],
                  gemini: ["gemini-embedding-2", 3072],
                  local: ["bge-m3", 1024],
                };
                const [model, dimensions] = defaults[provider];
                setSettings((current) =>
                  current
                    ? {
                        ...current,
                        embeddingProvider: provider,
                        embeddingModel: model,
                        embeddingNdims: dimensions,
                      }
                    : current,
                );
              }}
              className="h-10 w-full rounded-xl border border-border/70 bg-background/70 px-3 text-sm shadow-none"
            >
              <option value="openrouter">OpenRouter</option>
              <option value="openai">OpenAI</option>
              <option value="cohere">Cohere</option>
              <option value="voyageai">Voyage AI</option>
              <option value="gemini">Gemini</option>
              <option value="local" disabled={!settings.localModelsAvailable}>
                Local BGE-M3{settings.localModelsAvailable ? "" : " (not included in this build)"}
              </option>
            </select>
          </Field>
          <Field label="Model">
            <Input
              value={settings.embeddingModel}
              onChange={(event) => update("embeddingModel", event.currentTarget.value)}
              placeholder="provider model id"
            />
          </Field>
          <Field
            label="Dimensions"
            hint="Required for unknown OpenRouter models; leave empty when the provider reports it."
          >
            <Input
              inputMode="numeric"
              value={settings.embeddingNdims ?? ""}
              onChange={(event) => {
                const value = event.currentTarget.value;
                update("embeddingNdims", value ? Number(value) : null);
              }}
              placeholder="4096"
            />
          </Field>
          {settings.embeddingProvider === "openai" && (
            <Field
              label="OpenAI-compatible embeddings base URL"
              hint="The complete API base before /embeddings; local no-auth servers are supported."
            >
              <Input
                value={settings.openaiBaseUrl}
                onChange={(event) => update("openaiBaseUrl", event.currentTarget.value)}
                placeholder="http://localhost:11434/v1"
              />
            </Field>
          )}
        </SettingsSection>

        <SettingsSection
          title="Reranking"
          description="Reranks hybrid candidates for agentic search."
        >
          <Field label="Provider">
            <select
              value={settings.rerankProvider}
              disabled={settings.localOnly}
              onChange={(event) =>
                update("rerankProvider", event.currentTarget.value as RerankProvider)
              }
              className="h-10 w-full rounded-xl border border-border/70 bg-background/70 px-3 text-sm shadow-none"
            >
              <option value="openrouter">OpenRouter</option>
              <option value="local" disabled={!settings.localModelsAvailable}>
                Local BGE reranker
                {settings.localModelsAvailable ? "" : " (not included in this build)"}
              </option>
            </select>
          </Field>
          <Field label="Model">
            <Input
              value={settings.rerankModel}
              onChange={(event) => update("rerankModel", event.currentTarget.value)}
            />
          </Field>
          <Field
            label="OpenRouter base URL"
            hint="Used only by OpenRouter embeddings and reranking. Chat has its own protocol-neutral endpoint above."
          >
            <Input
              value={settings.openrouterBaseUrl}
              onChange={(event) => update("openrouterBaseUrl", event.currentTarget.value)}
            />
          </Field>
        </SettingsSection>

        <SettingsSection
          title="Realtime sync"
          description="Keep this workspace synchronized through your self-hosted notes-rs server. Changes apply after restart."
        >
          <Field
            label="Server URL"
            hint="Leave empty to keep this device standalone. Use the public HTTPS origin without an API path."
          >
            <Input
              value={settings.syncServerUrl ?? ""}
              onChange={(event) => update("syncServerUrl", event.currentTarget.value || null)}
              placeholder="https://notes.okhsunrog.ru"
            />
          </Field>
          <Field
            label="Device token"
            hint={
              settings.configuredKeys.includes("SYNC_TOKEN") && !clearKeys.includes("SYNC_TOKEN")
                ? "A token is configured; leave empty to keep it."
                : "Paste the bearer token assigned to this device."
            }
          >
            <div className="flex gap-2">
              <Input
                type="password"
                autoComplete="off"
                value={secrets.SYNC_TOKEN ?? ""}
                placeholder={
                  settings.configuredKeys.includes("SYNC_TOKEN") &&
                  !clearKeys.includes("SYNC_TOKEN")
                    ? "configured"
                    : "not configured"
                }
                onChange={(event) => {
                  const value = event.currentTarget.value;
                  setSecrets((current) => ({ ...current, SYNC_TOKEN: value }));
                  if (value)
                    setClearKeys((current) => current.filter((item) => item !== "SYNC_TOKEN"));
                }}
              />
              {settings.configuredKeys.includes("SYNC_TOKEN") &&
                !clearKeys.includes("SYNC_TOKEN") && (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    aria-label="Clear sync token"
                    onClick={() =>
                      setClearKeys((current) =>
                        current.includes("SYNC_TOKEN") ? current : [...current, "SYNC_TOKEN"],
                      )
                    }
                  >
                    <Trash2 className="size-4" />
                  </Button>
                )}
            </div>
          </Field>
          <div className="rounded-xl border border-border/60 bg-background/60 p-3 text-xs">
            <div className="flex items-center justify-between gap-3">
              <span className="font-medium">Current state</span>
              <span
                className={cn(
                  syncQuery.data?.state === "online"
                    ? "text-emerald-600"
                    : syncQuery.data?.state === "error"
                      ? "text-destructive"
                      : "text-muted-foreground",
                )}
              >
                {(syncQuery.data?.state ?? "disabled").replace(/_/g, " ")}
              </span>
            </div>
            {syncQuery.data && (
              <p className="mt-1 text-muted-foreground">
                seq {syncQuery.data.lastServerSeq} · {syncQuery.data.pendingOperations} pending
                {syncQuery.data.message ? ` · ${syncQuery.data.message}` : ""}
              </p>
            )}
          </div>
        </SettingsSection>

        <SettingsSection
          title="API keys"
          description="Secrets are written to the app-data .env with owner-only permissions and are never returned to the webview."
        >
          {API_KEYS.map(([key, label]) => {
            const configured = settings.configuredKeys.includes(key) && !clearKeys.includes(key);
            return (
              <Field
                key={key}
                label={label}
                hint={
                  configured ? "A key is currently configured; leave empty to keep it." : undefined
                }
              >
                <div className="flex gap-2">
                  <Input
                    type="password"
                    autoComplete="off"
                    value={secrets[key] ?? ""}
                    placeholder={configured ? "configured" : "not configured"}
                    onChange={(event) => {
                      const value = event.currentTarget.value;
                      setSecrets((current) => ({ ...current, [key]: value }));
                      if (value) setClearKeys((current) => current.filter((item) => item !== key));
                    }}
                  />
                  {configured && (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      aria-label={`Clear ${label}`}
                      onClick={() => setClearKeys((current) => [...new Set([...current, key])])}
                    >
                      <Trash2 className="size-4" />
                    </Button>
                  )}
                </div>
              </Field>
            );
          })}
          <p className="text-xs break-all text-muted-foreground">
            Config file: {settings.configPath}
          </p>
        </SettingsSection>

        <DataSettingsSections
          settings={settings}
          background={background}
          dataAvailable={dataAvailable}
          busy={busy}
          updateSyncDirectory={(directory) => update("syncDirectory", directory)}
          dataAction={dataAction}
          chooseSync={chooseSync}
          runSync={runSync}
          backgroundAction={backgroundAction}
        />

        {error && (
          <p
            role="alert"
            className="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
          >
            {error}
          </p>
        )}
        {message && (
          <p className="flex items-center gap-2 text-sm text-emerald-600">
            <Check className="size-4" />
            {message}
          </p>
        )}

        <div className="flex flex-wrap gap-2 border-t pt-5">
          <Button type="submit" disabled={busy}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />}
            Save settings
          </Button>
          <Button type="button" variant="outline" onClick={() => void restartApp()}>
            <RotateCcw className="size-4" />
            Restart app
          </Button>
        </div>
      </form>
    </div>
  );
}

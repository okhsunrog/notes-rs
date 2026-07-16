import { useEffect, useState } from "react";
import { useTheme } from "next-themes";
import {
  ArrowLeft,
  Check,
  DatabaseBackup,
  Download,
  FolderOpen,
  Loader2,
  Pause,
  Play,
  RotateCcw,
  Save,
  Trash2,
  Upload,
  Activity,
  AppWindow,
  BrainCircuit,
  Database,
  FolderSync,
  KeyRound,
  Laptop,
  Moon,
  Palette,
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
  loadSettings,
  restartApp,
  retryBackgroundJobs,
  saveSettings,
  setBackgroundPaused,
  syncPull,
  syncPush,
  type SettingsSnapshot,
  type BackgroundStatus,
} from "@/lib/api";

type Props = {
  onBack: () => void;
  onDecorationModeChanged: (mode: "native" | "borderless" | "kde") => void;
  dataAvailable: boolean;
  onDataChanged: () => void;
};

const API_KEYS = [
  ["OPENROUTER_API_KEY", "OpenRouter API key"],
  ["OPENAI_API_KEY", "OpenAI API key"],
  ["COHERE_API_KEY", "Cohere API key"],
  ["VOYAGE_API_KEY", "Voyage AI API key"],
  ["GEMINI_API_KEY", "Gemini API key"],
] as const;

export function SettingsPage({
  onBack,
  onDecorationModeChanged,
  dataAvailable,
  onDataChanged,
}: Props) {
  const { theme, setTheme } = useTheme();
  const { palette, setPalette } = useAppearance();
  const [settings, setSettings] = useState<SettingsSnapshot | null>(null);
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const [clearKeys, setClearKeys] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [background, setBackground] = useState<BackgroundStatus | null>(null);

  useEffect(() => {
    loadSettings()
      .then(setSettings)
      .catch((reason) => setError(String(reason)));
  }, []);

  useEffect(() => {
    if (!dataAvailable) return;
    let active = true;
    const refresh = () =>
      getBackgroundStatus()
        .then((status) => active && setBackground(status))
        .catch(() => undefined);
    void refresh();
    const timer = window.setInterval(refresh, 1500);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [dataAvailable]);

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
      const saved = await saveSettings({
        localOnly: settings.localOnly,
        entityExtractionEnabled: settings.entityExtractionEnabled,
        queryRewritingEnabled: settings.queryRewritingEnabled,
        chatModel: settings.chatModel,
        extractionModel: settings.extractionModel,
        embeddingProvider: settings.embeddingProvider,
        embeddingModel: settings.embeddingModel,
        embeddingNdims: settings.embeddingNdims,
        rerankProvider: settings.rerankProvider,
        rerankModel: settings.rerankModel,
        openrouterBaseUrl: settings.openrouterBaseUrl,
        windowDecorationMode: settings.windowDecorationMode,
        syncDirectory: settings.syncDirectory,
        apiKeys: secrets,
        clearKeys,
      });
      setSettings(saved);
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
      await saveSettings({
        localOnly: current.localOnly,
        entityExtractionEnabled: current.entityExtractionEnabled,
        queryRewritingEnabled: current.queryRewritingEnabled,
        chatModel: current.chatModel,
        extractionModel: current.extractionModel,
        embeddingProvider: current.embeddingProvider,
        embeddingModel: current.embeddingModel,
        embeddingNdims: current.embeddingNdims,
        rerankProvider: current.rerankProvider,
        rerankModel: current.rerankModel,
        openrouterBaseUrl: current.openrouterBaseUrl,
        windowDecorationMode: current.windowDecorationMode,
        syncDirectory: current.syncDirectory,
        apiKeys: {},
        clearKeys: [],
      });
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
      setBackground(await getBackgroundStatus());
    } catch (reason) {
      setError(String(reason));
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
                            embeddingNdims: "1024",
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
            description="Sends changed note text to the configured OpenRouter endpoint to build entity and relation edges."
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
          <Field label="Extraction model">
            <Input
              value={settings.extractionModel}
              disabled={settings.localOnly || !settings.entityExtractionEnabled}
              onChange={(event) => update("extractionModel", event.currentTarget.value)}
            />
          </Field>
        </SettingsSection>

        <SettingsSection
          title="Window"
          description="Choose the native GTK frame, a borderless notes-rs frame, or KWin's server-side decoration on native Wayland."
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
              <option value="native">Native Wayland (GTK header bar)</option>
              <option value="borderless">Borderless (notes-rs controls)</option>
              <option value="kde" disabled={!settings.kdeDecorationsAvailable}>
                KDE/KWin server decoration (native Wayland)
                {settings.kdeDecorationsAvailable ? "" : " (Wayland unavailable)"}
              </option>
            </select>
          </Field>
          <p className="text-xs text-muted-foreground">
            Borderless applies immediately. Switching to or from KDE/KWin requires an app restart
            because GTK negotiates its decoration strategy before creating the Wayland surface.
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
                const provider = event.currentTarget.value;
                const defaults: Record<string, [string, string]> = {
                  openrouter: ["qwen/qwen3-embedding-8b", "4096"],
                  openai: ["text-embedding-3-small", "1536"],
                  cohere: ["embed-multilingual-v3.0", "1024"],
                  voyageai: ["voyage-3-large", "1024"],
                  gemini: ["gemini-embedding-2", "3072"],
                  local: ["bge-m3", "1024"],
                };
                const [model, dimensions] = defaults[provider] ?? ["", ""];
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
              value={settings.embeddingNdims}
              onChange={(event) => update("embeddingNdims", event.currentTarget.value)}
              placeholder="4096"
            />
          </Field>
        </SettingsSection>

        <SettingsSection
          title="Reranking"
          description="Reranks hybrid candidates for agentic search."
        >
          <Field label="Provider">
            <select
              value={settings.rerankProvider}
              disabled={settings.localOnly}
              onChange={(event) => update("rerankProvider", event.currentTarget.value)}
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
            hint="Used consistently by Chat, extraction, embeddings, and reranking."
          >
            <Input
              value={settings.openrouterBaseUrl}
              onChange={(event) => update("openrouterBaseUrl", event.currentTarget.value)}
            />
          </Field>
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

        <SettingsSection
          title="Data"
          description="Portable JSON archives include every note, graph edge, and attachment. Destructive operations automatically create a timestamped recovery backup in app data."
        >
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={!dataAvailable || busy}
              onClick={() => void dataAction("export")}
            >
              <Download className="size-4" /> Export archive
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={!dataAvailable || busy}
              onClick={() => void dataAction("import")}
            >
              <Upload className="size-4" /> Import archive
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={!dataAvailable || busy}
              onClick={() => void dataAction("backup")}
            >
              <DatabaseBackup className="size-4" /> Create backup
            </Button>
          </div>
          {!dataAvailable && (
            <p className="text-xs text-muted-foreground">
              Data tools become available after the database starts successfully.
            </p>
          )}
        </SettingsSection>

        <SettingsSection
          title="Folder sync"
          description="Manual, conflict-safe sync through a folder managed by Syncthing, Nextcloud, Dropbox, or another file synchronizer. Push writes one atomic snapshot; pull always creates a local recovery backup first."
        >
          <Field label="Sync directory">
            <div className="flex gap-2">
              <Input
                value={settings.syncDirectory}
                onChange={(event) => update("syncDirectory", event.currentTarget.value)}
                placeholder="Choose a directory…"
              />
              <Button
                type="button"
                variant="outline"
                aria-label="Choose sync directory"
                onClick={() => void chooseSync()}
              >
                <FolderOpen className="size-4" />
              </Button>
            </div>
          </Field>
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={!dataAvailable || busy || !settings.syncDirectory.trim()}
              onClick={() => void runSync("push")}
            >
              <Upload className="size-4" /> Push snapshot
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={!dataAvailable || busy || !settings.syncDirectory.trim()}
              onClick={() => void runSync("pull")}
            >
              <Download className="size-4" /> Pull snapshot
            </Button>
          </div>
        </SettingsSection>

        <SettingsSection
          title="Background indexing"
          description="Embedding and entity-extraction queues run after saves. Failed jobs use exponential backoff instead of retrying continuously."
        >
          {background ? (
            <>
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                <QueueMetric label="Embeddings" value={background.embeddingsPending} />
                <QueueMetric label="Embedding retries" value={background.embeddingsFailed} />
                <QueueMetric label="Extractions" value={background.extractionsPending} />
                <QueueMetric label="Extraction retries" value={background.extractionsFailed} />
              </div>
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void backgroundAction("pause")}
                >
                  {background.paused ? <Play className="size-4" /> : <Pause className="size-4" />}
                  {background.paused ? "Resume" : "Pause"}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void backgroundAction("retry")}
                >
                  <RotateCcw className="size-4" /> Retry failed
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void backgroundAction("clear")}
                >
                  <Trash2 className="size-4" /> Cancel pending
                </Button>
              </div>
            </>
          ) : (
            <p className="text-xs text-muted-foreground">
              Indexing status is available after startup.
            </p>
          )}
        </SettingsSection>

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

function SettingsSection({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  const Icon =
    {
      Appearance: Palette,
      Window: AppWindow,
      "AI & privacy": BrainCircuit,
      Embeddings: BrainCircuit,
      Reranking: BrainCircuit,
      "API keys": KeyRound,
      Data: Database,
      "Folder sync": FolderSync,
      "Background indexing": Activity,
    }[title] ?? Palette;
  return (
    <section className="space-y-5 rounded-3xl border border-border/60 bg-card/70 p-5 shadow-sm backdrop-blur sm:p-6">
      <div className="flex gap-3">
        <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
          <Icon className="size-3.5" />
        </span>
        <div>
          <h2 className="font-semibold tracking-tight">{title}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{description}</p>
        </div>
      </div>
      <div className="grid gap-4">{children}</div>
    </section>
  );
}

function ToggleField({
  checked,
  disabled,
  label,
  description,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  description: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label
      className={cn(
        "flex items-start justify-between gap-4 rounded-2xl border border-border/60 bg-background/55 p-4",
        disabled && "opacity-55",
      )}
    >
      <span>
        <span className="block text-sm font-medium">{label}</span>
        <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
          {description}
        </span>
      </span>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.currentTarget.checked)}
        className="mt-1 size-4 shrink-0 accent-primary"
      />
    </label>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="grid gap-1.5 text-sm">
      <span className="font-medium">{label}</span>
      {children}
      {hint && <span className="text-xs text-muted-foreground">{hint}</span>}
    </label>
  );
}

function FieldGroup({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="grid gap-1.5 text-sm">
      <span className="font-medium">{label}</span>
      {children}
      {hint && <span className="text-xs text-muted-foreground">{hint}</span>}
    </div>
  );
}

function QueueMetric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-xl border border-border/60 bg-background/70 p-3">
      <div className="text-lg font-semibold tabular-nums">{value}</div>
      <div className="text-xs text-muted-foreground">{label}</div>
    </div>
  );
}

function ModeButton({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "flex h-10 items-center justify-center gap-2 rounded-xl text-xs font-medium transition",
        active
          ? "bg-background text-foreground shadow-sm"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
      {icon}
      {label}
    </button>
  );
}

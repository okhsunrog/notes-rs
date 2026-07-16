import { useEffect, useState } from "react";
import {
  ArrowLeft,
  Check,
  DatabaseBackup,
  Download,
  FolderOpen,
  Loader2,
  RotateCcw,
  Save,
  Trash2,
  Upload,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { WindowControls } from "@/app/window-controls";
import {
  createBackup,
  chooseSyncDirectory,
  exportData,
  importData,
  loadSettings,
  restartApp,
  saveSettings,
  syncPull,
  syncPush,
  type SettingsSnapshot,
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
  const [settings, setSettings] = useState<SettingsSnapshot | null>(null);
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const [clearKeys, setClearKeys] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    loadSettings()
      .then(setSettings)
      .catch((reason) => setError(String(reason)));
  }, []);

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
    <div className="h-screen overflow-y-auto bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="sticky top-0 z-10 flex h-14 items-center justify-between border-b bg-background/95 px-5 backdrop-blur"
      >
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="sm" onClick={onBack} aria-label="Back to notes">
            <ArrowLeft className="size-4" />
          </Button>
          <div>
            <h1 className="font-semibold">Settings</h1>
            <p className="text-xs text-muted-foreground">
              Models, providers, and local credentials
            </p>
          </div>
        </div>
        {settings.windowDecorationMode === "borderless" && <WindowControls />}
      </header>

      <form onSubmit={submit} className="mx-auto max-w-3xl space-y-8 p-6">
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
              className="h-9 w-full rounded-md border bg-background px-3 text-sm"
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
            because GTK chooses Wayland or X11 before creating the window.
          </p>
        </SettingsSection>

        <SettingsSection
          title="Embeddings"
          description="Used for semantic and hybrid search. Changing provider, model, or dimensions rebuilds the vector index on restart."
        >
          <Field label="Provider">
            <select
              value={settings.embeddingProvider}
              onChange={(event) => update("embeddingProvider", event.currentTarget.value)}
              className="h-9 w-full rounded-md border bg-background px-3 text-sm"
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
              onChange={(event) => update("rerankProvider", event.currentTarget.value)}
              className="h-9 w-full rounded-md border bg-background px-3 text-sm"
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
          <Field label="OpenRouter base URL">
            <Input
              value={settings.openrouterBaseUrl}
              onChange={(event) => update("openrouterBaseUrl", event.currentTarget.value)}
            />
          </Field>
        </SettingsSection>

        <SettingsSection
          title="API keys"
          description="Secrets are written to the app-data .env with owner-only permissions and are never returned to the webview. OpenRouter is also used by Chat and entity extraction."
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
          description="Portable JSON archives include every note and graph edge. Destructive operations automatically create a timestamped recovery backup in app data."
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
  return (
    <section className="space-y-4 rounded-lg border bg-card p-5">
      <div>
        <h2 className="font-semibold">{title}</h2>
        <p className="mt-1 text-sm text-muted-foreground">{description}</p>
      </div>
      <div className="grid gap-4">{children}</div>
    </section>
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

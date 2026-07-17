import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTheme } from "next-themes";
import {
  ArrowLeft,
  Check,
  Laptop,
  Loader2,
  Moon,
  RotateCcw,
  Save,
  Sparkles,
  Sun,
  Trash2,
} from "lucide-react";
import { WindowControls } from "@/app/window-controls";
import { PALETTES, useAppearance } from "@/app/appearance";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  chooseSyncDirectory,
  createBackup,
  exportData,
  getSyncStatus,
  importData,
  loadSettings,
  restartApp,
  saveSettings,
  syncPull,
  syncPush,
  type SecretKey,
  type SettingsSnapshot,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import { DataSettingsSections } from "./data-settings-sections";
import { Field, FieldGroup, ModeButton, SettingsSection } from "./settings-controls";
import { toSettingsUpdate } from "./settings-update";

type Props = {
  onBack: () => void;
  onDecorationModeChanged: (mode: "native" | "borderless") => void;
  dataAvailable: boolean;
  onDataChanged: () => void;
};

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

  const settingsQuery = useQuery({ queryKey: queryKeys.settings, queryFn: loadSettings });
  const syncQuery = useQuery({ queryKey: queryKeys.syncStatus, queryFn: getSyncStatus });

  useEffect(() => {
    if (settingsQuery.data) setSettings(settingsQuery.data);
  }, [settingsQuery.data]);

  useEffect(() => {
    if (settingsQuery.error) setError(String(settingsQuery.error));
  }, [settingsQuery.error]);

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
      setMessage("Saved. Restart the app to apply server connection changes.");
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
      const saved = await saveSettings(toSettingsUpdate(current, secrets, clearKeys));
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

  if (!settings) {
    return (
      <div className="app-shell flex h-full items-center justify-center text-foreground">
        {error ? (
          <p className="text-sm text-destructive">{error}</p>
        ) : (
          <Loader2 className="animate-spin" />
        )}
      </div>
    );
  }

  const tokenConfigured =
    settings.configuredKeys.includes("SYNC_TOKEN") && !clearKeys.includes("SYNC_TOKEN");

  return (
    <div className="app-shell h-full overflow-y-auto text-foreground">
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
            <p className="text-xs text-muted-foreground">Device, appearance, and server</p>
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
            On KDE Plasma Wayland, native mode uses KWin server-side decorations.
          </p>
        </SettingsSection>

        <SettingsSection
          title="Notes server"
          description="Realtime sync and every AI feature are owned by your notes-rs server. Local editing, graph navigation, and full-text search remain available offline."
        >
          <Field
            label="Server URL"
            hint="Leave empty for a standalone offline device. Use the public HTTPS origin without an API path."
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
              tokenConfigured
                ? "A token is configured; leave empty to keep it."
                : "Paste the bearer token assigned to this device."
            }
          >
            <div className="flex gap-2">
              <Input
                type="password"
                autoComplete="off"
                value={secrets.SYNC_TOKEN ?? ""}
                placeholder={tokenConfigured ? "configured" : "not configured"}
                onChange={(event) => {
                  const value = event.currentTarget.value;
                  setSecrets((current) => ({ ...current, SYNC_TOKEN: value }));
                  if (value)
                    setClearKeys((current) => current.filter((item) => item !== "SYNC_TOKEN"));
                }}
              />
              {tokenConfigured && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  aria-label="Clear sync token"
                  onClick={() => setClearKeys(["SYNC_TOKEN"])}
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
          <p className="text-xs break-all text-muted-foreground">
            Device config: {settings.configPath}
          </p>
        </SettingsSection>

        <DataSettingsSections
          settings={settings}
          dataAvailable={dataAvailable}
          busy={busy}
          updateSyncDirectory={(directory) => update("syncDirectory", directory)}
          dataAction={dataAction}
          chooseSync={chooseSync}
          runSync={runSync}
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

        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border/50 pt-5">
          <Button type="button" variant="ghost" onClick={() => restartApp()}>
            <RotateCcw className="size-4" /> Restart app
          </Button>
          <Button type="submit" disabled={busy}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />}
            Save settings
          </Button>
        </div>
      </form>
    </div>
  );
}

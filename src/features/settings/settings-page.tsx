import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { onBackButtonPress } from "@tauri-apps/api/app";
import { TangleafMark } from "@/brand/TangleafMark";
import "@/brand/brand.css";
import { SettingsSelect } from "./settings-select";
import { dismissBackOverlay } from "@/lib/back-overlays";
import { useTheme } from "next-themes";
import { ArrowLeft, Check, Laptop, Moon, RotateCcw, Save, Sun, Trash2 } from "lucide-react";
import { WindowControls } from "@/app/window-controls";
import { useCompactLayout } from "@/app/use-compact-layout";
import { PALETTES, useAppearance } from "@/app/appearance";
import { DISPLAY_PROFILE_OPTIONS, INK_COLOR_OPTIONS } from "@/app/display-profile";
import { useConfirmation } from "@/app/confirmation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  createBackup,
  exportData,
  getSyncStatus,
  importData,
  listPages,
  loadSettings,
  resetSettings,
  restartApp,
  retrySync,
  saveSettings,
  type SecretKey,
  type SettingsSnapshot,
  type WindowDecorationMode,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import { presentSyncStatus } from "@/features/sync/sync-status-presentation";
import { DataSettingsSections } from "./data-settings-sections";
import { HandwritingPreferenceField } from "@/features/handwriting/handwriting-preference";
import { ServerAiSettingsSection } from "./server-ai-settings-section";
import { Field, FieldGroup, ModeButton, SettingsSection, ToggleField } from "./settings-controls";
import { toSettingsUpdate } from "./settings-update";
import { ConfigurationTransferSection } from "./configuration-transfer-section";
import { BusyIndicator } from "@/components/ui/busy-indicator";

type Props = {
  onBack: () => void;
  onDecorationModeChanged: (mode: WindowDecorationMode) => void;
  dataAvailable: boolean;
  onDataChanged: (openPageUuid?: string | null) => void;
};

export function SettingsPage({
  onBack,
  onDecorationModeChanged,
  dataAvailable,
  onDataChanged,
}: Props) {
  const compact = useCompactLayout();
  const confirm = useConfirmation();
  const { theme, setTheme } = useTheme();
  const {
    palette,
    setPalette,
    displayProfile,
    setDisplayProfile,
    inkColor,
    setInkColor,
    display,
    panelMode,
  } = useAppearance();
  const queryClient = useQueryClient();
  const [settings, setSettings] = useState<SettingsSnapshot | null>(null);
  const [secrets, setSecrets] = useState<Partial<Record<SecretKey, string>>>({});
  const [clearKeys, setClearKeys] = useState<SecretKey[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    if (!compact || !navigator.userAgent.toLocaleLowerCase().includes("android")) return;
    let disposed = false;
    let unlisten: (() => Promise<void>) | undefined;
    void onBackButtonPress(() => {
      if (!dismissBackOverlay()) onBack();
    })
      .then((listener) => {
        if (disposed) void listener.unregister();
        else unlisten = () => listener.unregister();
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      void unlisten?.();
    };
  }, [compact, onBack]);

  const settingsQuery = useQuery({ queryKey: queryKeys.settings, queryFn: loadSettings });
  const syncQuery = useQuery({ queryKey: queryKeys.syncStatus, queryFn: getSyncStatus });
  const syncPresentation = syncQuery.data ? presentSyncStatus(syncQuery.data) : null;
  const startupPagesQuery = useQuery({
    queryKey: [...queryKeys.pages, "settings-startup"],
    queryFn: () => listPages({ filter: "notes", limit: 10_000 }),
  });

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
      onDecorationModeChanged(saved.activeWindowDecorationMode);
      setSecrets({});
      setClearKeys([]);
      setMessage("Saved. Startup and server connection changes apply on the next app launch.");
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
      !(await confirm({
        title: "Import archive?",
        description:
          "The selected archive will replace the current local workspace. A recovery backup is created first.",
        confirmLabel: "Choose archive",
        destructive: true,
      }))
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

  async function resetInvalidSettings() {
    if (
      !(await confirm({
        title: "Reset device settings?",
        description:
          "The invalid settings file will be replaced with current defaults. Local notes are not affected, but server credentials must be entered again.",
        confirmLabel: "Reset settings",
        destructive: true,
      }))
    )
      return;
    setBusy(true);
    try {
      const restored = await resetSettings();
      setSettings(restored);
      queryClient.setQueryData(queryKeys.settings, restored);
      onDecorationModeChanged(restored.activeWindowDecorationMode);
      setError("");
      setMessage("Device settings reset. Restart the app after configuring the server.");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function retrySyncConnection() {
    setBusy(true);
    setError("");
    try {
      await retrySync();
      setMessage("Sync retry requested.");
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
          <div className="mx-5 max-w-lg rounded-2xl border border-destructive/30 surface-card-strong p-6 shadow-popover">
            <h1 className="font-semibold">Device settings could not be loaded</h1>
            <p className="mt-2 text-sm break-words text-destructive">{error}</p>
            <div className="mt-5 flex flex-wrap gap-2">
              <Button type="button" variant="outline" onClick={onBack}>
                <ArrowLeft className="size-4" /> Back
              </Button>
              <Button
                type="button"
                variant="destructive"
                disabled={busy}
                onClick={() => void resetInvalidSettings()}
              >
                {busy ? (
                  <BusyIndicator className="size-4" label="Working" hideLabel />
                ) : (
                  <RotateCcw className="size-4" />
                )}
                Reset device settings
              </Button>
            </div>
          </div>
        ) : (
          <BusyIndicator label="Loading device settings…" />
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
        className={cn(
          "app-chrome sticky top-0 z-20 flex items-center justify-between border-b border-border/50",
          compact ? "h-14 px-3" : "h-16 px-5",
        )}
      >
        <div className={cn("flex items-center", compact ? "gap-2" : "gap-3")}>
          <Button
            variant="ghost"
            size={compact ? "icon-lg" : "icon-sm"}
            className="rounded-xl"
            onClick={onBack}
            aria-label="Back to notes"
          >
            <ArrowLeft className={compact ? "size-5" : "size-4"} />
          </Button>
          {!compact && <TangleafMark width={32} height={32} />}
          <div>
            <h1 className={cn("font-semibold tracking-tight", compact && "text-base")}>Settings</h1>
            {!compact && (
              <p className="text-xs text-muted-foreground">Device, appearance, and server</p>
            )}
          </div>
        </div>
        {settings.capabilities.windowDecorations &&
          settings.activeWindowDecorationMode === "borderless" && <WindowControls />}
      </header>

      <form
        onSubmit={submit}
        className="mx-auto max-w-4xl space-y-7 px-4 py-5 pb-[calc(6rem+var(--safe-area-inset-bottom))] sm:p-10"
      >
        <ConfigurationTransferSection
          appearance={{
            theme: theme === "light" || theme === "dark" ? theme : "system",
            palette,
            displayProfile,
            inkColor,
          }}
          disabled={busy}
          onBusyChange={setBusy}
          onImported={(result) => {
            setSettings(result.settings);
            queryClient.setQueryData(queryKeys.settings, result.settings);
            setSecrets({});
            setClearKeys([]);
            if (result.appearance) {
              setTheme(result.appearance.theme);
              setPalette(result.appearance.palette);
              setDisplayProfile(result.appearance.displayProfile ?? "auto");
              setInkColor(result.appearance.inkColor ?? "auto");
            }
          }}
        />
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
                      : "border-border/60 surface-base hover:border-primary/25",
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
          <Field
            label="Display"
            hint="E-ink drops shadows, blur and animation and raises contrast. Auto follows the panel the device reports."
          >
            <SettingsSelect
              label="Display"
              value={displayProfile}
              options={[...DISPLAY_PROFILE_OPTIONS]}
              onValueChange={setDisplayProfile}
            />
            {panelMode && (
              <p className="mt-2 text-xs text-muted-foreground">Panel mode: {panelMode}</p>
            )}
          </Field>
          <Field
            label="E-ink color"
            hint={
              display === "eink"
                ? "Monochrome removes the palette accent from buttons and handwriting."
                : "Available while the e-ink display profile is in effect."
            }
          >
            <SettingsSelect
              label="E-ink color"
              value={inkColor}
              disabled={display !== "eink"}
              options={[...INK_COLOR_OPTIONS]}
              onValueChange={setInkColor}
            />
          </Field>
        </SettingsSection>

        {settings.capabilities.windowDecorations && (
          <SettingsSection
            title="Window"
            description="Choose the native window frame or a borderless Tangleaf frame."
          >
            <Field label="Decoration mode">
              <SettingsSelect
                label="Decoration mode"
                value={settings.windowDecorationMode}
                onValueChange={(value) => update("windowDecorationMode", value)}
                options={[
                  { value: "native", label: "Native (system decorations)" },
                  { value: "borderless", label: "Borderless (Tangleaf controls)" },
                ]}
              />
            </Field>
            <p className="text-xs text-muted-foreground">
              Native mode uses system decorations. On Wayland, save your choice and restart the app
              to change the frame; the current window keeps its existing controls.
            </p>
            <Field label="Borderless corner radius">
              <SettingsSelect
                label="Borderless corner radius"
                value={settings.windowCornerRadius}
                disabled={
                  !settings.capabilities.windowCornerRounding ||
                  settings.windowDecorationMode !== "borderless"
                }
                onValueChange={(value) => update("windowCornerRadius", value)}
                options={[0, 6, 10, 16, 24].map((radius) => ({
                  value: radius,
                  label:
                    radius === 0 ? "Square" : `${radius} px${radius === 10 ? " (default)" : ""}`,
                }))}
              />
            </Field>
            <p className="text-xs text-muted-foreground">
              Applies after saving on Linux and Windows, only in borderless mode. Maximized and
              fullscreen windows stay square.
            </p>
            {settings.windowDecorationsRequireRestart &&
              settingsQuery.data?.windowDecorationMode !== settings.activeWindowDecorationMode && (
                <p role="status" className="text-sm text-muted-foreground">
                  Saved window frame change will apply after restarting the app.
                </p>
              )}
          </SettingsSection>
        )}

        <HandwritingPreferenceField />

        <SettingsSection
          title="Startup"
          description="Choose what this device shows when Tangleaf opens. Dashboard is the calm default; your notes are never changed by this choice."
        >
          <Field label="Open on launch">
            <SettingsSelect<SettingsSnapshot["startupView"]>
              label="Open on launch"
              value={settings.startupView}
              onValueChange={(startupView) => {
                setSettings((current) =>
                  current
                    ? {
                        ...current,
                        startupView,
                        startupPageUuid:
                          startupView === "specific_page"
                            ? (current.startupPageUuid ?? startupPagesQuery.data?.[0]?.uuid ?? null)
                            : current.startupPageUuid,
                      }
                    : current,
                );
                setMessage("");
              }}
              options={[
                { value: "dashboard", label: "Dashboard" },
                { value: "last_session", label: "Restore last session" },
                { value: "today", label: "Today's journal" },
                { value: "specific_page", label: "A specific note" },
              ]}
            />
          </Field>
          {settings.startupView === "specific_page" && (
            <Field
              label="Startup note"
              hint={
                startupPagesQuery.data?.length
                  ? "If this note is deleted, Tangleaf safely falls back to Dashboard."
                  : "Create a note before choosing it as your startup page."
              }
            >
              <SettingsSelect
                label="Startup note"
                value={settings.startupPageUuid}
                disabled={!startupPagesQuery.data?.length}
                placeholder={
                  startupPagesQuery.data?.length ? "Choose a note" : "No notes available"
                }
                onValueChange={(value) => update("startupPageUuid", value)}
                options={(startupPagesQuery.data ?? []).map((page) => ({
                  value: page.uuid,
                  label: page.title ?? "Untitled note",
                }))}
              />
            </Field>
          )}
          <p className="text-xs leading-relaxed text-muted-foreground">
            Favorites, recent notes, and the restored session stay on this device.
          </p>
        </SettingsSection>

        <SettingsSection
          title="Search"
          description="Control when server AI joins the always-available local full-text search."
        >
          <ToggleField
            checked={settings.aiSearchEnabled}
            label="AI search"
            description="Allow the configured notes server to refine local search results. Local FTS stays enabled when this is off."
            onChange={(checked) => update("aiSearchEnabled", checked)}
          />
          <Field
            label="AI trigger"
            hint="Enter-only avoids server requests while you are still typing. Press Enter once to request AI results, then again to open the selection."
          >
            <SettingsSelect
              label="AI trigger"
              value={settings.aiSearchTrigger}
              disabled={!settings.aiSearchEnabled}
              onValueChange={(value) => update("aiSearchTrigger", value)}
              options={[
                { value: "as_you_type", label: "As you type" },
                { value: "enter_only", label: "Enter only" },
              ]}
            />
          </Field>
          <ToggleField
            checked={settings.aiSearchRerank}
            disabled={!settings.aiSearchEnabled}
            label="Reranker"
            description="Applies when AI search runs on Enter. When disabled, the server keeps its original RRF order."
            onChange={(checked) => update("aiSearchRerank", checked)}
          />
          <details className="rounded-2xl border border-border/60 surface-base-soft p-4">
            <summary className="cursor-pointer text-sm font-medium">Advanced</summary>
            <div className="mt-3">
              <ToggleField
                checked={settings.searchDebugSources}
                label="Per-result source badges"
                description="Show local FTS and server AI labels on every result for ranking diagnostics."
                onChange={(checked) => update("searchDebugSources", checked)}
              />
            </div>
          </details>
        </SettingsSection>

        <SettingsSection
          title="Notes server"
          description="Realtime sync and every AI feature are owned by your Tangleaf server. Local editing, graph navigation, and full-text search remain available offline."
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
          <div className="rounded-xl border border-border/60 surface-base p-3 text-xs">
            <div className="flex items-center justify-between gap-3">
              <span className="font-medium">Current state</span>
              <span
                className={cn(
                  syncPresentation?.tone === "success"
                    ? "text-emerald-600"
                    : syncPresentation?.tone === "danger"
                      ? "text-destructive"
                      : syncPresentation?.tone === "warning"
                        ? "text-amber-600 dark:text-amber-400"
                        : "text-muted-foreground",
                )}
              >
                {syncPresentation?.label ?? "Sync off"}
              </span>
            </div>
            {syncQuery.data && (
              <div className="mt-1 space-y-1 text-muted-foreground">
                <p>{syncPresentation?.description}</p>
                <p>
                  seq {syncQuery.data.lastServerSeq} · {syncQuery.data.pendingOperations} pending
                </p>
                {syncQuery.data.message && (
                  <p className="break-words text-destructive">{syncQuery.data.message}</p>
                )}
              </div>
            )}
            {syncPresentation?.canRetry && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="mt-3"
                disabled={busy}
                onClick={() => void retrySyncConnection()}
              >
                Retry sync
              </Button>
            )}
          </div>
          <p className="text-xs break-all text-muted-foreground">
            Device config: {settings.configPath}
          </p>
        </SettingsSection>

        <ServerAiSettingsSection
          enabled={Boolean(settings.syncServerUrl?.trim() && tokenConfigured)}
          onError={setError}
          onMessage={setMessage}
        />

        <DataSettingsSections
          dataAvailable={dataAvailable}
          busy={busy}
          dataAction={dataAction}
          onDataChanged={onDataChanged}
          onError={setError}
          onMessage={setMessage}
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
          <p role="status" className="flex items-center gap-2 text-sm text-emerald-600">
            <Check className="size-4" />
            {message}
          </p>
        )}

        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border/50 pt-5">
          <Button type="button" variant="ghost" onClick={() => restartApp()}>
            <RotateCcw className="size-4" /> Restart app
          </Button>
          <Button type="submit" disabled={busy}>
            {busy ? (
              <BusyIndicator className="size-4" label="Working" hideLabel />
            ) : (
              <Save className="size-4" />
            )}
            Save settings
          </Button>
        </div>
      </form>
    </div>
  );
}

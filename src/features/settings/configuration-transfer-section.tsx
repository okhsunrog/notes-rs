import { useEffect, useState } from "react";
import { Download, Upload, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  applyConfigurationImport,
  cancelConfigurationImport,
  exportDeviceConfiguration,
  previewConfigurationImport,
  restartApp,
  type ConfigurationAppearance,
  type ConfigurationImportResult,
  type ConfigurationPreview,
} from "@/lib/api";
import { Field, SettingsSection, ToggleField } from "./settings-controls";

type Props = {
  appearance: ConfigurationAppearance;
  disabled: boolean;
  onBusyChange: (busy: boolean) => void;
  onImported: (result: ConfigurationImportResult) => void;
};

export function ConfigurationTransferSection({
  appearance,
  disabled,
  onBusyChange,
  onImported,
}: Props) {
  const [includeToken, setIncludeToken] = useState(false);
  const [includeAppearance, setIncludeAppearance] = useState(false);
  const [preview, setPreview] = useState<ConfigurationPreview | null>(null);
  const [token, setToken] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [restartRequired, setRestartRequired] = useState(false);

  useEffect(() => {
    onBusyChange(loading || preview !== null);
  }, [loading, preview, onBusyChange]);
  const previewId = preview?.id;
  useEffect(
    () => () => {
      if (previewId) void cancelConfigurationImport(previewId).catch(() => undefined);
    },
    [previewId],
  );

  async function chooseFile() {
    setLoading(true);
    setError("");
    setMessage("");
    setToken("");
    try {
      setPreview(await previewConfigurationImport());
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  async function exportFile() {
    setLoading(true);
    setError("");
    setMessage("");
    try {
      if (await exportDeviceConfiguration(includeToken, includeAppearance ? appearance : null)) {
        setMessage(
          includeToken
            ? "Configuration exported with a device token. Keep the file private and delete it after transfer."
            : "Configuration exported without a token.",
        );
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  async function apply() {
    if (!preview) return;
    setLoading(true);
    setError("");
    try {
      const result = await applyConfigurationImport(preview.id, token || null);
      onImported(result);
      setRestartRequired(result.restartRequired);
      setMessage(
        !result.settings.syncServerUrl
          ? "Configuration imported. Sync is disabled."
          : result.connectionVerified
            ? "Configuration imported. Server connection verified."
            : "Configuration imported. Could not verify the connection; check the server and token, or retry when online.",
      );
      setPreview(null);
      setToken("");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  return (
    <SettingsSection
      title="Configuration transfer"
      description="Set up another device with a JSON file. Exports saved server and AI-search settings; your notes and server AI-provider keys stay where they are."
    >
      <ToggleField
        checked={includeToken}
        disabled={disabled}
        onChange={setIncludeToken}
        label="Include device token"
        description="Anyone with this file can access your server. Tokens are omitted by default."
      />
      <ToggleField
        checked={includeAppearance}
        disabled={disabled}
        onChange={setIncludeAppearance}
        label="Include appearance"
        description="Transfer the current brightness mode, color palette and display profile."
      />
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={disabled}
          onClick={() => void exportFile()}
        >
          <Download className="size-4" />
          Export configuration
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={disabled}
          onClick={() => void chooseFile()}
        >
          <Upload className="size-4" />
          Import configuration
        </Button>
      </div>
      {preview && (
        <div
          className="space-y-4 rounded-2xl border border-primary/30 bg-background/70 p-4"
          aria-label="Configuration import preview"
        >
          <h3 className="font-semibold">Import preview</h3>
          <dl className="grid gap-2 text-sm">
            <div>
              <dt className="text-muted-foreground">Server</dt>
              <dd className="break-all">{preview.serverUrl ?? "Offline — disable sync"}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Device token</dt>
              <dd>
                {preview.tokenIncluded
                  ? "Included in file (hidden)"
                  : preview.tokenRequired
                    ? "Required for this server"
                    : preview.serverUrl
                      ? "Keep the token already saved for this server"
                      : "Remove saved token"}
              </dd>
            </div>
            <div>
              <dt className="text-muted-foreground">AI search</dt>
              <dd>
                {preview.search.enabled ? "Enabled" : "Disabled"} ·{" "}
                {preview.search.trigger === "as_you_type" ? "As you type" : "On Enter"} · reranker{" "}
                {preview.search.rerank ? "on" : "off"}
              </dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Appearance</dt>
              <dd>
                {preview.appearance
                  ? `${preview.appearance.theme} · ${preview.appearance.palette} · ${
                      preview.appearance.displayProfile ?? "auto"
                    } display · ${preview.appearance.inkColor ?? "auto"} ink`
                  : "Keep this device's appearance"}
              </dd>
            </div>
          </dl>
          <p className="text-xs text-muted-foreground">
            These settings replace the saved connection and AI-search preferences. Local notes are
            preserved. Connecting may download notes and upload pending changes to the selected
            server.
          </p>
          {preview.tokenRequired && (
            <Field label="Device token for imported server">
              <Input
                type="password"
                autoComplete="off"
                value={token}
                onChange={(event) => setToken(event.currentTarget.value)}
              />
            </Field>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              disabled={loading || (preview.tokenRequired && !token.trim())}
              onClick={() => void apply()}
            >
              Apply configuration
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={loading}
              onClick={() => {
                setPreview(null);
                setToken("");
              }}
            >
              Cancel import
            </Button>
          </div>
        </div>
      )}
      {loading && (
        <p role="status" className="text-sm text-muted-foreground">
          Working…
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {message && (
        <p role="status" className="text-sm">
          {message}
        </p>
      )}
      {restartRequired && (
        <div className="space-y-2">
          <p className="text-sm text-muted-foreground">
            Restart to use this connection for sync and AI.
          </p>
          <Button type="button" variant="outline" onClick={() => void restartApp()}>
            <RotateCcw className="size-4" />
            Restart app
          </Button>
        </div>
      )}
    </SettingsSection>
  );
}

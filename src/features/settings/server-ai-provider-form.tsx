import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, FlaskConical, Loader2, Save, XCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  probeServerAiProvider,
  saveServerAiProvider,
  type AiIndexStatus,
  type AiProviderSettings,
  type AiProviderSettingsUpdate,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import { Field } from "./settings-controls";

type Props = {
  provider: AiProviderSettings;
  onError: (message: string) => void;
  onMessage: (message: string) => void;
};

function editableSettings(provider: AiProviderSettings): AiProviderSettingsUpdate {
  return {
    retrievalBaseUrl: provider.retrievalBaseUrl,
    retrievalApiKey: null,
    embeddingModel: provider.embeddingModel,
    embeddingDimensions: provider.embeddingDimensions,
    rerankModel: provider.rerankModel,
    completionProtocol: provider.completionProtocol,
    completionBaseUrl: provider.completionBaseUrl,
    completionApiKey: null,
    chatModel: provider.chatModel,
    extractionModel: provider.extractionModel,
  };
}

export function ServerAiProviderForm({ provider, onError, onMessage }: Props) {
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState(() => editableSettings(provider));
  const [probeResult, setProbeResult] = useState<Awaited<
    ReturnType<typeof probeServerAiProvider>
  > | null>(null);

  const applyStatus = (status: AiIndexStatus) => {
    queryClient.setQueryData(queryKeys.serverAi, status);
  };
  const saveMutation = useMutation({
    mutationFn: saveServerAiProvider,
    onSuccess: (status) => {
      applyStatus(status);
      onMessage("Server AI provider configuration updated.");
    },
    onError: (error) => onError(String(error)),
  });
  const probeMutation = useMutation({
    mutationFn: probeServerAiProvider,
    onSuccess: (result) => {
      setProbeResult(result);
      const healthy = Object.values(result).every((check) => check.ok);
      onMessage(
        healthy ? "Every AI provider check passed." : "Provider probe completed with errors.",
      );
    },
    onError: (error) => onError(String(error)),
  });
  const busy = saveMutation.isPending || probeMutation.isPending;
  const set = <Key extends keyof AiProviderSettingsUpdate>(
    key: Key,
    value: AiProviderSettingsUpdate[Key],
  ) => {
    setDraft((current) => ({ ...current, [key]: value }));
    setProbeResult(null);
  };
  const submit = (action: "save" | "probe") => {
    onError("");
    onMessage("");
    if (action === "save") saveMutation.mutate(draft);
    else probeMutation.mutate(draft);
  };

  return (
    <div className="grid gap-4 rounded-2xl border border-border/60 bg-background/55 p-4">
      <div>
        <h3 className="text-sm font-semibold">Provider configuration</h3>
        <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
          Retrieval uses OpenRouter embeddings and reranking. Chat and extraction can use any
          OpenAI- or Anthropic-compatible endpoint. Secrets are write-only and stay on the server.
        </p>
      </div>

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Retrieval API URL">
          <Input
            value={draft.retrievalBaseUrl}
            disabled={busy}
            onChange={(event) => set("retrievalBaseUrl", event.currentTarget.value)}
          />
        </Field>
        <Field
          label="Retrieval API key"
          hint={
            provider.retrievalApiKeyConfigured
              ? "Configured; leave blank to keep the stored secret."
              : "No retrieval secret is configured."
          }
        >
          <Input
            type="password"
            autoComplete="new-password"
            value={draft.retrievalApiKey ?? ""}
            disabled={busy}
            placeholder={provider.retrievalApiKeyConfigured ? "configured" : "required"}
            onChange={(event) => set("retrievalApiKey", event.currentTarget.value || null)}
          />
        </Field>
        <Field label="Embedding model">
          <Input
            value={draft.embeddingModel}
            disabled={busy}
            onChange={(event) => set("embeddingModel", event.currentTarget.value)}
          />
        </Field>
        <Field label="Embedding dimensions">
          <Input
            type="number"
            min={1}
            step={1}
            value={draft.embeddingDimensions || ""}
            disabled={busy}
            onChange={(event) =>
              set("embeddingDimensions", Number.parseInt(event.currentTarget.value, 10) || 0)
            }
          />
        </Field>
        <Field label="Rerank model">
          <Input
            value={draft.rerankModel}
            disabled={busy}
            onChange={(event) => set("rerankModel", event.currentTarget.value)}
          />
        </Field>
      </div>

      <div className="h-px bg-border/60" />

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Completion protocol">
          <select
            value={draft.completionProtocol}
            disabled={busy}
            onChange={(event) =>
              set("completionProtocol", event.currentTarget.value as "openai" | "anthropic")
            }
            className="h-9 w-full rounded-md border border-input bg-transparent px-3 text-sm dark:bg-input/30"
          >
            <option value="openai">OpenAI-compatible</option>
            <option value="anthropic">Anthropic-compatible</option>
          </select>
        </Field>
        <Field label="Completion API URL">
          <Input
            value={draft.completionBaseUrl}
            disabled={busy}
            onChange={(event) => set("completionBaseUrl", event.currentTarget.value)}
          />
        </Field>
        <Field
          label="Completion API key"
          hint={
            provider.completionApiKeyConfigured
              ? "Configured; leave blank to keep the stored secret."
              : "No completion secret is configured."
          }
        >
          <Input
            type="password"
            autoComplete="new-password"
            value={draft.completionApiKey ?? ""}
            disabled={busy}
            placeholder={provider.completionApiKeyConfigured ? "configured" : "required"}
            onChange={(event) => set("completionApiKey", event.currentTarget.value || null)}
          />
        </Field>
        <Field label="Chat model">
          <Input
            value={draft.chatModel}
            disabled={busy}
            onChange={(event) => set("chatModel", event.currentTarget.value)}
          />
        </Field>
        <Field label="Extraction model">
          <Input
            value={draft.extractionModel}
            disabled={busy}
            onChange={(event) => set("extractionModel", event.currentTarget.value)}
          />
        </Field>
      </div>

      {probeResult && (
        <div className="grid gap-2 sm:grid-cols-2">
          {Object.entries(probeResult).map(([name, check]) => (
            <div
              key={name}
              className={cn(
                "flex gap-2 rounded-xl border p-3 text-xs",
                check.ok
                  ? "border-emerald-500/25 bg-emerald-500/5 text-emerald-700"
                  : "border-destructive/25 bg-destructive/5 text-destructive",
              )}
            >
              {check.ok ? (
                <CheckCircle2 className="mt-0.5 size-3.5 shrink-0" />
              ) : (
                <XCircle className="mt-0.5 size-3.5 shrink-0" />
              )}
              <span>
                <strong className="capitalize">{name}:</strong> {check.message}
              </span>
            </div>
          ))}
        </div>
      )}

      <p className="text-xs text-muted-foreground">
        Changing the embedding endpoint, model, or dimensions creates a fresh generation. The
        previous active vectors remain isolated and are never mixed with incompatible embeddings.
      </p>
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" disabled={busy} onClick={() => submit("probe")}>
          {probeMutation.isPending ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <FlaskConical className="size-4" />
          )}
          Test configuration
        </Button>
        <Button type="button" disabled={busy} onClick={() => submit("save")}>
          {saveMutation.isPending ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Save className="size-4" />
          )}
          Save on server
        </Button>
      </div>
    </div>
  );
}

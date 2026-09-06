import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { BrainCircuit, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  getServerAiStatus,
  reindexServerAi,
  saveServerAiSettings,
  type AiRuntimeSettings,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import { QueueMetric, SettingsSection, ToggleField } from "./settings-controls";
import { ServerAiProviderForm } from "./server-ai-provider-form";
import { BusyIndicator } from "@/components/ui/busy-indicator";

type Props = {
  enabled: boolean;
  onError: (message: string) => void;
  onMessage: (message: string) => void;
};

export function ServerAiSettingsSection({ enabled, onError, onMessage }: Props) {
  const queryClient = useQueryClient();
  const statusQuery = useQuery({
    queryKey: queryKeys.serverAi,
    queryFn: getServerAiStatus,
    enabled,
    refetchInterval: enabled ? 2_000 : false,
  });
  const saveMutation = useMutation({
    mutationFn: saveServerAiSettings,
    onSuccess: (status) => {
      queryClient.setQueryData(queryKeys.serverAi, status);
      onMessage("Server AI settings updated.");
    },
    onError: (error) => onError(String(error)),
  });
  const reindexMutation = useMutation({
    mutationFn: reindexServerAi,
    onSuccess: (status) => {
      queryClient.setQueryData(queryKeys.serverAi, status);
      onMessage("A fresh embedding index is being built.");
    },
    onError: (error) => onError(String(error)),
  });
  const status = statusQuery.data;
  const busy = saveMutation.isPending || reindexMutation.isPending;
  const update = (patch: Partial<AiRuntimeSettings>) => {
    if (!status) return;
    onError("");
    onMessage("");
    saveMutation.mutate({ ...status.settings, ...patch });
  };
  const progress = status?.sourceDocuments
    ? Math.min(100, Math.round((status.indexedDocuments / status.sourceDocuments) * 100))
    : status?.generationState === "active"
      ? 100
      : 0;

  return (
    <SettingsSection
      title="Server AI"
      description="Inspect and control the server-owned retrieval index. Provider credentials and vectors are never stored on this device."
    >
      {!enabled ? (
        <p className="rounded-2xl border border-border/60 surface-base p-4 text-sm text-muted-foreground">
          Configure the notes server and restart the app to manage its AI runtime.
        </p>
      ) : statusQuery.isPending ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <BusyIndicator className="size-4" label="Reading server index state" hideLabel /> Reading
          server index state…
        </div>
      ) : statusQuery.error ? (
        <p className="rounded-2xl border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
          {String(statusQuery.error)}
        </p>
      ) : status ? (
        <>
          <div className="rounded-2xl border border-border/60 surface-base p-4">
            <div className="flex items-center justify-between gap-3">
              <span className="flex items-center gap-2 text-sm font-medium">
                <BrainCircuit className="size-4 text-primary" /> Embedding index
              </span>
              <span
                className={cn(
                  "rounded-full px-2 py-1 text-[11px] font-semibold uppercase tracking-wide",
                  status.generationState === "active"
                    ? "bg-emerald-500/10 text-emerald-600"
                    : "bg-amber-500/10 text-amber-600",
                )}
              >
                {status.generationState}
              </span>
            </div>
            <div className="mt-4 h-2 overflow-hidden rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-primary transition-[width] duration-500"
                style={{ width: `${progress}%` }}
              />
            </div>
            <div className="mt-2 flex justify-between text-xs text-muted-foreground">
              <span>{progress}% indexed</span>
              <span>
                {status.indexedDocuments} / {status.sourceDocuments} documents
              </span>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            <QueueMetric label="Embedding queue" value={status.pendingEmbeddings} />
            <QueueMetric label="Embedding failures" value={status.failedEmbeddings} />
            <QueueMetric label="Extraction queue" value={status.pendingExtractions} />
            <QueueMetric label="Extraction failures" value={status.failedExtractions} />
          </div>

          <ToggleField
            checked={status.settings.automaticEmbeddings}
            disabled={busy}
            label="Automatic embeddings"
            description="Continuously index synced note changes on the server. Disabling this keeps the current active index readable."
            onChange={(automaticEmbeddings) => update({ automaticEmbeddings })}
          />
          <ToggleField
            checked={status.settings.entityExtraction}
            disabled={busy}
            label="Entity extraction"
            description="Let the server derive concepts and relations and sync them back as ordinary graph operations."
            onChange={(entityExtraction) => update({ entityExtraction })}
          />
          <ToggleField
            checked={status.settings.queryRewriting}
            disabled={busy}
            label="Context-aware query rewriting"
            description="Allow chat retrieval to rewrite contextual questions before searching your notes."
            onChange={(queryRewriting) => update({ queryRewriting })}
          />

          <ServerAiProviderForm
            key={JSON.stringify(status.provider)}
            provider={status.provider}
            onError={onError}
            onMessage={onMessage}
          />

          <div className="grid gap-1 rounded-2xl border border-border/60 surface-base p-4 text-xs text-muted-foreground sm:grid-cols-2">
            <span>Embedding: {status.provider.embeddingModel}</span>
            <span>Dimensions: {status.provider.embeddingDimensions}</span>
            <span>Reranker: {status.provider.rerankModel}</span>
            <span>Chat: {status.provider.chatModel}</span>
            <span>Extraction: {status.provider.extractionModel}</span>
            <span className="truncate" title={status.generationId}>
              Generation: {status.generationId}
            </span>
          </div>

          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => reindexMutation.mutate()}
          >
            {reindexMutation.isPending ? (
              <BusyIndicator className="size-4" label="Rebuilding index" hideLabel />
            ) : (
              <RefreshCw className="size-4" />
            )}
            Rebuild embedding index
          </Button>
        </>
      ) : null}
    </SettingsSection>
  );
}

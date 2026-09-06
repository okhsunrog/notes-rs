import { useQuery } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  FileInput,
  Image,
  RefreshCw,
  X,
} from "lucide-react";
import { useConfirmation } from "@/app/confirmation";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  getLogseqImportCapability,
  type ImportDiagnostic,
  type LogseqImportAvailability,
  type LogseqImportBlocker,
  type LogseqImportProgress,
} from "@/lib/api";
import { cn } from "@/lib/utils";
import { type LogseqImportState } from "./logseq-import-state";
import { useLogseqImport } from "./use-logseq-import";
import { BusyIndicator } from "@/components/ui/busy-indicator";

const capabilityQueryKey = ["backend", "logseq-import-capability"] as const;

const blockerLabels: Record<LogseqImportBlocker, string> = {
  empty_import: "The selected graph does not contain importable pages.",
  non_empty_workspace: "Logseq import is only available for an empty local workspace.",
  blocking_diagnostics: "Resolve the blocking diagnostics before importing.",
  alias_collision: "The graph contains aliases that resolve to different pages.",
  existing_import_changed: "This graph changed after its previous import receipt was created.",
  background_ai_enabled: "Pause automatic server AI indexing before importing.",
  server_ai_status_unavailable: "The server AI state could not be verified safely.",
};

const stageLabels: Record<LogseqImportProgress["stage"], string> = {
  scanning: "Scanning graph",
  parsing: "Parsing Markdown",
  preparing: "Building import plan",
  verifying: "Verifying source",
  materializing: "Preparing workspace",
  installing: "Installing media",
  committing: "Committing notes",
  complete: "Complete",
};

type ViewProps = {
  state: LogseqImportState;
  disabled?: boolean;
  hasMoreDiagnostics: boolean;
  onChoose: () => void;
  onCommit: () => void;
  onClose: () => void;
  onOpenResult: () => void;
  onLoadMoreDiagnostics: () => void;
};

type Props = {
  disabled?: boolean;
  onDataChanged: (openPageUuid: string | null) => void;
  onError: (message: string) => void;
  onMessage: (message: string) => void;
};

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-xl border border-border/55 surface-base px-3 py-2.5">
      <div className="text-lg font-semibold tabular-nums">{value.toLocaleString()}</div>
      <div className="text-[11px] text-muted-foreground">{label}</div>
    </div>
  );
}

function ImportProgress({ progress }: { progress: LogseqImportProgress | null }) {
  const percentage =
    progress?.progress === "items" && progress.total > 0
      ? Math.min(100, Math.round((progress.completed / progress.total) * 100))
      : null;
  return (
    <div role="status" aria-live="polite" className="space-y-2 text-sm text-muted-foreground">
      <div className="flex items-center gap-2">
        <BusyIndicator className="size-4 text-primary" label="Importing" hideLabel />
        <span>{progress ? stageLabels[progress.stage] : "Waiting for importer…"}</span>
        {progress?.progress === "items" && (
          <span className="ml-auto tabular-nums">
            {progress.completed.toLocaleString()} / {progress.total.toLocaleString()}
          </span>
        )}
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          className={cn(
            "h-full rounded-full bg-primary transition-[width]",
            percentage === null && "w-1/3 animate-pulse eink:animate-none",
          )}
          style={percentage === null ? undefined : { width: `${percentage}%` }}
        />
      </div>
    </div>
  );
}

function DiagnosticItem({ diagnostic }: { diagnostic: ImportDiagnostic }) {
  const location = diagnostic.relativePath
    ? `${diagnostic.relativePath}${diagnostic.range ? `:${diagnostic.range.start.line}:${diagnostic.range.start.column}` : ""}`
    : null;
  return (
    <li className="rounded-xl border border-border/50 surface-base p-3 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <Badge
          variant={diagnostic.severity === "error" ? "destructive" : "outline"}
          className={cn(diagnostic.severity === "warning" && "border-amber-500/40 text-amber-700")}
        >
          {diagnostic.severity}
        </Badge>
        <code className="text-[11px] text-muted-foreground">{diagnostic.code}</code>
        {location && <span className="min-w-0 truncate text-muted-foreground">{location}</span>}
      </div>
      <p className="mt-2 leading-relaxed">{diagnostic.message}</p>
      {diagnostic.remediation && (
        <p className="mt-1.5 leading-relaxed text-muted-foreground">{diagnostic.remediation}</p>
      )}
    </li>
  );
}

function Preview({ state, hasMoreDiagnostics, onLoadMoreDiagnostics }: ViewProps) {
  if (state.phase !== "preview" && state.phase !== "committing") return null;
  const { preview, diagnostics } = state;
  const report = preview.report;
  const preservedDrawingLabel =
    report.preservedExcalidrawCount === 1 ? "drawing reference" : "drawing references";

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="truncate font-medium" title={preview.sourceName}>
            {preview.sourceName}
          </p>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {preview.destination === "empty"
              ? "New import into an empty workspace"
              : preview.destination === "existing_receipt"
                ? "Previously imported graph"
                : "Existing workspace detected"}
          </p>
        </div>
        <Badge variant={preview.canCommit ? "secondary" : "destructive"}>
          {preview.canCommit ? "Ready to import" : "Import blocked"}
        </Badge>
      </div>

      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        <Metric label="Pages" value={report.pageCount} />
        <Metric label="Blocks" value={report.blockCount} />
        <Metric label="References" value={report.referenceCount} />
        <Metric label="Media refs" value={report.mediaReferenceCount} />
      </div>

      <div className="grid gap-2 rounded-xl border border-border/55 surface-base p-3 text-xs text-muted-foreground sm:grid-cols-2">
        <span>{report.journalCount.toLocaleString()} journals</span>
        <span>{report.taskCount.toLocaleString()} tasks</span>
        <span>
          {report.resolvedReferenceCount.toLocaleString()} resolved ·{" "}
          {report.unresolvedReferenceCount.toLocaleString()} unresolved references
        </span>
        <span>
          {report.localMediaReferenceCount.toLocaleString()} local ·{" "}
          {report.inlineMediaReferenceCount.toLocaleString()} inline media
        </span>
        <span>
          {report.missingMediaReferenceCount.toLocaleString()} missing ·{" "}
          {report.blockedRemoteMediaReferenceCount.toLocaleString()} remote blocked
        </span>
        <span>
          {report.blockedUnsafeMediaReferenceCount.toLocaleString()} unsafe ·{" "}
          {report.unsupportedMediaReferenceCount.toLocaleString()} unsupported media
        </span>
        <span>
          {report.unreferencedAssetCount.toLocaleString()} unreferenced assets ·{" "}
          {report.unreferencedDrawingCount.toLocaleString()} drawings
        </span>
        <span>{report.preservedBlockUuidCount.toLocaleString()} block UUIDs preserved</span>
      </div>

      {report.preparedExcalidrawCount + report.preservedExcalidrawCount > 0 && (
        <div
          className={cn(
            "flex items-start gap-3 rounded-xl border p-3 text-sm",
            report.drawingConversionState !== "prepared" || report.preservedExcalidrawCount > 0
              ? "border-amber-500/35 bg-amber-500/5"
              : "border-border/55 surface-base",
          )}
        >
          <Image className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <div>
            <p className="font-medium">
              {report.drawingConversionState === "prepared"
                ? `${report.preparedExcalidrawCount.toLocaleString()} Excalidraw drawings prepared as PNG`
                : report.drawingConversionState === "invalid"
                  ? "Drawing conversion publication ignored"
                  : `${report.preservedExcalidrawCount.toLocaleString()} Excalidraw drawings preserved as source`}
            </p>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              {report.drawingConversionState === "prepared"
                ? `${report.preservedExcalidrawCount.toLocaleString()} ${preservedDrawingLabel} will keep the original source syntax. PNG artifacts are verified again during commit.`
                : report.drawingConversionState === "invalid"
                  ? "The conversion publication is invalid and will be ignored; original drawing references remain recoverable."
                  : "No matching conversion publication is available; original drawing references remain recoverable."}
            </p>
          </div>
        </div>
      )}

      {preview.blockers.length > 0 && (
        <div role="alert" className="rounded-xl border border-destructive/35 bg-destructive/5 p-3">
          <div className="flex items-center gap-2 text-sm font-medium text-destructive">
            <AlertTriangle className="size-4" /> Import cannot continue
          </div>
          <ul className="mt-2 list-disc space-y-1 pl-5 text-xs text-destructive">
            {preview.blockers.map((blocker) => (
              <li key={blocker}>{blockerLabels[blocker]}</li>
            ))}
          </ul>
        </div>
      )}

      {report.diagnosticCount > 0 && (
        <div>
          <div className="mb-2 flex items-center justify-between gap-3">
            <h4 className="text-sm font-medium">
              Diagnostics ({diagnostics.length.toLocaleString()} of{" "}
              {state.diagnosticsTotal.toLocaleString()})
            </h4>
            <span className="text-xs text-muted-foreground">
              {report.blockingDiagnosticCount} blocking · {report.warningCount} warnings
            </span>
          </div>
          <ul className="max-h-72 space-y-2 overflow-y-auto pr-1">
            {diagnostics.map((diagnostic, index) => (
              <DiagnosticItem
                key={`${diagnostic.code}-${diagnostic.relativePath ?? "graph"}-${index}`}
                diagnostic={diagnostic}
              />
            ))}
          </ul>
          {hasMoreDiagnostics && state.phase === "preview" && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="mt-2"
              disabled={state.diagnosticsLoading || state.discarding}
              onClick={onLoadMoreDiagnostics}
            >
              {state.diagnosticsLoading ? (
                <BusyIndicator className="size-4" label="Working" hideLabel />
              ) : (
                <ChevronDown className="size-4" />
              )}
              Load more diagnostics
            </Button>
          )}
        </div>
      )}

      {state.error && (
        <p
          role="alert"
          className="rounded-xl border border-destructive/35 bg-destructive/5 p-3 text-sm text-destructive"
        >
          {state.error}
        </p>
      )}
      {state.phase === "committing" && <ImportProgress progress={state.progress} />}
    </div>
  );
}

export function LogseqImportSectionView(props: ViewProps) {
  const { state, disabled, onChoose, onCommit, onClose, onOpenResult } = props;
  const busy = state.phase === "preparing" || state.phase === "committing";
  const previewBusy = state.phase === "preview" && state.discarding;

  return (
    <div className="space-y-4 border-t border-border/55 pt-5" data-testid="logseq-import">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex min-w-0 gap-3">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
            <FileInput className="size-4" />
          </span>
          <div>
            <h3 className="text-sm font-semibold">Import a Logseq graph</h3>
            <p className="mt-1 max-w-2xl text-xs leading-relaxed text-muted-foreground">
              Choose a graph directory for a loss-aware dry run. Nothing is written until you review
              the report and explicitly confirm it.
            </p>
          </div>
        </div>
        {state.phase !== "idle" && state.phase !== "committing" && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label="Close Logseq import"
            disabled={previewBusy}
            onClick={onClose}
          >
            {previewBusy ? <BusyIndicator label="Closing" hideLabel /> : <X />}
          </Button>
        )}
      </div>

      {state.phase === "idle" && (
        <div>
          <Button type="button" variant="outline" disabled={disabled} onClick={onChoose}>
            <FileInput className="size-4" /> Choose Logseq graph
          </Button>
          {state.error && (
            <p role="alert" className="mt-3 text-sm text-destructive">
              {state.error}
            </p>
          )}
        </div>
      )}

      {state.phase === "preparing" && <ImportProgress progress={state.progress} />}

      {(state.phase === "preview" || state.phase === "committing") && (
        <>
          <Preview {...props} />
          <div className="flex flex-wrap justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={busy || previewBusy}
              onClick={onChoose}
            >
              <RefreshCw className="size-4" /> Choose another graph
            </Button>
            <Button
              type="button"
              disabled={
                busy || previewBusy || !state.preview.canCommit || state.preview.blockers.length > 0
              }
              onClick={onCommit}
            >
              {state.phase === "committing" ? (
                <BusyIndicator className="size-4" label="Working" hideLabel />
              ) : (
                <FileInput className="size-4" />
              )}
              Import graph
            </Button>
          </div>
        </>
      )}

      {state.phase === "result" && (
        <div role="status" className="rounded-xl border border-emerald-500/30 bg-emerald-500/5 p-4">
          <div className="flex items-center gap-2 font-medium text-emerald-700">
            <CheckCircle2 className="size-4" />
            {state.result.status === "applied" ? "Logseq graph imported" : "Already up to date"}
          </div>
          <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
            {state.result.status === "applied"
              ? `${state.result.pageCount.toLocaleString()} pages, ${state.result.blockCount.toLocaleString()} blocks, ${state.result.aliasCount.toLocaleString()} aliases, and ${state.result.attachmentCount.toLocaleString()} attachments were committed.`
              : "The selected graph matches its existing import receipt, so no duplicate notes were created."}
          </p>
          <div className="mt-3 flex flex-wrap gap-2">
            {state.result.openPageUuid && (
              <Button type="button" size="sm" onClick={onOpenResult}>
                Open imported notes
              </Button>
            )}
            <Button type="button" variant="outline" size="sm" onClick={onChoose}>
              <RefreshCw className="size-4" /> Import another graph
            </Button>
            <Button type="button" size="sm" onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

export function LogseqImportSectionPresentation({
  availability,
  ...props
}: ViewProps & { availability: LogseqImportAvailability | undefined }) {
  if (availability?.status !== "available") return null;
  return <LogseqImportSectionView {...props} />;
}

export function LogseqImportSection({ disabled, onDataChanged, onError, onMessage }: Props) {
  const confirm = useConfirmation();
  const capabilityQuery = useQuery({
    queryKey: capabilityQueryKey,
    queryFn: getLogseqImportCapability,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const controller = useLogseqImport({ onDataChanged, onError, onMessage });
  const preview = controller.state.phase === "preview" ? controller.state.preview : undefined;

  const confirmCommit = async () => {
    if (!preview || !preview.canCommit || preview.blockers.length > 0) return;
    const confirmed = await confirm({
      title: "Import this Logseq graph?",
      description: `Commit ${preview.report.pageCount.toLocaleString()} pages and ${preview.report.blockCount.toLocaleString()} blocks from ${preview.sourceName}. This uses the reviewed dry-run plan; embeddings remain a separate server-owned process.`,
      confirmLabel: "Import graph",
    });
    if (confirmed) await controller.commit();
  };

  return (
    <LogseqImportSectionPresentation
      availability={capabilityQuery.data}
      state={controller.state}
      disabled={disabled}
      hasMoreDiagnostics={controller.hasMoreDiagnostics}
      onChoose={() => void controller.start()}
      onCommit={() => void confirmCommit()}
      onClose={() => void controller.close()}
      onOpenResult={controller.openResult}
      onLoadMoreDiagnostics={() => void controller.loadMoreDiagnostics()}
    />
  );
}

import {
  DatabaseBackup,
  Download,
  FolderOpen,
  Pause,
  Play,
  RotateCcw,
  Trash2,
  Upload,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { BackgroundStatus, SettingsSnapshot } from "@/lib/api";
import { Field, QueueMetric, SettingsSection } from "./settings-controls";

type Props = {
  settings: SettingsSnapshot;
  background: BackgroundStatus | null;
  dataAvailable: boolean;
  busy: boolean;
  updateSyncDirectory: (directory: string | null) => void;
  dataAction: (action: "export" | "import" | "backup") => Promise<void>;
  chooseSync: () => Promise<void>;
  runSync: (direction: "push" | "pull") => Promise<void>;
  backgroundAction: (action: "pause" | "retry" | "clear") => Promise<void>;
};

export function DataSettingsSections({
  settings,
  background,
  dataAvailable,
  busy,
  updateSyncDirectory,
  dataAction,
  chooseSync,
  runSync,
  backgroundAction,
}: Props) {
  return (
    <>
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
              value={settings.syncDirectory ?? ""}
              onChange={(event) => updateSyncDirectory(event.currentTarget.value || null)}
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
            disabled={!dataAvailable || busy || !settings.syncDirectory?.trim()}
            onClick={() => void runSync("push")}
          >
            <Upload className="size-4" /> Push snapshot
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={!dataAvailable || busy || !settings.syncDirectory?.trim()}
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
            {background.failures.length > 0 && (
              <div className="space-y-2">
                <p className="text-xs font-semibold tracking-wide text-foreground uppercase">
                  Recent failures
                </p>
                {background.failures.map((failure) => (
                  <div
                    key={`${failure.queue}-${failure.nodeId}`}
                    className="rounded-xl border border-destructive/20 bg-destructive/[0.035] p-3 text-xs"
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium text-foreground">
                        {failure.queue === "embedding" ? "Embedding" : "Entity extraction"}
                      </span>
                      <span className="rounded-full bg-muted px-2 py-0.5 text-muted-foreground">
                        {failure.failureKind}
                      </span>
                      <span className={failure.terminal ? "text-destructive" : "text-amber-600"}>
                        {failure.terminal ? "Needs attention" : "Retry scheduled"}
                      </span>
                      <span className="ml-auto text-muted-foreground">
                        attempt {failure.retryCount}
                      </span>
                    </div>
                    <p className="mt-1 text-muted-foreground">
                      {failure.nodeTitle || `Node #${failure.nodeId}`}
                    </p>
                    <p className="mt-2 line-clamp-4 break-all text-destructive/90">
                      {failure.lastError}
                    </p>
                  </div>
                ))}
              </div>
            )}
          </>
        ) : (
          <p className="text-xs text-muted-foreground">
            Indexing status is available after startup.
          </p>
        )}
      </SettingsSection>
    </>
  );
}

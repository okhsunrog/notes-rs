import { useState } from "react";
import { AlertTriangle, Check, Cloud, CloudOff, RefreshCw, Settings } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { retrySync, type SyncStatus } from "@/lib/api";
import { cn } from "@/lib/utils";
import { notifyError } from "@/lib/notify";
import { presentSyncStatus, type SyncStatusTone } from "./sync-status-presentation";

type Props = {
  status: SyncStatus;
  onOpenSettings: () => void;
};

const toneClasses: Record<SyncStatusTone, string> = {
  success: "border-emerald-500/25 bg-emerald-500/10 text-emerald-600",
  progress: "border-primary/25 bg-primary/10 text-primary",
  warning: "border-amber-500/25 bg-amber-500/10 text-amber-600 dark:text-amber-400",
  danger: "border-destructive/25 bg-destructive/8 text-destructive",
  muted: "border-border/60 surface-card text-muted-foreground",
};

export function SyncStatusIndicator({ status, onOpenSettings }: Props) {
  const [retrying, setRetrying] = useState(false);
  const presentation = presentSyncStatus(status);
  const Icon = statusIcon(status);

  async function reconnect() {
    setRetrying(true);
    try {
      await retrySync();
    } catch (error) {
      notifyError("sync retry", error);
    } finally {
      setRetrying(false);
    }
  }

  return (
    <Popover>
      <PopoverTrigger
        aria-label={`Sync status: ${presentation.label}`}
        className={cn(
          "mr-1 flex h-8 items-center gap-1.5 rounded-xl border px-2.5 text-xs font-medium transition-colors outline-none hover:brightness-95 focus-visible:ring-[3px] focus-visible:ring-ring/50",
          toneClasses[presentation.tone],
        )}
      >
        <Icon
          className={cn("size-3.5", presentation.animated && "animate-spin eink:animate-none")}
        />
        <span className="hidden lg:inline">{presentation.label}</span>
        {status.pendingOperations > 0 && (
          <span className="tabular-nums" aria-label={`${status.pendingOperations} pending changes`}>
            {status.pendingOperations}
          </span>
        )}
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="w-[min(21rem,calc(100vw-1.5rem))] p-0"
        initialFocus={false}
      >
        <div className="flex items-start gap-3 p-4">
          <div
            className={cn(
              "mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-xl border",
              toneClasses[presentation.tone],
            )}
          >
            <Icon
              className={cn("size-4", presentation.animated && "animate-spin eink:animate-none")}
            />
          </div>
          <div className="min-w-0">
            <h2 className="text-sm font-semibold">{presentation.label}</h2>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              {presentation.description}
            </p>
          </div>
        </div>

        {status.message && (
          <div className="mx-4 mb-3 rounded-lg border border-destructive/20 bg-destructive/5 px-3 py-2">
            <p className="text-[10px] eink:text-xs font-semibold tracking-wide text-destructive uppercase">
              Last connection error
            </p>
            <p className="mt-1 text-xs leading-relaxed break-words text-foreground/80">
              {status.message}
            </p>
          </div>
        )}

        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 border-y border-border/60 px-4 py-3 text-xs">
          <dt className="text-muted-foreground">Server</dt>
          <dd className="truncate text-right" title={status.serverUrl ?? undefined}>
            {serverLabel(status.serverUrl)}
          </dd>
          <dt className="text-muted-foreground">Local queue</dt>
          <dd className="text-right tabular-nums">
            {status.pendingOperations === 0 ? "Empty" : `${status.pendingOperations} pending`}
          </dd>
          <dt className="text-muted-foreground">Server cursor</dt>
          <dd className="text-right font-mono tabular-nums">#{status.lastServerSeq}</dd>
        </dl>

        <div className="flex items-center justify-end gap-2 p-3">
          {presentation.canRetry && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={retrying}
              onClick={() => void reconnect()}
            >
              <RefreshCw className={cn("size-3.5", retrying && "animate-spin eink:animate-none")} />
              Reconnect now
            </Button>
          )}
          <Button type="button" variant="ghost" size="sm" onClick={onOpenSettings}>
            <Settings className="size-3.5" />
            Sync settings
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function statusIcon(status: SyncStatus) {
  if (status.state === "online" && status.pendingOperations === 0) return Check;
  if (status.state === "connecting" || status.state === "syncing" || status.pendingOperations > 0)
    return RefreshCw;
  if (status.state === "offline") return CloudOff;
  if (status.state === "error" || status.state === "conflict" || status.state === "update_required")
    return AlertTriangle;
  return Cloud;
}

function serverLabel(serverUrl: string | null): string {
  if (!serverUrl) return "Not configured";
  try {
    return new URL(serverUrl).host;
  } catch {
    return serverUrl;
  }
}

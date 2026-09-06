import { useReducer } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import {
  CommandFailure,
  handwritingNoteStatus,
  previewHandwritingVersion,
  resolveHandwritingConflict,
  unknownErrorMessage,
} from "@/lib/api";
import type { InkVersionInfo } from "@/lib/bindings";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import {
  conflictReducer,
  expectedHeads,
  initialConflictState,
  keepAll,
  keepOne,
} from "./handwriting-conflict-model";
import { InkCanvas } from "./ink-canvas";

type Props = {
  pageUuid: string;
  heads: readonly InkVersionInfo[];
  onClose: () => void;
  onResolved: (keptPageUuids: string[]) => void | Promise<void>;
};

/**
 * Manual branch selection. Concurrent drawings are never merged automatically,
 * so the user compares the published versions and decides what survives.
 */
export function HandwritingConflictDialog({ pageUuid, heads, onClose, onResolved }: Props) {
  const [state, dispatch] = useReducer(conflictReducer, heads, initialConflictState);

  const resolve = async (keep: string[] | null) => {
    if (!keep || state.busy) return;
    dispatch({ type: "resolve_started" });
    try {
      const kept = await resolveHandwritingConflict(pageUuid, expectedHeads(state), keep);
      await onResolved(kept);
    } catch (error) {
      if (error instanceof CommandFailure && error.code === "conflict") {
        try {
          const status = await handwritingNoteStatus(pageUuid);
          dispatch({ type: "resolve_stale", heads: status.heads });
          return;
        } catch (refreshError) {
          dispatch({ type: "resolve_failed", message: unknownErrorMessage(refreshError) });
          return;
        }
      }
      dispatch({ type: "resolve_failed", message: unknownErrorMessage(error) });
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !state.busy) onClose();
      }}
    >
      <DialogContent
        showCloseButton={false}
        className="flex max-h-[85dvh] w-full flex-col gap-3 sm:max-w-3xl"
      >
        <div>
          <DialogTitle>Versions from other devices</DialogTitle>
          <DialogDescription className="mt-1 text-xs">
            {state.heads.length} versions of this note were written separately. Choose which one
            stays, or keep them all as separate notes.
          </DialogDescription>
        </div>
        {state.notice && (
          <p role="alert" className="text-xs text-destructive">
            {state.notice}
          </p>
        )}
        <div className="grid min-h-0 flex-1 gap-3 overflow-y-auto sm:grid-cols-2">
          {state.heads.map((head) => (
            <VersionCard
              key={head.versionUuid}
              pageUuid={pageUuid}
              head={head}
              selected={state.selectedVersionUuid === head.versionUuid}
              busy={state.busy}
              onSelect={() => dispatch({ type: "select", versionUuid: head.versionUuid })}
              onKeep={() =>
                void resolve(
                  keepOne(
                    conflictReducer(state, { type: "select", versionUuid: head.versionUuid }),
                  ),
                )
              }
            />
          ))}
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2">
          <Button type="button" variant="ghost" disabled={state.busy} onClick={onClose}>
            Later
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={state.busy || keepAll(state) === null}
            onClick={() => void resolve(keepAll(state))}
          >
            Keep all as separate notes
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function VersionCard({
  pageUuid,
  head,
  selected,
  busy,
  onSelect,
  onKeep,
}: {
  pageUuid: string;
  head: InkVersionInfo;
  selected: boolean;
  busy: boolean;
  onSelect: () => void;
  onKeep: () => void;
}) {
  const preview = useQuery({
    queryKey: queryKeys.handwritingVersion(pageUuid, head.versionUuid),
    queryFn: () => previewHandwritingVersion(pageUuid, head.versionUuid),
    enabled: head.available,
  });

  return (
    <section
      aria-label={`Version from ${head.deviceName}`}
      data-selected={selected || undefined}
      className={cn(
        "flex min-w-0 flex-col gap-2 rounded-xl border p-2",
        selected && "border-primary",
      )}
    >
      <div className="flex min-w-0 items-baseline gap-2">
        <span className="truncate text-sm font-medium">{head.deviceName}</span>
        <span className="ml-auto shrink-0 text-[11px] text-muted-foreground">
          {new Date(head.modifiedAtMs).toLocaleString()}
        </span>
      </div>
      <div className="min-h-32 overflow-hidden border border-neutral-300 bg-white">
        {!head.available ? (
          <p role="status" className="p-6 text-center text-xs text-muted-foreground">
            Not downloaded yet
          </p>
        ) : preview.error ? (
          <p role="alert" className="p-6 text-center text-xs text-destructive">
            Could not read this version. {unknownErrorMessage(preview.error)}
          </p>
        ) : preview.data ? (
          <InkCanvas
            draft={preview.data}
            disabled
            tool="pen"
            width={3}
            mouseEnabled={false}
            onChange={noop}
            onActiveChange={noop}
            onLimit={noop}
          />
        ) : (
          <p role="status" className="p-6 text-center text-xs text-muted-foreground">
            Loading this version…
          </p>
        )}
      </div>
      <div className="flex gap-2">
        <Button
          type="button"
          variant={selected ? "secondary" : "ghost"}
          size="xs"
          aria-pressed={selected}
          disabled={busy || !head.available}
          onClick={onSelect}
        >
          Select
        </Button>
        <Button
          type="button"
          size="xs"
          className="ml-auto"
          disabled={busy || !head.available}
          onClick={onKeep}
        >
          Keep this one
        </Button>
      </div>
    </section>
  );
}

function noop() {}

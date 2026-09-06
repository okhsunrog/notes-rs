import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { MarkdownRenderer, type MarkdownOpenHandler } from "@/features/markdown";
import { useAttachmentImageResolver } from "@/features/markdown";
import {
  CommandFailure,
  getPageDocument,
  replacePageDocument,
  type PageDocumentSnapshot,
} from "@/lib/api";
import { DebouncedAction } from "@/lib/debounced-action";
import { notifyError } from "@/lib/notify";
import { queryKeys } from "@/lib/query";
import {
  useDocumentDraftOverlay,
  usePageSessionRegistry,
  type PersistedDocumentSnapshot,
} from "@/features/pages/page-session";
import { DocumentCodec } from "./document-codec";
import { ContinuousDocumentEditor, type DocumentAuthoringMode } from "./continuous-document-editor";
import { documentUnitsForSave } from "./document-save-model";

type Props = {
  pageUuid: string;
  editing: boolean;
  canEdit: boolean;
  authoringMode: DocumentAuthoringMode;
  focusRequest: number;
  onOpenMarkdownLink: MarkdownOpenHandler;
  onFlushReady?: (flush: (() => Promise<boolean>) | null) => void;
};

type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

const DOCUMENT_AUTOSAVE_MS = 400;

export function pageDocumentToPersisted(snapshot: PageDocumentSnapshot): PersistedDocumentSnapshot {
  const encoded = DocumentCodec.encode(
    snapshot.blocks.map((block) => ({
      uuid: block.uuid,
      parentUuid: block.parentUuid,
      style: block.style,
      markdown: block.markdown,
      revision: block.markdownRevision,
    })),
  );
  return Object.freeze({
    buffer: encoded.markdown,
    sourceMap: encoded.sourceMap,
    revision: snapshot.revision,
  });
}

export function DocumentPage({
  pageUuid,
  editing,
  canEdit,
  authoringMode,
  focusRequest,
  onOpenMarkdownLink,
  onFlushReady,
}: Props) {
  const queryClient = useQueryClient();
  const sessions = usePageSessionRegistry();
  const documentQuery = useQuery({
    queryKey: queryKeys.pageDocument(pageUuid),
    queryFn: () => getPageDocument(pageUuid),
  });
  const persisted = useMemo(
    () => (documentQuery.data ? pageDocumentToPersisted(documentQuery.data) : null),
    [documentQuery.data],
  );

  useEffect(() => {
    if (persisted) sessions.acceptDocumentSnapshot(pageUuid, persisted);
  }, [pageUuid, persisted, sessions]);

  if (documentQuery.isPending) return <DocumentLoading />;
  if (documentQuery.error) return <DocumentError message={String(documentQuery.error)} />;
  if (!documentQuery.data || !persisted) {
    return <DocumentError message="This document is no longer available." />;
  }

  return (
    <LoadedDocumentPage
      pageUuid={pageUuid}
      persisted={persisted}
      editing={editing}
      canEdit={canEdit}
      authoringMode={authoringMode}
      focusRequest={focusRequest}
      onOpenMarkdownLink={onOpenMarkdownLink}
      onFlushReady={onFlushReady}
      queryClient={queryClient}
    />
  );
}

type LoadedProps = Props & {
  persisted: PersistedDocumentSnapshot;
  queryClient: ReturnType<typeof useQueryClient>;
};

function LoadedDocumentPage({
  pageUuid,
  persisted,
  editing,
  canEdit,
  authoringMode,
  focusRequest,
  onOpenMarkdownLink,
  onFlushReady,
  queryClient,
}: LoadedProps) {
  const sessions = usePageSessionRegistry();
  const overlay = useDocumentDraftOverlay(pageUuid);
  const buffer = overlay?.draft ?? persisted.buffer;
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const autosave = useRef(new DebouncedAction()).current;
  const saveInFlight = useRef<Promise<boolean> | null>(null);
  const persistedRef = useRef(persisted);
  persistedRef.current = persisted;

  const flush = useCallback(async (): Promise<boolean> => {
    autosave.cancel();
    if (saveInFlight.current) return saveInFlight.current;
    const pending = (async () => {
      while (true) {
        const attempt = sessions.beginDocumentSave(pageUuid);
        if (!attempt) {
          const current = sessions.getSnapshot(pageUuid).document;
          if (current?.conflict || current?.inFlight) {
            setSaveState(current.conflict ? "error" : "saving");
            return false;
          }
          setSaveState("idle");
          return true;
        }
        setSaveState("saving");
        try {
          const units = documentUnitsForSave(attempt.draft, attempt.baseSourceMap);
          const updated = await replacePageDocument(pageUuid, attempt.expectedRevision, units);
          const rebased = pageDocumentToPersisted(updated);
          queryClient.setQueryData(queryKeys.pageDocument(pageUuid), updated);
          sessions.acknowledgeDocumentSave(pageUuid, attempt, rebased);
          if (sessions.getSnapshot(pageUuid).document) continue;
          setSaveState("saved");
          return true;
        } catch (error) {
          sessions.failDocumentSave(pageUuid, attempt);
          if (error instanceof CommandFailure && error.code === "conflict") {
            try {
              const latest = await getPageDocument(pageUuid);
              if (latest) {
                queryClient.setQueryData(queryKeys.pageDocument(pageUuid), latest);
                sessions.acceptDocumentSnapshot(pageUuid, pageDocumentToPersisted(latest));
              }
            } catch (refreshError) {
              console.error("document conflict refresh failed", refreshError);
            }
          }
          setSaveState("error");
          notifyError("document save", error);
          return false;
        }
      }
    })();
    saveInFlight.current = pending;
    try {
      return await pending;
    } finally {
      if (saveInFlight.current === pending) saveInFlight.current = null;
    }
  }, [autosave, pageUuid, queryClient, sessions]);

  const scheduleSave = useCallback(() => {
    if (!canEdit) return;
    setSaveState("dirty");
    autosave.schedule(() => void flush(), DOCUMENT_AUTOSAVE_MS);
  }, [autosave, canEdit, flush]);

  useEffect(() => {
    onFlushReady?.(flush);
    return () => onFlushReady?.(null);
  }, [flush, onFlushReady]);

  const updateDraft = useCallback(
    (next: string, composing: boolean) => {
      if (!canEdit) return;
      sessions.editDocument(pageUuid, next, persistedRef.current);
      setSaveState("dirty");
      if (!composing) scheduleSave();
    },
    [canEdit, pageUuid, scheduleSave, sessions],
  );

  useEffect(() => {
    if (overlay?.conflict) setSaveState("error");
    else if (overlay?.inFlight) setSaveState("saving");
    else if (overlay) {
      if (saveState !== "error") setSaveState("dirty");
    } else if (saveState !== "saved") setSaveState("idle");
  }, [overlay, saveState]);

  useEffect(() => {
    return () => {
      if (autosave.cancel()) void flush();
    };
  }, [autosave, flush]);

  if (!editing) {
    return (
      <DocumentReadingSurface
        pageUuid={pageUuid}
        markdown={buffer}
        onOpenMarkdownLink={onOpenMarkdownLink}
      />
    );
  }

  return (
    <section className="document-authoring" aria-label="Document editor">
      <div className="mb-3 flex min-h-5 items-center justify-end">
        <DocumentSaveIndicator state={saveState} />
      </div>
      {overlay?.conflict && canEdit && (
        <div
          role="alert"
          className="mb-4 flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs"
        >
          <span className="mr-auto text-foreground">
            This document changed on another replica. Choose which version to keep.
          </span>
          <button
            type="button"
            onClick={() => sessions.useRemoteDocument(pageUuid)}
            className="rounded-md border bg-background px-2 py-1 hover:bg-accent"
          >
            Use remote
          </button>
          <button
            type="button"
            onClick={() => {
              sessions.keepLocalDocument(pageUuid);
              scheduleSave();
            }}
            className="rounded-md bg-primary px-2 py-1 text-primary-foreground hover:bg-primary/90"
          >
            Keep mine
          </button>
        </div>
      )}
      <div className="relative rounded-2xl border border-border/55 surface-card-soft px-5 py-4 shadow-sm">
        {buffer.length === 0 && (
          <p className="pointer-events-none absolute top-5 left-5 text-sm text-muted-foreground/45">
            Start writing your document…
          </p>
        )}
        <ContinuousDocumentEditor
          value={buffer}
          readOnly={!canEdit}
          focusRequest={focusRequest}
          mode={authoringMode}
          pageUuid={pageUuid}
          onChange={updateDraft}
          onCompositionEnd={(value) => {
            updateDraft(value, false);
          }}
          onBlur={() => {
            if (canEdit) void flush();
          }}
          onOpenMarkdownLink={onOpenMarkdownLink}
        />
      </div>
    </section>
  );
}

export function DocumentReadingSurface({
  pageUuid,
  markdown,
  onOpenMarkdownLink,
}: {
  pageUuid: string;
  markdown: string;
  onOpenMarkdownLink?: MarkdownOpenHandler;
}) {
  const resolveImage = useAttachmentImageResolver(markdown);
  if (!markdown.trim()) {
    return (
      <div className="rounded-2xl border border-dashed border-border/60 px-5 py-12 text-center text-sm text-muted-foreground">
        This document is empty.
      </div>
    );
  }
  return (
    <MarkdownRenderer
      className="document-reading-surface"
      context={{ kind: "note", presentation: "reading", pageUuid }}
      markdown={markdown}
      onOpenLink={onOpenMarkdownLink}
      resolveImage={resolveImage}
    />
  );
}

function DocumentLoading() {
  return (
    <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-muted-foreground">
      <Loader2 className="size-4 animate-spin" />
      Loading document…
    </div>
  );
}

function DocumentError({ message }: { message: string }) {
  return (
    <div
      role="alert"
      className="rounded-xl border border-destructive/25 bg-destructive/5 p-4 text-sm"
    >
      Could not load document: {message}
    </div>
  );
}

function DocumentSaveIndicator({ state }: { state: SaveState }) {
  if (state === "idle") return null;
  if (state === "dirty") return <span className="text-xs text-muted-foreground">unsaved…</span>;
  if (state === "saving") {
    return (
      <span className="flex items-center gap-1 text-xs text-muted-foreground">
        <Loader2 className="size-3 animate-spin" /> saving document
      </span>
    );
  }
  if (state === "saved") return <span className="text-xs text-primary">document saved</span>;
  return <span className="text-xs text-destructive">document save failed</span>;
}

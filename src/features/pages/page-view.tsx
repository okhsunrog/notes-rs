import { useCallback, useEffect, useRef, useState } from "react";
import {
  CalendarDays,
  Check,
  Clock3,
  FileText,
  ListTree,
  Loader2,
  MoreHorizontal,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DocumentPage } from "@/features/document/document-page";
import { DocumentAuthoringControls } from "@/features/document/document-authoring-controls";
import {
  documentAuthoringAvailability,
  transitionDocumentAuthoring,
  type DocumentAuthoringAction,
} from "@/features/document/document-authoring-model";
import { useDocumentAuthoringPreference } from "@/features/document/document-authoring-preference";
import { Outliner } from "@/features/outliner/outliner";
import type { MarkdownOpenHandler } from "@/features/markdown";
import { AttachmentsCard } from "@/features/attachments/attachments-card";
import { pageDisplayTitle, shiftJournalDate } from "@/features/journal/journal-date";
import { notifyError } from "@/lib/notify";
import {
  CommandFailure,
  getPage,
  renamePage,
  setPageLayout,
  type JournalDate,
  type Page,
  type PageLayout,
} from "@/lib/api";
import { DebouncedAction } from "@/lib/debounced-action";
import { PagePresentation } from "./page-presentation";
import {
  usePageSessionRegistry,
  usePageWriterLease,
  usePageWriterPaneId,
  useTitleDraftOverlay,
} from "./page-session";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
  type PaneId,
} from "@/features/workspace/workspace-model";

type Props = {
  paneId: PaneId;
  page: Page;
  onSaved: (updated: Page) => void;
  onClose: () => void;
  onDelete: (page: Page) => void | Promise<void>;
  onOpenMarkdownLink: MarkdownOpenHandler;
  onOpenJournalDate: (date: JournalDate, disposition?: OpenDisposition) => void | Promise<void>;
  journalBusy: boolean;
  initialBlockUuid?: string | null;
  autoFocusTitle?: boolean;
  presentation: PagePresentation;
  onPresentationChange: (presentation: PagePresentation) => void;
};

type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

const AUTOSAVE_MS = 400;

export function PageView({
  paneId,
  page,
  onSaved,
  onClose,
  onDelete,
  onOpenMarkdownLink,
  onOpenJournalDate,
  journalBusy,
  initialBlockUuid = null,
  autoFocusTitle = false,
  presentation,
  onPresentationChange,
}: Props) {
  const sessions = usePageSessionRegistry();
  const titleOverlay = useTitleDraftOverlay(page.uuid);
  const writerPaneId = usePageWriterPaneId(page.uuid);
  const ownsWriter = usePageWriterLease(
    page.uuid,
    paneId,
    presentation === PagePresentation.Editing,
  );
  const title = titleOverlay?.draft ?? page.title ?? "";
  const canEdit = presentation === PagePresentation.Editing && ownsWriter;
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [layoutBusy, setLayoutBusy] = useState(false);
  const [bodyFocusRequest, setBodyFocusRequest] = useState(0);
  const [documentAuthoringMode, setDocumentAuthoringMode] = useDocumentAuthoringPreference();
  const authoringAvailability = documentAuthoringAvailability(paneId, writerPaneId);

  const autosave = useRef(new DebouncedAction()).current;
  const titleInput = useRef<HTMLInputElement>(null);
  const pageRef = useRef(page);
  const titleRef = useRef(title);
  const titleSaveInFlight = useRef<Promise<boolean> | null>(null);
  const documentFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const onSavedRef = useRef(onSaved);

  titleRef.current = title;
  onSavedRef.current = onSaved;

  const flush = useCallback(async (): Promise<boolean> => {
    autosave.cancel();
    if (titleSaveInFlight.current) return titleSaveInFlight.current;

    const pending = (async () => {
      while (true) {
        const current = pageRef.current;
        if (current.kind.kind === "journal") {
          setSaveState("idle");
          return true;
        }
        const attempt = sessions.beginTitleSave(current.uuid);
        if (!attempt) {
          const overlay = sessions.getSnapshot(current.uuid).title;
          if (overlay?.conflict || overlay?.inFlight) {
            setSaveState(overlay.conflict ? "error" : "saving");
            return false;
          }
          setSaveState("idle");
          return true;
        }
        const nextTitle = attempt.draft.trim() || null;
        setSaveState("saving");
        try {
          if (nextTitle === (current.title ?? null)) {
            sessions.acknowledgeTitleSave(current.uuid, attempt, {
              text: current.title ?? "",
              revision: current.titleRevision,
            });
          } else {
            const updated = await renamePage(current.uuid, nextTitle, attempt.expectedRevision);
            pageRef.current = updated;
            sessions.acknowledgeTitleSave(current.uuid, attempt, {
              text: updated.title ?? "",
              revision: updated.titleRevision,
            });
            onSavedRef.current(updated);
          }
          if (sessions.getSnapshot(current.uuid).title) continue;
          setSaveState("saved");
          return true;
        } catch (err) {
          sessions.failTitleSave(current.uuid, attempt);
          if (err instanceof CommandFailure && err.code === "conflict") {
            try {
              const latest = await getPage(current.uuid);
              if (latest) {
                pageRef.current = latest;
                onSavedRef.current(latest);
                sessions.acceptTitleSnapshot(current.uuid, {
                  text: latest.title ?? "",
                  revision: latest.titleRevision,
                });
              }
            } catch (refreshError) {
              console.error("title conflict refresh failed", refreshError);
            }
          }
          setSaveState("error");
          notifyError("save", err);
          return false;
        }
      }
    })();
    titleSaveInFlight.current = pending;
    try {
      return await pending;
    } finally {
      if (titleSaveInFlight.current === pending) titleSaveInFlight.current = null;
    }
  }, [autosave, sessions]);

  const scheduleSave = useCallback(() => {
    if (!canEdit) return;
    setSaveState("dirty");
    autosave.schedule(() => void flush(), AUTOSAVE_MS);
  }, [autosave, canEdit, flush]);

  const registerDocumentFlush = useCallback((flushDocument: (() => Promise<boolean>) | null) => {
    documentFlushRef.current = flushDocument;
  }, []);

  const selectDocumentAction = useCallback(
    (action: DocumentAuthoringAction) => {
      const transition = transitionDocumentAuthoring(
        action,
        documentAuthoringMode,
        authoringAvailability,
      );
      if (!transition) return;
      if (transition.persistedAuthoringMode) {
        setDocumentAuthoringMode(transition.persistedAuthoringMode);
      }
      onPresentationChange(transition.presentation);
    },
    [authoringAvailability, documentAuthoringMode, onPresentationChange, setDocumentAuthoringMode],
  );

  useEffect(() => {
    pageRef.current = page;
    sessions.acceptTitleSnapshot(page.uuid, {
      text: page.title ?? "",
      revision: page.titleRevision,
    });
  }, [page, sessions]);

  useEffect(() => {
    const overlay = titleOverlay;
    if (overlay?.conflict) setSaveState("error");
    else if (overlay?.inFlight) setSaveState("saving");
    else if (overlay) {
      if (saveState !== "error") setSaveState("dirty");
    } else if (saveState !== "saved") setSaveState("idle");
  }, [saveState, titleOverlay]);

  useEffect(() => {
    setBodyFocusRequest(0);
  }, [page.uuid]);

  useEffect(() => {
    if (!autoFocusTitle || !canEdit) return;
    const input = titleInput.current;
    input?.focus();
    input?.select();
  }, [autoFocusTitle, canEdit, page.uuid]);

  const changeLayout = async (layout: PageLayout) => {
    if (!canEdit || layout === pageRef.current.layout || layoutBusy) return;
    setLayoutBusy(true);
    try {
      if (!(await flush())) return;
      if (
        pageRef.current.layout === "document" &&
        layout !== "document" &&
        documentFlushRef.current &&
        !(await documentFlushRef.current())
      ) {
        return;
      }
      const updated = await setPageLayout(pageRef.current.uuid, layout);
      pageRef.current = updated;
      onSavedRef.current(updated);
    } catch (error) {
      notifyError("layout", error);
    } finally {
      setLayoutBusy(false);
    }
  };

  useEffect(() => {
    return () => {
      if (autosave.cancel()) {
        void flush();
      }
    };
  }, [autosave, flush]);

  const journalDate = page.kind.kind === "journal" ? page.kind.date : null;

  return (
    <article className="editor-page mx-auto flex min-h-full max-w-[52rem] flex-col px-8 pt-12 pb-24 sm:px-12 lg:px-16">
      <div className="mb-8 flex items-center gap-2 text-[11px] font-medium text-muted-foreground">
        <button type="button" onClick={onClose} className="transition hover:text-foreground">
          All notes
        </button>
        <span>/</span>
        <span className="truncate">{pageDisplayTitle(page)}</span>
        <span className="ml-auto flex items-center gap-1.5">
          <Clock3 className="size-3" />
          {new Date(page.updatedAt * 1000).toLocaleDateString(undefined, {
            month: "short",
            day: "numeric",
          })}
        </span>
      </div>

      <div className="group flex items-start gap-3">
        {journalDate ? (
          <div className="min-w-0 flex-1">
            <div className="mb-2 flex items-center gap-2 text-xs font-semibold tracking-[0.12em] text-primary uppercase">
              <CalendarDays className="size-3.5" />
              Journal · {journalDate}
            </div>
            <h1 className="text-[2.6rem] leading-tight font-semibold tracking-[-0.045em]">
              {pageDisplayTitle(page)}
            </h1>
          </div>
        ) : (
          <Input
            ref={titleInput}
            value={title}
            readOnly={!canEdit}
            onBlur={() => {
              if (canEdit) void flush();
            }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === "Enter") {
                event.preventDefault();
                if (!canEdit) return;
                const input = event.currentTarget;
                void (async () => {
                  if (!(await flush())) return;
                  setBodyFocusRequest((request) => request + 1);
                  input.blur();
                })();
              }
            }}
            onChange={(e) => {
              if (!canEdit) return;
              sessions.editTitle(page.uuid, e.currentTarget.value, {
                text: pageRef.current.title ?? "",
                revision: pageRef.current.titleRevision,
              });
              scheduleSave();
            }}
            placeholder="Untitled note"
            className="h-20 min-w-0 appearance-none border-0 bg-transparent px-0 py-3 text-[2.6rem] leading-normal font-semibold tracking-[-0.045em] shadow-none placeholder:text-muted-foreground/35 focus-visible:ring-0"
          />
        )}
        <div className="mt-2 flex items-center gap-1">
          <SaveIndicator state={saveState} />
          <Button
            variant="ghost"
            size="icon-sm"
            className="rounded-lg text-muted-foreground opacity-0 transition group-hover:opacity-100 focus:opacity-100"
            aria-label="More note actions"
          >
            <MoreHorizontal className="size-4" />
          </Button>
        </div>
      </div>

      {titleOverlay?.conflict && canEdit && (
        <div
          role="alert"
          className="mt-3 flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs"
        >
          <span className="mr-auto text-foreground">
            This title changed on another replica. Choose which version to keep.
          </span>
          <button
            type="button"
            onClick={() => sessions.useRemoteTitle(page.uuid)}
            className="rounded-md border bg-background px-2 py-1 hover:bg-accent"
          >
            Use remote
          </button>
          <button
            type="button"
            onClick={() => {
              sessions.keepLocalTitle(page.uuid);
              scheduleSave();
            }}
            className="rounded-md bg-primary px-2 py-1 text-primary-foreground hover:bg-primary/90"
          >
            Keep mine
          </button>
        </div>
      )}

      <div className="mt-4 mb-9 flex items-center gap-2">
        <span className="rounded-full bg-primary/10 px-2.5 py-1 text-[10px] font-semibold tracking-wide text-primary">
          {page.layout.toUpperCase()}
        </span>
        {journalDate && (
          <div className="flex items-center rounded-lg border border-border/60 bg-card/55 p-0.5">
            <Button
              type="button"
              variant="ghost"
              size="xs"
              disabled={journalBusy}
              onClick={(event) =>
                void onOpenJournalDate(
                  shiftJournalDate(journalDate, -1),
                  dispositionFromShiftKey(event.shiftKey),
                )
              }
              className="rounded-md px-2 text-[10px]"
            >
              Previous day
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="xs"
              disabled={journalBusy}
              onClick={(event) =>
                void onOpenJournalDate(
                  shiftJournalDate(journalDate, 1),
                  dispositionFromShiftKey(event.shiftKey),
                )
              }
              className="rounded-md px-2 text-[10px]"
            >
              Next day
            </Button>
          </div>
        )}
        <div
          role="group"
          aria-label="Page layout"
          className="ml-1 flex items-center rounded-lg border border-border/60 bg-card/55 p-0.5"
        >
          {PAGE_LAYOUTS.map(({ value, label, icon: Icon }) => (
            <Button
              key={value}
              type="button"
              variant={page.layout === value ? "secondary" : "ghost"}
              size="xs"
              disabled={layoutBusy || !canEdit}
              aria-pressed={page.layout === value}
              onClick={() => void changeLayout(value)}
              className="rounded-md px-2 text-[10px]"
            >
              <Icon className="size-3" />
              <span className="hidden sm:inline">{label}</span>
            </Button>
          ))}
        </div>
        <DocumentAuthoringControls
          layout={page.layout}
          presentation={presentation}
          authoringMode={documentAuthoringMode}
          availability={authoringAvailability}
          onAction={selectDocumentAction}
        />
        <Button
          variant="ghost"
          size="xs"
          className="ml-auto rounded-lg text-muted-foreground opacity-60 hover:text-destructive hover:opacity-100"
          aria-label="Delete page"
          disabled={!canEdit}
          onClick={() => void onDelete(page)}
        >
          <Trash2 className="size-4" />
        </Button>
      </div>

      <div className="editor-body">
        {page.layout === "document" ? (
          <DocumentPage
            key={page.uuid}
            pageUuid={page.uuid}
            editing={presentation === PagePresentation.Editing}
            canEdit={canEdit}
            authoringMode={documentAuthoringMode}
            focusRequest={bodyFocusRequest}
            onOpenMarkdownLink={onOpenMarkdownLink}
            onFlushReady={registerDocumentFlush}
          />
        ) : (
          <Outliner
            key={page.uuid}
            page={page}
            initialEditingUuid={initialBlockUuid}
            focusRequest={bodyFocusRequest}
            onOpenMarkdownLink={onOpenMarkdownLink}
            presentation={presentation}
            readOnly={!canEdit}
          />
        )}
      </div>
      <div className="mt-16">
        <AttachmentsCard location={{ kind: "page", uuid: page.uuid }} readOnly={!canEdit} />
      </div>
    </article>
  );
}

const PAGE_LAYOUTS: Array<{
  value: PageLayout;
  label: string;
  icon: typeof ListTree;
}> = [
  { value: "outline", label: "Outline", icon: ListTree },
  { value: "document", label: "Document", icon: FileText },
];

function SaveIndicator({ state }: { state: SaveState }) {
  if (state === "idle") return null;
  if (state === "dirty") return <span className="text-xs text-muted-foreground">unsaved…</span>;
  if (state === "saving")
    return (
      <span className="flex items-center gap-1 text-xs text-muted-foreground">
        <Loader2 className="size-3 animate-spin" />
        saving
      </span>
    );
  if (state === "saved")
    return (
      <span className="flex items-center gap-1 text-xs text-emerald-600">
        <Check className="size-3" />
        saved
      </span>
    );
  return <span className="text-xs text-destructive">save failed</span>;
}

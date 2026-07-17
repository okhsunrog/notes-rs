import { useCallback, useEffect, useRef, useState } from "react";
import {
  BookOpen,
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
import { Outliner } from "@/features/outliner/outliner";
import type { MarkdownOpenHandler } from "@/features/markdown";
import { AttachmentsCard } from "@/features/attachments/attachments-card";
import { pageDisplayTitle, shiftJournalDate } from "@/features/journal/journal-date";
import { renamePage, setPageLayout, type JournalDate, type Page, type PageLayout } from "@/lib/api";
import { DebouncedAction } from "@/lib/debounced-action";
import { PagePresentation } from "./page-presentation";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";

type Props = {
  page: Page;
  onSaved: (updated: Page) => void;
  onStatus: (s: string) => void;
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
  page,
  onSaved,
  onStatus,
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
  const [title, setTitle] = useState(page.title ?? "");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [layoutBusy, setLayoutBusy] = useState(false);
  const [bodyFocusRequest, setBodyFocusRequest] = useState(0);

  const autosave = useRef(new DebouncedAction()).current;
  const titleInput = useRef<HTMLInputElement>(null);
  const pageRef = useRef(page);
  const titleRef = useRef(title);
  const onSavedRef = useRef(onSaved);
  const onStatusRef = useRef(onStatus);

  titleRef.current = title;
  onSavedRef.current = onSaved;
  onStatusRef.current = onStatus;

  const flush = useCallback(async (): Promise<boolean> => {
    autosave.cancel();
    const current = pageRef.current;
    if (current.kind.kind === "journal") {
      setSaveState("idle");
      return true;
    }
    const nextTitle = titleRef.current.trim() || null;
    if (nextTitle === (current.title ?? null)) {
      setSaveState("idle");
      return true;
    }
    setSaveState("saving");
    try {
      const updated = await renamePage(current.uuid, nextTitle, current.titleRevision);
      pageRef.current = updated;
      onSavedRef.current(updated);
      setSaveState("saved");
      return true;
    } catch (err) {
      setSaveState("error");
      onStatusRef.current(`save error: ${String(err)}`);
      return false;
    }
  }, [autosave]);

  const scheduleSave = useCallback(() => {
    setSaveState("dirty");
    autosave.schedule(() => void flush(), AUTOSAVE_MS);
  }, [autosave, flush]);

  useEffect(() => {
    autosave.cancel();
    pageRef.current = page;
    setTitle(page.title ?? "");
    setSaveState("idle");
  }, [autosave, page]);

  useEffect(() => {
    setBodyFocusRequest(0);
  }, [page.uuid]);

  useEffect(() => {
    if (!autoFocusTitle) return;
    const input = titleInput.current;
    input?.focus();
    input?.select();
  }, [autoFocusTitle, page.uuid]);

  const changeLayout = async (layout: PageLayout) => {
    if (
      presentation === PagePresentation.Reading ||
      layout === pageRef.current.layout ||
      layoutBusy
    )
      return;
    setLayoutBusy(true);
    try {
      await flush();
      const updated = await setPageLayout(pageRef.current.uuid, layout);
      pageRef.current = updated;
      onSavedRef.current(updated);
    } catch (error) {
      onStatusRef.current(`layout error: ${String(error)}`);
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
            readOnly={presentation === PagePresentation.Reading}
            onBlur={() => void flush()}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === "Enter") {
                event.preventDefault();
                if (presentation === PagePresentation.Reading) return;
                const input = event.currentTarget;
                void (async () => {
                  if (!(await flush())) return;
                  setBodyFocusRequest((request) => request + 1);
                  input.blur();
                })();
              }
            }}
            onChange={(e) => {
              setTitle(e.currentTarget.value);
              scheduleSave();
            }}
            placeholder="Untitled note"
            className="h-[3.5rem] min-w-0 border-0 bg-transparent px-0 py-1 text-[2.6rem] leading-tight font-semibold tracking-[-0.045em] shadow-none placeholder:text-muted-foreground/35 focus-visible:ring-0"
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
              disabled={layoutBusy || presentation === PagePresentation.Reading}
              aria-pressed={page.layout === value}
              onClick={() => void changeLayout(value)}
              className="rounded-md px-2 text-[10px]"
            >
              <Icon className="size-3" />
              <span className="hidden sm:inline">{label}</span>
            </Button>
          ))}
        </div>
        {page.layout === "document" && (
          <div
            role="group"
            aria-label="Document presentation"
            className="flex items-center rounded-lg border border-border/60 bg-card/55 p-0.5"
          >
            {DOCUMENT_PRESENTATIONS.map(({ value, label, icon: Icon }) => (
              <Button
                key={value}
                type="button"
                variant={presentation === value ? "secondary" : "ghost"}
                size="xs"
                aria-pressed={presentation === value}
                onClick={() => onPresentationChange(value)}
                className="rounded-md px-2 text-[10px]"
              >
                <Icon className="size-3" />
                <span className="hidden sm:inline">{label}</span>
              </Button>
            ))}
          </div>
        )}
        <Button
          variant="ghost"
          size="xs"
          className="ml-auto rounded-lg text-muted-foreground opacity-60 hover:text-destructive hover:opacity-100"
          aria-label="Delete page"
          disabled={presentation === PagePresentation.Reading}
          onClick={() => void onDelete(page)}
        >
          <Trash2 className="size-4" />
        </Button>
      </div>

      <div className="editor-body">
        <Outliner
          key={page.uuid}
          page={page}
          initialEditingUuid={initialBlockUuid}
          focusRequest={bodyFocusRequest}
          onOpenMarkdownLink={onOpenMarkdownLink}
          presentation={presentation}
        />
      </div>
      <div className="mt-16">
        <AttachmentsCard location={{ kind: "page", uuid: page.uuid }} onStatus={onStatus} />
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

const DOCUMENT_PRESENTATIONS: Array<{
  value: PagePresentation;
  label: string;
  icon: typeof FileText;
}> = [
  { value: PagePresentation.Editing, label: "Write", icon: FileText },
  { value: PagePresentation.Reading, label: "Read", icon: BookOpen },
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

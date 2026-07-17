import { useCallback, useEffect, useRef, useState } from "react";
import {
  BookOpen,
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
import { AttachmentsCard } from "@/features/attachments/attachments-card";
import { renamePage, setPageView, type Page, type PageView } from "@/lib/api";
import { DebouncedAction } from "@/lib/debounced-action";

type Props = {
  page: Page;
  onSaved: (updated: Page) => void;
  onStatus: (s: string) => void;
  onClose: () => void;
  onDelete: (page: Page) => void | Promise<void>;
  initialBlockUuid?: string | null;
  autoFocusTitle?: boolean;
};

type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

const AUTOSAVE_MS = 400;

export function PageView({
  page,
  onSaved,
  onStatus,
  onClose,
  onDelete,
  initialBlockUuid = null,
  autoFocusTitle = false,
}: Props) {
  const [title, setTitle] = useState(page.title ?? "");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [viewBusy, setViewBusy] = useState(false);
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
    const nextTitle = titleRef.current.trim() || null;
    if (nextTitle === (current.title ?? null)) {
      setSaveState("idle");
      return true;
    }
    setSaveState("saving");
    try {
      const updated = await renamePage(current.uuid, nextTitle);
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

  const changeView = async (view: PageView) => {
    if (view === pageRef.current.defaultView || viewBusy) return;
    setViewBusy(true);
    try {
      await flush();
      const updated = await setPageView(pageRef.current.uuid, view);
      pageRef.current = updated;
      onSavedRef.current(updated);
    } catch (error) {
      onStatusRef.current(`view error: ${String(error)}`);
    } finally {
      setViewBusy(false);
    }
  };

  useEffect(() => {
    return () => {
      if (autosave.cancel()) {
        void flush();
      }
    };
  }, [autosave, flush]);

  return (
    <article className="editor-page mx-auto flex min-h-full max-w-[52rem] flex-col px-8 pt-12 pb-24 sm:px-12 lg:px-16">
      <div className="mb-8 flex items-center gap-2 text-[11px] font-medium text-muted-foreground">
        <button type="button" onClick={onClose} className="transition hover:text-foreground">
          All notes
        </button>
        <span>/</span>
        <span className="truncate">{title || "Untitled"}</span>
        <span className="ml-auto flex items-center gap-1.5">
          <Clock3 className="size-3" />
          {new Date(page.updatedAt * 1000).toLocaleDateString(undefined, {
            month: "short",
            day: "numeric",
          })}
        </span>
      </div>

      <div className="group flex items-start gap-3">
        <Input
          ref={titleInput}
          value={title}
          readOnly={page.defaultView === "reading"}
          onBlur={() => void flush()}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === "Enter") {
              event.preventDefault();
              if (pageRef.current.defaultView === "reading") return;
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
          {page.defaultView.toUpperCase()}
        </span>
        <div
          role="group"
          aria-label="Page view"
          className="ml-1 flex items-center rounded-lg border border-border/60 bg-card/55 p-0.5"
        >
          {PAGE_VIEWS.map(({ value, label, icon: Icon }) => (
            <Button
              key={value}
              type="button"
              variant={page.defaultView === value ? "secondary" : "ghost"}
              size="xs"
              disabled={viewBusy}
              aria-pressed={page.defaultView === value}
              onClick={() => void changeView(value)}
              className="rounded-md px-2 text-[10px]"
            >
              <Icon className="size-3" />
              <span className="hidden sm:inline">{label}</span>
            </Button>
          ))}
        </div>
        <Button
          variant="ghost"
          size="xs"
          className="ml-auto rounded-lg text-muted-foreground opacity-60 hover:text-destructive hover:opacity-100"
          aria-label="Delete page"
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
        />
      </div>
      <div className="mt-16">
        <AttachmentsCard location={{ kind: "page", uuid: page.uuid }} onStatus={onStatus} />
      </div>
    </article>
  );
}

const PAGE_VIEWS: Array<{
  value: PageView;
  label: string;
  icon: typeof ListTree;
}> = [
  { value: "outline", label: "Outline", icon: ListTree },
  { value: "document", label: "Document", icon: FileText },
  { value: "reading", label: "Reading", icon: BookOpen },
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

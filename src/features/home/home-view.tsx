import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowRight,
  ArrowUp,
  CalendarDays,
  ChevronDown,
  ChevronUp,
  Clock3,
  FilePlus2,
  FileText,
  PencilLine,
  Star,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { NewNoteButton } from "@/features/handwriting/new-note-button";
import { PageIcon } from "@/features/pages/page-icon";
import { Input } from "@/components/ui/input";
import { useCompactLayout } from "@/app/use-compact-layout";
import { pageDisplayTitle, todayJournalDate } from "@/features/journal/journal-date";
import { MarkdownRenderer } from "@/features/markdown";
import { usePageNavigationStore } from "@/features/pages/page-navigation-store";
import {
  getJournal,
  getPageDocument,
  listJournals,
  listPages,
  type Block,
  type Content,
  type JournalDate,
  type Page,
} from "@/lib/api";
import { notifyError } from "@/lib/notify";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";
import { journalOutlineRows, journalPreviewLimitForHeight } from "./journal-outline-preview";

type Props = {
  creating: boolean;
  journalBusy: boolean;
  onCreate: () => void | Promise<void>;
  onCapture: (date: JournalDate, markdown: string) => boolean | Promise<boolean>;
  onOpenJournal: (date: JournalDate, disposition?: OpenDisposition) => void | Promise<void>;
  onOpenContent: (content: Content, disposition?: OpenDisposition) => void | Promise<void>;
  onOpenAllNotes: () => void;
};

export function HomeView({
  creating,
  journalBusy,
  onCreate,
  onCapture,
  onOpenJournal,
  onOpenContent,
  onOpenAllNotes,
}: Props) {
  const compact = useCompactLayout();
  const recentPageUuids = usePageNavigationStore((state) => state.recentPageUuids);
  const favoritePageUuids = usePageNavigationStore((state) => state.favoritePageUuids);
  const today = todayJournalDate();
  const notesQuery = useQuery({
    queryKey: [...queryKeys.pages, "dashboard-notes"],
    queryFn: () => listPages({ filter: "notes", limit: 10_000 }),
  });
  const journalsQuery = useQuery({
    queryKey: [...queryKeys.journals, "dashboard"],
    queryFn: () => listJournals({ limit: 7 }),
  });
  const todayJournalQuery = useQuery({
    queryKey: queryKeys.journal(today),
    queryFn: () => getJournal(today),
  });
  const todayJournalUuid = todayJournalQuery.data?.uuid;
  const todayDocumentQuery = useQuery({
    queryKey: todayJournalUuid
      ? queryKeys.pageDocument(todayJournalUuid)
      : [...queryKeys.pageDocumentRoot, "today-empty"],
    queryFn: () => getPageDocument(todayJournalUuid!),
    enabled: Boolean(todayJournalUuid),
  });
  const notes = notesQuery.data ?? [];
  const pagesByUuid = useMemo(() => new Map(notes.map((page) => [page.uuid, page])), [notes]);
  const favorites = favoritePageUuids.flatMap((uuid) => {
    const page = pagesByUuid.get(uuid);
    return page ? [page] : [];
  });
  const recent = recentPageUuids.flatMap((uuid) => {
    const page = pagesByUuid.get(uuid);
    return page ? [page] : [];
  });
  const recentlyEdited = notes.slice(0, 6);
  const firstRun =
    notesQuery.isSuccess &&
    journalsQuery.isSuccess &&
    notes.length === 0 &&
    journalsQuery.data.length === 0;

  useEffect(() => {
    if (notesQuery.error) notifyError("dashboard", notesQuery.error);
    if (journalsQuery.error) notifyError("dashboard", journalsQuery.error);
    if (todayJournalQuery.error) notifyError("dashboard", todayJournalQuery.error);
    if (todayDocumentQuery.error) notifyError("dashboard", todayDocumentQuery.error);
  }, [journalsQuery.error, notesQuery.error, todayDocumentQuery.error, todayJournalQuery.error]);

  const openPage = (page: Page, disposition?: OpenDisposition) =>
    onOpenContent({ kind: "page", record: page }, disposition);

  if (firstRun) {
    return (
      <div className="mx-auto flex min-h-full max-w-3xl flex-col justify-center px-8 py-16 sm:px-12">
        <p className="text-xs font-semibold tracking-[0.16em] text-primary uppercase">Dashboard</p>
        <h1 className="mt-3 text-3xl font-semibold tracking-[-0.04em] sm:text-4xl">
          A quiet place for your notes.
        </h1>
        <p className="mt-3 max-w-xl text-sm leading-relaxed text-muted-foreground">
          Start with a note or use today&apos;s journal. This welcome disappears as soon as you add
          something.
        </p>
        <div className="mt-7 flex flex-wrap gap-2.5">
          <NewNoteButton
            disabled={creating}
            onClick={() => void onCreate()}
            className="brand-button h-9 rounded-xl px-5"
          >
            <FilePlus2 className="size-4" />
            Create first note
          </NewNoteButton>
          <Button
            variant="outline"
            disabled={journalBusy}
            onClick={(event) => void onOpenJournal(today, dispositionFromShiftKey(event.shiftKey))}
            className="h-11 rounded-xl px-5"
          >
            <CalendarDays className="size-4 text-primary" />
            Open today
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto min-h-full max-w-4xl px-5 pt-6 pb-[calc(1.5rem+var(--safe-area-inset-bottom))] sm:px-10 sm:py-8">
      <header className="mb-7 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold tracking-[0.16em] text-primary uppercase">
            Dashboard
          </p>
          <h1 className="mt-1 text-2xl font-semibold tracking-[-0.035em] sm:text-3xl">
            {dayGreeting()}
          </h1>
          <p className="mt-1 text-xs text-muted-foreground">
            {new Date().toLocaleDateString(undefined, {
              weekday: "long",
              month: "long",
              day: "numeric",
            })}
          </p>
        </div>
        {compact && (
          <div className="flex gap-2">
            <Button
              type="button"
              variant="outline"
              onClick={onOpenAllNotes}
              className="h-9 rounded-xl px-3.5"
            >
              <FileText className="size-3.5" />
              All notes
            </Button>
            <NewNoteButton
              disabled={creating}
              onClick={() => void onCreate()}
              className="brand-button h-9 rounded-xl px-3.5"
            >
              <FilePlus2 className="size-3.5" />
              New note
            </NewNoteButton>
          </div>
        )}
      </header>

      <div className="space-y-4">
        <DashboardCard className="p-4">
          <div className="mb-2.5 flex flex-wrap items-center justify-between gap-3">
            <div>
              <div className="flex items-center gap-2 text-sm font-semibold">
                <CalendarDays className="size-4 text-primary" />
                Today&apos;s journal
              </div>
              <p className="mt-1 text-xs text-muted-foreground">Capture first, organize later.</p>
            </div>
            <Button
              variant="ghost"
              size="sm"
              disabled={journalBusy}
              onClick={(event) =>
                void onOpenJournal(today, dispositionFromShiftKey(event.shiftKey))
              }
              className="rounded-lg"
            >
              Open today
              <ArrowRight className="size-3.5" />
            </Button>
          </div>
          {compact && (
            <DashboardQuickCapture
              busy={journalBusy}
              onCapture={(markdown) => onCapture(today, markdown)}
            />
          )}
          <JournalOutlinePreview
            blocks={todayDocumentQuery.data?.blocks ?? []}
            loading={todayJournalQuery.isLoading || todayDocumentQuery.isLoading}
          />
        </DashboardCard>

        <div
          className="grid gap-4"
          style={{ gridTemplateColumns: "repeat(auto-fit, minmax(min(18rem, 100%), 1fr))" }}
        >
          <DashboardSection
            title="Continue"
            icon={<Clock3 className="size-4" />}
            empty="Open a note and it will appear here."
            pages={recent.slice(0, 4)}
            compact
            onOpen={openPage}
          />
          {compact && (
            <DashboardSection
              title="Favorites"
              icon={<Star className="size-4" />}
              empty="Star important notes to pin them here."
              pages={favorites.slice(0, 8)}
              compact
              onOpen={openPage}
            />
          )}
        </div>

        <DashboardSection
          title="Recently edited"
          icon={<FileText className="size-4" />}
          empty="Your edited notes will appear here."
          pages={recentlyEdited}
          showDate
          onOpen={openPage}
        />
      </div>
    </div>
  );
}

function JournalOutlinePreview({ blocks, loading }: { blocks: Block[]; loading: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const [limit, setLimit] = useState(() => journalPreviewLimitForHeight(window.innerHeight));
  const rows = useMemo(() => journalOutlineRows(blocks), [blocks]);
  const visibleRows = expanded ? rows : rows.slice(0, limit);
  const hasMore = rows.length > limit;

  useEffect(() => {
    const updateLimit = () => setLimit(journalPreviewLimitForHeight(window.innerHeight));
    window.addEventListener("resize", updateLimit);
    return () => window.removeEventListener("resize", updateLimit);
  }, []);

  if (loading) {
    return <p className="mt-3 text-xs text-muted-foreground">Loading today&apos;s outline…</p>;
  }

  if (rows.length === 0) return null;

  return (
    <div className="mt-4 border-t border-border/50 pt-3">
      <div className="mb-1.5 flex items-center justify-between gap-3">
        <p className="text-[11px] font-semibold tracking-[0.08em] text-muted-foreground uppercase">
          Today so far
        </p>
        <span className="text-[10px] eink:text-xs text-muted-foreground tabular-nums">
          {expanded ? rows.length : Math.min(rows.length, limit)} of {rows.length}
        </span>
      </div>
      <div className="space-y-0.5">
        {visibleRows.map(({ block, depth }, index) => (
          <JournalOutlineRow key={block.uuid} block={block} depth={depth} ordinal={index + 1} />
        ))}
      </div>
      {hasMore && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => setExpanded((value) => !value)}
          className="mt-1.5 h-7 rounded-lg px-2 text-xs text-muted-foreground"
        >
          {expanded ? "Show less" : `Show ${rows.length - limit} more`}
          {expanded ? <ChevronUp className="size-3.5" /> : <ChevronDown className="size-3.5" />}
        </Button>
      )}
    </div>
  );
}

function JournalOutlineRow({
  block,
  depth,
  ordinal,
}: {
  block: Block;
  depth: number;
  ordinal: number;
}) {
  if (block.style.kind === "divider") {
    return <hr className="my-2 border-border/50" style={{ marginLeft: Math.min(depth, 4) * 16 }} />;
  }

  const marker =
    block.style.kind === "numbered"
      ? `${ordinal}.`
      : block.style.kind === "task"
        ? block.style.state === "done"
          ? "✓"
          : "○"
        : "•";
  const heading = block.style.kind.startsWith("heading_");

  return (
    <div
      className="flex min-w-0 items-center gap-2 rounded-lg px-1.5 py-1 text-xs"
      style={{ marginLeft: Math.min(depth, 4) * 16 }}
    >
      <span className="w-3 shrink-0 text-center text-[10px] eink:text-xs text-primary/75">
        {marker}
      </span>
      <MarkdownRenderer
        className={cn("min-w-0 flex-1 truncate", heading && "font-semibold")}
        context={{ kind: "preview", pageUuid: block.pageUuid, blockUuid: block.uuid }}
        markdown={block.markdown}
        mode="inline"
      />
    </div>
  );
}

function DashboardQuickCapture({
  busy,
  onCapture,
}: {
  busy: boolean;
  onCapture: (markdown: string) => boolean | Promise<boolean>;
}) {
  const [markdown, setMarkdown] = useState("");
  const canSubmit = markdown.trim().length > 0 && !busy;

  async function submit() {
    const next = markdown.trim();
    if (!next || busy) return;
    if (await onCapture(next)) setMarkdown("");
  }

  return (
    <form
      className="flex min-h-11 items-center gap-2 rounded-xl border border-border/60 surface-base-soft px-3 py-1.5 transition-colors focus-within:border-primary/30 focus-within:surface-base"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <PencilLine className="size-3.5 shrink-0 text-muted-foreground" />
      <Input
        value={markdown}
        disabled={busy}
        aria-label="Capture a block in today's journal"
        placeholder="Write a thought…"
        onChange={(event) => setMarkdown(event.currentTarget.value)}
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
            event.preventDefault();
            void submit();
          }
        }}
        className="h-8 min-w-0 flex-1 border-0 bg-transparent px-1 text-sm shadow-none focus-visible:ring-0"
      />
      <Button
        type="submit"
        size="icon-sm"
        disabled={!canSubmit}
        aria-label="Add capture to today's journal"
        className="brand-button size-8 shrink-0 rounded-lg"
      >
        <ArrowUp className="size-4" />
      </Button>
    </form>
  );
}

function DashboardSection({
  title,
  icon,
  empty,
  pages,
  compact = false,
  showDate = false,
  onOpen,
}: {
  title: string;
  icon: React.ReactNode;
  empty: string;
  pages: Page[];
  compact?: boolean;
  showDate?: boolean;
  onOpen: (page: Page, disposition?: OpenDisposition) => void | Promise<void>;
}) {
  return (
    <DashboardCard className="p-3.5">
      <div className="mb-2.5 flex items-center gap-2 px-1 text-sm font-semibold">
        <span className="text-primary">{icon}</span>
        {title}
        {pages.length > 0 && (
          <span className="ml-auto text-[10px] eink:text-xs font-medium text-muted-foreground tabular-nums">
            {pages.length}
          </span>
        )}
      </div>
      {pages.length === 0 ? (
        <p className="rounded-xl border border-dashed border-border/60 px-3 py-5 text-center text-xs text-muted-foreground">
          {empty}
        </p>
      ) : (
        <div
          className="grid gap-1.5"
          style={
            compact
              ? undefined
              : { gridTemplateColumns: "repeat(auto-fit, minmax(min(18rem, 100%), 1fr))" }
          }
        >
          {pages.map((page) => (
            <button
              key={page.uuid}
              type="button"
              onClick={(event) => void onOpen(page, dispositionFromShiftKey(event.shiftKey))}
              className="group flex min-w-0 items-center gap-2.5 rounded-xl px-3 py-2.5 text-left transition hover:bg-accent/70"
            >
              <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/8 text-primary">
                <PageIcon page={page} className="size-3.5" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-xs font-medium">{pageDisplayTitle(page)}</span>
                {showDate && (
                  <span className="mt-0.5 block text-[10px] eink:text-xs text-muted-foreground">
                    {formatEditedDate(page.updatedAt)}
                  </span>
                )}
              </span>
              <ArrowRight className="reveal-on-hover size-3.5 shrink-0 text-muted-foreground transition" />
            </button>
          ))}
        </div>
      )}
    </DashboardCard>
  );
}

function DashboardCard({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <section
      className={cn("rounded-2xl border border-border/60 surface-card-soft shadow-sm", className)}
    >
      {children}
    </section>
  );
}

function dayGreeting() {
  const hour = new Date().getHours();
  if (hour < 12) return "Good morning";
  if (hour < 18) return "Good afternoon";
  return "Good evening";
}

function formatEditedDate(timestamp: number) {
  const date = new Date(timestamp * 1000);
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

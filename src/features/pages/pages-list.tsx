import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Clock3, Files, Plus, Star } from "lucide-react";
import { NewNoteButton } from "@/features/handwriting/new-note-button";
import { JournalNavigation } from "@/features/journal/journal-navigation";
import { JournalQuickCapture } from "@/features/journal/journal-quick-capture";
import { RecentJournals } from "@/features/journal/recent-journals";
import { pageDisplayTitle } from "@/features/journal/journal-date";
import { listJournals, listPages, type JournalDate, type Page } from "@/lib/api";
import { cn } from "@/lib/utils";
import { queryKeys } from "@/lib/query";
import { notifyError } from "@/lib/notify";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";
import { PageIcon } from "./page-icon";
import { usePageNavigationStore } from "./page-navigation-store";

type Props = {
  selectedUuid: string | null;
  activeJournalDate: JournalDate | null;
  onSelect: (page: Page, disposition?: OpenDisposition) => void;
  onCreate: () => void | Promise<void>;
  onOpenJournal: (date: JournalDate, disposition?: OpenDisposition) => void | Promise<void>;
  onQuickCapture: (markdown: string) => boolean | Promise<boolean>;
  allNotesActive: boolean;
  onOpenAllNotes: (disposition?: OpenDisposition) => void;
  journalBusy: boolean;
};

export function PagesList({
  selectedUuid,
  activeJournalDate,
  onSelect,
  onCreate,
  onOpenJournal,
  onQuickCapture,
  allNotesActive,
  onOpenAllNotes,
  journalBusy,
}: Props) {
  const [busy, setBusy] = useState(false);
  const recentPageUuids = usePageNavigationStore((state) => state.recentPageUuids);
  const favoritePageUuids = usePageNavigationStore((state) => state.favoritePageUuids);
  const toggleFavoritePage = usePageNavigationStore((state) => state.toggleFavoritePage);
  const queryClient = useQueryClient();
  const pagesQuery = useQuery({
    queryKey: queryKeys.pageList("notes", 10_000),
    queryFn: () => listPages({ filter: "notes", limit: 10_000 }),
  });
  const journalsQuery = useQuery({
    queryKey: queryKeys.journals,
    queryFn: () => listJournals({ limit: 7 }),
  });
  const pages = pagesQuery.data ?? [];
  const recentJournals = journalsQuery.data ?? [];
  const pagesByUuid = useMemo(() => new Map(pages.map((page) => [page.uuid, page])), [pages]);
  const favoritePages = favoritePageUuids.flatMap((uuid) => {
    const page = pagesByUuid.get(uuid);
    return page ? [page] : [];
  });
  const recentPages = recentPageUuids.flatMap((uuid) => {
    const page = pagesByUuid.get(uuid);
    return page ? [page] : [];
  });
  useEffect(() => {
    if (pagesQuery.error) notifyError("pages", pagesQuery.error);
    if (journalsQuery.error) notifyError("journal", journalsQuery.error);
  }, [journalsQuery.error, pagesQuery.error]);

  async function create() {
    if (busy) return;
    setBusy(true);
    try {
      await onCreate();
      await queryClient.invalidateQueries({ queryKey: queryKeys.pages });
    } catch (err) {
      notifyError("pages", err);
    } finally {
      setBusy(false);
    }
  }

  const openPage = (page: Page, shiftKey: boolean) =>
    onSelect(page, dispositionFromShiftKey(shiftKey));

  return (
    <div className="flex h-full min-h-[22rem] flex-col gap-2.5">
      <NewNoteButton
        fullWidth
        onClick={() => void create()}
        disabled={busy}
        className="brand-button h-9 w-full justify-between rounded-lg px-2.5 shadow-none"
      >
        <span className="flex items-center gap-2">
          <Plus className="size-4" />
          New note
        </span>
        <kbd className="rounded-md bg-primary-foreground/15 px-1.5 py-0.5 text-[10px] font-medium">
          Ctrl N
        </kbd>
      </NewNoteButton>

      <JournalNavigation
        activeDate={activeJournalDate}
        busy={journalBusy}
        onOpenDate={onOpenJournal}
      />
      <RecentJournals
        pages={recentJournals}
        activeUuid={selectedUuid}
        busy={journalBusy}
        onOpen={onSelect}
      />
      <JournalQuickCapture busy={journalBusy} onCapture={onQuickCapture} />

      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto pt-1 pr-0.5">
        {favoritePages.length === 0 ? (
          <p className="flex items-center gap-2 px-1 py-1 text-xs text-muted-foreground">
            <Star className="size-3.5 shrink-0" /> No favorites yet
          </p>
        ) : (
          <NoteSection title="Favorites" count={favoritePages.length} icon={Star}>
            {favoritePages.map((page) => (
              <NoteRow
                key={page.uuid}
                page={page}
                selected={selectedUuid === page.uuid}
                favorite
                onOpen={openPage}
                onToggleFavorite={toggleFavoritePage}
              />
            ))}
          </NoteSection>
        )}

        <NoteSection title="Recent" count={recentPages.length} icon={Clock3}>
          {recentPages.length === 0 ? (
            <p className="px-2 py-2 text-[10px] leading-relaxed text-muted-foreground">
              Notes you open will appear here.
            </p>
          ) : (
            recentPages
              .slice(0, 10)
              .map((page) => (
                <NoteRow
                  key={page.uuid}
                  page={page}
                  selected={selectedUuid === page.uuid}
                  favorite={favoritePageUuids.includes(page.uuid)}
                  onOpen={openPage}
                  onToggleFavorite={toggleFavoritePage}
                />
              ))
          )}
        </NoteSection>

        <button
          type="button"
          aria-current={allNotesActive ? "page" : undefined}
          onClick={(event) => onOpenAllNotes(dispositionFromShiftKey(event.shiftKey))}
          className={cn(
            "flex w-full items-center gap-2 rounded-lg px-1 py-2 text-left text-xs font-medium text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground",
            allNotesActive && "bg-sidebar-accent text-sidebar-accent-foreground shadow-sm",
          )}
        >
          <Files className={cn("size-3.5 shrink-0", allNotesActive && "text-primary")} />
          <span className="whitespace-nowrap">All notes</span>
          <span className="ml-auto text-[11px] font-normal tabular-nums">{pages.length}</span>
        </button>
      </div>
    </div>
  );
}

function NoteSection({
  title,
  count,
  icon: Icon,
  children,
}: {
  title: string;
  count: number;
  icon: typeof Star;
  children: React.ReactNode;
}) {
  const id = `sidebar-${title.toLocaleLowerCase().replace(/\s+/g, "-")}`;
  return (
    <section aria-labelledby={id}>
      <div className="flex items-center gap-2 px-1 py-1 text-[11px] font-semibold tracking-wide text-muted-foreground">
        <Icon className="size-3.5" />
        <span id={id}>{title}</span>
        {count > 0 && <span className="ml-auto text-[10px] font-medium tabular-nums">{count}</span>}
      </div>
      <div className="mt-0.5 space-y-0.5">{children}</div>
    </section>
  );
}

function NoteRow({
  page,
  selected,
  favorite,
  detail,
  onOpen,
  onToggleFavorite,
}: {
  page: Page;
  selected: boolean;
  favorite: boolean;
  detail?: string;
  onOpen: (page: Page, shiftKey: boolean) => void;
  onToggleFavorite: (pageUuid: string) => void;
}) {
  return (
    <li
      className={cn(
        "group flex items-center rounded-xl transition hover:bg-sidebar-accent",
        selected && "bg-sidebar-accent text-sidebar-accent-foreground shadow-sm",
      )}
    >
      <button
        type="button"
        onClick={(event) => onOpen(page, event.shiftKey)}
        title={pageDisplayTitle(page)}
        className="flex min-w-0 flex-1 items-center gap-1.5 px-1 py-1.5 text-left"
      >
        <PageIcon
          page={page}
          className={cn("size-3.5 shrink-0 text-muted-foreground", selected && "text-primary")}
        />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[12px] font-medium">{pageDisplayTitle(page)}</span>
          {detail && (
            <span className="mt-0.5 block text-[9px] text-muted-foreground">{detail}</span>
          )}
        </span>
      </button>
      <button
        type="button"
        onClick={() => onToggleFavorite(page.uuid)}
        aria-label={
          favorite
            ? `Remove ${pageDisplayTitle(page)} from favorites`
            : `Add ${pageDisplayTitle(page)} to favorites`
        }
        className={cn(
          "mr-1 flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground opacity-0 transition hover:bg-background/70 hover:text-primary group-hover:opacity-100 focus:opacity-100",
          favorite && "text-primary opacity-100",
        )}
      >
        <Star className={cn("size-3", favorite && "fill-current")} />
      </button>
    </li>
  );
}

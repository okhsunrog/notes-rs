import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FileText, Plus, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { JournalNavigation } from "@/features/journal/journal-navigation";
import { JournalQuickCapture } from "@/features/journal/journal-quick-capture";
import { RecentJournals } from "@/features/journal/recent-journals";
import { pageDisplayTitle } from "@/features/journal/journal-date";
import { listJournals, listPages, type JournalDate, type Page } from "@/lib/api";
import { cn } from "@/lib/utils";
import { queryKeys } from "@/lib/query";

type Props = {
  selectedUuid: string | null;
  activeJournalDate: JournalDate | null;
  onSelect: (page: Page) => void;
  onCreate: () => void | Promise<void>;
  onOpenJournal: (date: JournalDate) => void | Promise<void>;
  onQuickCapture: (markdown: string) => boolean | Promise<boolean>;
  journalBusy: boolean;
  onStatus: (s: string) => void;
};

export function PagesList({
  selectedUuid,
  activeJournalDate,
  onSelect,
  onCreate,
  onOpenJournal,
  onQuickCapture,
  journalBusy,
  onStatus,
}: Props) {
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const queryClient = useQueryClient();
  const pagesQuery = useQuery({
    queryKey: queryKeys.pages,
    queryFn: () => listPages({ filter: "notes" }),
  });
  const journalsQuery = useQuery({
    queryKey: queryKeys.journals,
    queryFn: () => listJournals({ limit: 7 }),
  });
  const pages = pagesQuery.data ?? [];
  const recentJournals = journalsQuery.data ?? [];

  useEffect(() => {
    if (pagesQuery.error) onStatus(`error: ${String(pagesQuery.error)}`);
    if (journalsQuery.error) onStatus(`journal error: ${String(journalsQuery.error)}`);
  }, [journalsQuery.error, onStatus, pagesQuery.error]);

  async function create() {
    if (busy) return;
    setBusy(true);
    try {
      await onCreate();
      await queryClient.invalidateQueries({ queryKey: queryKeys.pages });
    } catch (err) {
      onStatus(`error: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  const visiblePages = pages.filter((page) =>
    pageDisplayTitle(page).toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
  );
  return (
    <div className="flex h-full min-h-[22rem] flex-col gap-3">
      <Button
        type="button"
        onClick={() => void create()}
        disabled={busy}
        className="brand-button h-10 w-full justify-between rounded-xl px-3 shadow-md shadow-primary/15"
      >
        <span className="flex items-center gap-2">
          <Plus className="size-4" />
          New note
        </span>
        <kbd className="rounded-md bg-primary-foreground/15 px-1.5 py-0.5 text-[10px] font-medium">
          Ctrl N
        </kbd>
      </Button>

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

      <div className="relative">
        <Search className="pointer-events-none absolute top-1/2 left-3 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          aria-label="Filter notes"
          placeholder="Filter notes…"
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
          className="h-9 rounded-xl border-transparent bg-sidebar-accent pl-9 shadow-none focus-visible:border-primary/30 focus-visible:ring-primary/15"
        />
      </div>

      <div className="flex items-center justify-between px-1 pt-1">
        <span className="text-[11px] font-semibold tracking-wide text-muted-foreground">NOTES</span>
        <span className="rounded-full bg-sidebar-accent px-2 py-0.5 text-[10px] font-medium text-muted-foreground">
          {pages.length}
        </span>
      </div>

      <ul className="flex flex-1 flex-col gap-1 overflow-y-auto">
        {pages.length === 0 && (
          <li className="rounded-xl border border-dashed border-border/70 px-3 py-5 text-center">
            <FileText className="mx-auto mb-2 size-5 text-muted-foreground/60" />
            <p className="text-xs font-medium">Your notes will live here</p>
            <p className="mt-1 text-[11px] text-muted-foreground">Create one to begin.</p>
          </li>
        )}
        {pages.length > 0 && visiblePages.length === 0 && (
          <li className="px-3 py-5 text-center text-xs text-muted-foreground">No matching notes</li>
        )}
        {visiblePages.map((p) => (
          <li key={p.uuid}>
            <button
              type="button"
              onClick={() => onSelect(p)}
              className={cn(
                "group flex w-full items-center gap-2.5 rounded-xl px-2.5 py-2 text-left transition hover:bg-sidebar-accent",
                selectedUuid === p.uuid &&
                  "bg-sidebar-accent text-sidebar-accent-foreground shadow-sm",
              )}
            >
              <span
                className={cn(
                  "flex size-7 shrink-0 items-center justify-center rounded-lg bg-background/60 text-muted-foreground shadow-sm",
                  selectedUuid === p.uuid && "bg-primary/12 text-primary",
                )}
              >
                <FileText className="size-3.5" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[13px] font-medium">
                  {pageDisplayTitle(p)}
                </span>
                <span className="mt-0.5 block text-[10px] text-muted-foreground">
                  {formatRelativeDate(p.updatedAt)}
                </span>
              </span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function formatRelativeDate(timestamp: number) {
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - timestamp);
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 604800) return `${Math.floor(seconds / 86400)}d ago`;
  return new Date(timestamp * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

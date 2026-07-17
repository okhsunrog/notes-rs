import { CalendarClock } from "lucide-react";
import { pageDisplayTitle } from "./journal-date";
import type { Page } from "@/lib/api";
import { cn } from "@/lib/utils";

type Props = {
  pages: Page[];
  activeUuid: string | null;
  busy: boolean;
  onOpen: (page: Page) => void | Promise<void>;
};

export function RecentJournals({ pages, activeUuid, busy, onOpen }: Props) {
  if (pages.length === 0) return null;

  return (
    <div className="flex gap-1 overflow-x-auto pb-0.5" aria-label="Recent journal days">
      {pages.map((page) => (
        <button
          key={page.uuid}
          type="button"
          disabled={busy}
          onClick={() => void onOpen(page)}
          className={cn(
            "flex shrink-0 items-center gap-1.5 rounded-lg border border-transparent bg-sidebar-accent/45 px-2 py-1 text-[10px] text-muted-foreground transition hover:bg-sidebar-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-50",
            activeUuid === page.uuid && "border-primary/20 bg-primary/8 text-primary",
          )}
          title={pageDisplayTitle(page)}
        >
          <CalendarClock className="size-3" />
          {page.kind.kind === "journal"
            ? formatCompactDate(page.kind.date)
            : pageDisplayTitle(page)}
        </button>
      ))}
    </div>
  );
}

function formatCompactDate(date: string): string {
  return `${date.slice(8, 10)}.${date.slice(5, 7)}`;
}

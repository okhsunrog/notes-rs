import { formatJournalDate, pageDisplayTitle } from "./journal-date";
import type { Page } from "@/lib/api";
import { cn } from "@/lib/utils";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";

type Props = {
  pages: Page[];
  activeUuid: string | null;
  busy: boolean;
  onOpen: (page: Page, disposition?: OpenDisposition) => void | Promise<void>;
};

export function RecentJournals({ pages, activeUuid, busy, onOpen }: Props) {
  if (pages.length === 0) return null;

  return (
    <div className="flex gap-1" aria-label="Recent journal days">
      {pages.map((page) => (
        <button
          key={page.uuid}
          type="button"
          disabled={busy}
          onClick={(event) => void onOpen(page, dispositionFromShiftKey(event.shiftKey))}
          className={cn(
            "flex min-w-0 flex-1 flex-col items-center gap-0.5 rounded-lg border border-transparent bg-sidebar-accent/35 px-0.5 py-1.5 text-muted-foreground transition hover:bg-sidebar-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-50",
            activeUuid === page.uuid && "border-primary/20 bg-primary/8 text-primary",
          )}
          title={pageDisplayTitle(page)}
          aria-label={`Open journal ${pageDisplayTitle(page)}`}
        >
          {page.kind.kind === "journal" ? (
            <>
              <span className="text-[8px] leading-none font-semibold tracking-wide uppercase opacity-70">
                {formatJournalDate(page.kind.date, { weekday: "short" }).slice(0, 2)}
              </span>
              <span className="text-[10px] leading-none font-medium tabular-nums">
                {formatCompactDate(page.kind.date)}
              </span>
            </>
          ) : (
            <span className="max-w-full truncate text-[10px]">{pageDisplayTitle(page)}</span>
          )}
        </button>
      ))}
    </div>
  );
}

function formatCompactDate(date: string): string {
  return `${date.slice(8, 10)}.${date.slice(5, 7)}`;
}

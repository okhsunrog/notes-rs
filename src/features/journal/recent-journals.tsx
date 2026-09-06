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
    <div className="grid grid-cols-7 gap-0.5" aria-label="Recent journal days">
      {pages.map((page) => (
        <button
          key={page.uuid}
          type="button"
          disabled={busy}
          onClick={(event) => void onOpen(page, dispositionFromShiftKey(event.shiftKey))}
          className={cn(
            "flex min-w-0 flex-col items-center gap-1 rounded-md border border-transparent py-1.5 text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-50",
            activeUuid === page.uuid && "border-primary/20 bg-primary/8 text-primary",
          )}
          title={pageDisplayTitle(page)}
          aria-label={`Open journal ${pageDisplayTitle(page)}`}
        >
          {page.kind.kind === "journal" ? (
            <>
              <span className="text-[9px] leading-none">
                {formatJournalDate(page.kind.date, { weekday: "short" }).slice(0, 2)}
              </span>
              <span className="text-xs leading-none font-medium tabular-nums">
                {page.kind.date.slice(8, 10)}
              </span>
            </>
          ) : (
            <span className="max-w-full truncate text-[10px] eink:text-xs">
              {pageDisplayTitle(page)}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}

import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { JournalDate } from "@/lib/api";
import { shiftJournalDate, todayJournalDate } from "./journal-date";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";

type Props = {
  activeDate: JournalDate | null;
  busy: boolean;
  onOpenDate: (date: JournalDate, disposition?: OpenDisposition) => void | Promise<void>;
};

export function JournalNavigation({ activeDate, busy, onOpenDate }: Props) {
  const today = todayJournalDate();
  const navigationDate = activeDate ?? today;

  return (
    <section aria-labelledby="journal-navigation-title" aria-busy={busy} className="space-y-2">
      <div className="flex items-center justify-between px-1">
        <span
          id="journal-navigation-title"
          className="text-[11px] font-semibold tracking-wide text-muted-foreground"
        >
          JOURNAL
        </span>
        <div className="flex items-center gap-0.5">
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            disabled={busy}
            aria-label="Open previous journal day"
            onClick={(event) =>
              void onOpenDate(
                shiftJournalDate(navigationDate, -1),
                dispositionFromShiftKey(event.shiftKey),
              )
            }
          >
            <ChevronLeft className="size-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            disabled={busy}
            aria-label="Open next journal day"
            onClick={(event) =>
              void onOpenDate(
                shiftJournalDate(navigationDate, 1),
                dispositionFromShiftKey(event.shiftKey),
              )
            }
          >
            <ChevronRight className="size-3.5" />
          </Button>
        </div>
      </div>

      <div className="grid grid-cols-[1fr_auto] gap-1.5">
        <Button
          type="button"
          variant={activeDate === today ? "secondary" : "outline"}
          disabled={busy}
          onClick={(event) => void onOpenDate(today, dispositionFromShiftKey(event.shiftKey))}
          className="h-9 justify-start rounded-xl border-border/60 bg-card/45 px-2.5"
        >
          <span className="flex size-6 items-center justify-center rounded-lg bg-primary/10 text-primary">
            <CalendarDays className="size-3.5" />
          </span>
          <span className="text-xs font-medium">Today</span>
          <span className="ml-auto text-[10px] tabular-nums text-muted-foreground">{today}</span>
        </Button>
        <label
          aria-disabled={busy}
          className="relative flex size-9 cursor-pointer items-center justify-center rounded-xl border border-border/60 bg-card/45 text-muted-foreground transition hover:bg-accent hover:text-accent-foreground has-disabled:pointer-events-none has-disabled:cursor-not-allowed has-disabled:opacity-50"
        >
          <CalendarDays className="size-4" />
          <span className="sr-only">Choose journal date</span>
          <input
            type="date"
            value={navigationDate}
            disabled={busy}
            aria-label="Choose journal date"
            onChange={(event) => {
              if (event.currentTarget.value) {
                void onOpenDate(event.currentTarget.value);
              }
            }}
            className="absolute inset-0 cursor-pointer opacity-0"
          />
        </label>
      </div>
    </section>
  );
}

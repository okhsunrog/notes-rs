import { useState } from "react";
import { ArrowUp, CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import type { JournalDate } from "@/lib/api";
import { formatJournalDate, shiftJournalDate } from "./journal-date";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";

type Props = {
  date: JournalDate;
  busy: boolean;
  onCapture: (date: JournalDate, markdown: string, openAfterCapture?: boolean) => Promise<boolean>;
  onOpenAllNotes: () => void;
  onOpenDate: (date: JournalDate, disposition?: OpenDisposition) => void | Promise<void>;
};

/** A read-only projection for a date that does not exist in durable source state yet. */
export function EmptyJournalView({ date, busy, onCapture, onOpenAllNotes, onOpenDate }: Props) {
  const [markdown, setMarkdown] = useState("");

  async function submit() {
    const content = markdown.trim();
    if (!content || busy) return;
    if (await onCapture(date, content, true)) setMarkdown("");
  }

  return (
    <article
      aria-busy={busy}
      className="editor-page mx-auto flex min-h-full max-w-[52rem] flex-col px-5 pt-8 pb-24 sm:px-12 sm:pt-12 lg:px-16"
    >
      <div className="mb-8 flex items-center gap-2 text-[11px] font-medium text-muted-foreground">
        <button type="button" onClick={onOpenAllNotes} className="transition hover:text-foreground">
          All notes
        </button>
        <span>/</span>
        <span>Journal</span>
      </div>

      <div className="mb-2 flex items-center gap-2 text-xs font-semibold tracking-[0.12em] text-primary uppercase">
        <CalendarDays className="size-3.5" />
        Journal · {date}
      </div>
      <div className="flex items-start justify-between gap-4">
        <h1 className="text-[2.6rem] leading-tight font-semibold tracking-[-0.045em]">
          {formatJournalDate(date)}
        </h1>
        <div className="flex shrink-0 items-center rounded-lg border border-border/60 surface-card p-0.5">
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label="Open previous journal day"
            disabled={busy}
            onClick={(event) =>
              void onOpenDate(shiftJournalDate(date, -1), dispositionFromShiftKey(event.shiftKey))
            }
          >
            <ChevronLeft className="size-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label="Open next journal day"
            disabled={busy}
            onClick={(event) =>
              void onOpenDate(shiftJournalDate(date, 1), dispositionFromShiftKey(event.shiftKey))
            }
          >
            <ChevronRight className="size-3.5" />
          </Button>
        </div>
      </div>

      <div className="mt-12 rounded-2xl border border-dashed border-primary/25 surface-card-soft p-5 shadow-sm">
        <p className="text-sm font-medium">Nothing captured for this day yet.</p>
        <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
          This date is only being previewed. It becomes a journal page when you save the first
          entry.
        </p>
        <Textarea
          value={markdown}
          rows={5}
          disabled={busy}
          aria-label={`Write the first entry for journal ${date}`}
          placeholder="Write the first thought for this day…"
          onChange={(event) => setMarkdown(event.currentTarget.value)}
          onKeyDown={(event) => {
            if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
              event.preventDefault();
              void submit();
            }
          }}
          className="mt-4 min-h-32 resize-y rounded-xl surface-base"
        />
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-[10px] text-muted-foreground">Ctrl/⌘ + Enter to save</span>
          <Button
            type="button"
            disabled={busy || markdown.trim().length === 0}
            onClick={() => void submit()}
            className="brand-button rounded-xl"
          >
            Save first entry
            <ArrowUp className="size-4" />
          </Button>
        </div>
      </div>
    </article>
  );
}

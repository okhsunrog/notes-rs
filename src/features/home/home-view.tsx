import { ArrowRight, CalendarDays, FilePlus2, Link2, Search, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import { todayJournalDate } from "@/features/journal/journal-date";
import { SearchCard } from "@/features/search/search-card";
import type { Content, JournalDate } from "@/lib/api";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";

type Props = {
  creating: boolean;
  journalBusy: boolean;
  onCreate: () => void | Promise<void>;
  onOpenJournal: (date: JournalDate, disposition?: OpenDisposition) => void | Promise<void>;
  onOpenContent: (content: Content, disposition?: OpenDisposition) => void | Promise<void>;
};

export function HomeView({ creating, journalBusy, onCreate, onOpenJournal, onOpenContent }: Props) {
  return (
    <div className="mx-auto flex min-h-full max-w-4xl flex-col justify-center px-8 py-16 sm:px-12">
      <div className="max-w-2xl">
        <div className="mb-6 flex size-12 items-center justify-center rounded-2xl bg-primary/12 text-primary shadow-sm ring-1 ring-primary/15">
          <Sparkles className="size-5" />
        </div>
        <p className="mb-3 text-xs font-semibold tracking-[0.18em] text-primary uppercase">
          Your connected notebook
        </p>
        <h2 className="text-4xl leading-[1.08] font-semibold tracking-[-0.045em] sm:text-5xl">
          Turn scattered thoughts into a living knowledge graph.
        </h2>
        <p className="mt-5 max-w-xl text-base leading-relaxed text-muted-foreground">
          Capture an idea, link it to what you already know, and let your workspace reveal the
          connections.
        </p>
        <div className="mt-8 flex flex-wrap gap-2.5">
          <Button
            size="lg"
            disabled={creating}
            onClick={() => void onCreate()}
            className="brand-button h-12 rounded-xl px-5 shadow-lg shadow-primary/20"
          >
            <FilePlus2 className="size-4" />
            Create a new note
            <ArrowRight className="ml-2 size-4" />
          </Button>
          <Button
            size="lg"
            variant="outline"
            disabled={creating || journalBusy}
            onClick={(event) =>
              void onOpenJournal(todayJournalDate(), dispositionFromShiftKey(event.shiftKey))
            }
            className="h-12 rounded-xl border-primary/20 bg-card/65 px-5 shadow-sm"
          >
            <CalendarDays className="size-4 text-primary" />
            Open today
          </Button>
        </div>
        <p className="mt-3 text-xs text-muted-foreground">
          Tip: press <kbd className="rounded border bg-card px-1.5 py-0.5">Ctrl N</kbd> anywhere.
        </p>
      </div>

      <div className="mt-14 grid gap-3 sm:grid-cols-3">
        <Feature icon={<FilePlus2 className="size-4" />} title="Capture">
          Start typing in one click.
        </Feature>
        <Feature icon={<Link2 className="size-4" />} title="Connect">
          Type [[ to link your ideas.
        </Feature>
        <Feature icon={<Search className="size-4" />} title="Rediscover">
          Search by words or meaning.
        </Feature>
      </div>

      <div className="mt-10">
        <SearchCard variant="inline" onOpenContent={onOpenContent} />
      </div>
    </div>
  );
}

function Feature({
  icon,
  title,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-2xl border border-border/60 bg-card/55 p-4 shadow-sm backdrop-blur">
      <span className="mb-3 flex size-8 items-center justify-center rounded-xl bg-primary/10 text-primary">
        {icon}
      </span>
      <p className="text-sm font-semibold">{title}</p>
      <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{children}</p>
    </div>
  );
}

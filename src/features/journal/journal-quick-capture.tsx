import { useState } from "react";
import { ArrowUp, PencilLine } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";

type Props = {
  busy: boolean;
  onCapture: (markdown: string) => boolean | Promise<boolean>;
};

export function JournalQuickCapture({ busy, onCapture }: Props) {
  const [markdown, setMarkdown] = useState("");
  const canSubmit = markdown.trim().length > 0 && !busy;

  async function submit() {
    const next = markdown.trim();
    if (!next || busy) return;
    if (await onCapture(next)) setMarkdown("");
  }

  return (
    <form
      aria-busy={busy}
      className="rounded-xl border border-border/60 bg-card/45 p-2 shadow-sm"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div className="mb-1.5 flex items-center gap-1.5 px-1 text-[10px] font-semibold tracking-wide text-muted-foreground uppercase">
        <PencilLine className="size-3" />
        Quick capture · Today
      </div>
      <div className="flex items-end gap-1.5">
        <Textarea
          value={markdown}
          disabled={busy}
          rows={2}
          aria-label="Capture a block in today's journal"
          placeholder="Write a thought…"
          onChange={(event) => setMarkdown(event.currentTarget.value)}
          onKeyDown={(event) => {
            if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
              event.preventDefault();
              void submit();
            }
          }}
          className="min-h-16 flex-1 resize-none rounded-lg border-0 bg-background/55 px-2.5 py-2 text-xs shadow-none focus-visible:ring-1"
        />
        <Button
          type="submit"
          size="icon-sm"
          disabled={!canSubmit}
          aria-label="Add capture to today's journal"
          className="brand-button shrink-0 rounded-lg"
        >
          <ArrowUp className="size-4" />
        </Button>
      </div>
    </form>
  );
}

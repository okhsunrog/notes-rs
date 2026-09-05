import { useState } from "react";
import { ArrowUp } from "lucide-react";
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
      className="py-1"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div className="mb-1.5 px-1 text-xs font-medium text-muted-foreground">Quick capture</div>
      <div className="relative">
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
          className="min-h-20 w-full resize-none rounded-lg border-border/60 bg-background/55 px-2.5 pt-2 pb-9 text-xs shadow-none focus-visible:ring-1"
        />
        <Button
          type="submit"
          size="icon-sm"
          disabled={!canSubmit}
          aria-label="Add capture to today's journal"
          className="absolute right-1.5 bottom-1.5 size-6 rounded-md shadow-none"
        >
          <ArrowUp className="size-4" />
        </Button>
      </div>
    </form>
  );
}

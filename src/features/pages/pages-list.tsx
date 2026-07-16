import { useCallback, useEffect, useState } from "react";
import { Plus, FileText } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { createPage, listPages, type Node } from "@/lib/api";
import { cn } from "@/lib/utils";

type Props = {
  selectedId: number | null;
  onSelect: (page: Node) => void;
  onStatus: (s: string) => void;
};

export function PagesList({ selectedId, onSelect, onStatus }: Props) {
  const [pages, setPages] = useState<Node[]>([]);
  const [newTitle, setNewTitle] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setPages(await listPages());
    } catch (err) {
      onStatus(`error: ${String(err)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const title = newTitle.trim();
    if (!title || busy) return;
    setBusy(true);
    try {
      const page = await createPage(title);
      setNewTitle("");
      await refresh();
      onSelect(page);
      onStatus(`created page #${page.id}`);
    } catch (err) {
      onStatus(`error: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full flex-col gap-3">
      <form onSubmit={submit} className="flex gap-1">
        <Input
          placeholder="new page title…"
          value={newTitle}
          onChange={(e) => setNewTitle(e.currentTarget.value)}
          disabled={busy}
          className="h-8"
        />
        <Button
          type="submit"
          size="sm"
          aria-label="Create page"
          disabled={busy || !newTitle.trim()}
        >
          <Plus className="size-4" />
        </Button>
      </form>

      <div className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Pages
      </div>

      <ul className="flex flex-1 flex-col gap-0.5 overflow-y-auto">
        {pages.length === 0 && (
          <li className="px-2 py-1 text-sm italic text-muted-foreground">no pages yet</li>
        )}
        {pages.map((p) => (
          <li key={p.id}>
            <button
              type="button"
              onClick={() => onSelect(p)}
              className={cn(
                "flex w-full items-center gap-2 rounded-sm px-2 py-1 text-left text-sm transition hover:bg-accent",
                selectedId === p.id && "bg-accent font-medium",
              )}
            >
              <FileText className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="truncate">{p.title ?? "untitled"}</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

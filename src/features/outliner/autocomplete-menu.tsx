import { Loader2 } from "lucide-react";
import type { Block, Page } from "@/lib/api";

export type AutocompleteItem = {
  /** Stable UUID used as the React key. */
  uuid: string;
  /** Primary text shown in the row. */
  label: string;
  /** Optional secondary text (e.g. block content preview). */
  hint?: string;
};

export function pageToItem(page: Page): AutocompleteItem {
  return { uuid: page.uuid, label: page.title ?? "Untitled" };
}

export function blockToItem(block: Block): AutocompleteItem {
  const preview = block.markdown.replace(/\s+/g, " ").slice(0, 60);
  return { uuid: block.uuid, label: block.uuid, hint: preview };
}

type Props = {
  items: AutocompleteItem[];
  selectedIdx: number;
  loading: boolean;
  query: string;
  emptyLabel: string;
  onPick: (idx: number) => void;
};

export function AutocompleteMenu({
  items,
  selectedIdx,
  loading,
  query,
  emptyLabel,
  onPick,
}: Props) {
  return (
    <div
      className="absolute left-0 top-full z-20 mt-1 w-80 max-w-[90vw] overflow-hidden rounded-md border bg-popover text-sm shadow-md"
      // Prevent the textarea from blurring when clicking an item.
      onMouseDown={(e) => e.preventDefault()}
    >
      {loading && items.length === 0 && (
        <div className="flex items-center gap-2 px-3 py-2 text-muted-foreground">
          <Loader2 className="size-3 animate-spin" />
          searching…
        </div>
      )}
      {!loading && items.length === 0 && (
        <div className="px-3 py-2 text-muted-foreground">
          {query ? <>{emptyLabel}</> : <>type to search…</>}
        </div>
      )}
      {items.length > 0 && (
        <ul className="max-h-64 overflow-y-auto py-1">
          {items.map((it, idx) => (
            <li
              key={it.uuid}
              onClick={() => onPick(idx)}
              className={`flex cursor-pointer flex-col gap-0.5 px-3 py-1.5 ${
                idx === selectedIdx ? "bg-accent text-accent-foreground" : "hover:bg-accent/50"
              }`}
            >
              <span className="truncate font-medium">{it.label}</span>
              {it.hint && <span className="truncate text-xs text-muted-foreground">{it.hint}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

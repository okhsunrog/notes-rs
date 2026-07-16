import { useEffect } from "react";
import { Loader2, Plus } from "lucide-react";
import { createBlock, type Node } from "@/lib/api";
import { BlockNode } from "./block-node";
import { useOutliner } from "./outliner-store";

type Props = {
  parent: Node;
  depth: number;
};

/** Renders the ordered list of children for `parent`. Subscribes to the
 * outliner store and ensures the level is loaded on mount. */
export function BlockChildren({ parent, depth }: Props) {
  const store = useOutliner();
  const blocks = store.getChildren(parent.id);
  const error = store.loadError(parent.id);

  useEffect(() => {
    store.ensureLoaded(parent.id);
  }, [parent.id, store]);

  const addBlock = async () => {
    try {
      const created = await createBlock({
        parentId: parent.id,
        position: null,
        content: "",
        contentJson: null,
      });
      await store.refresh(parent.id);
      store.setEditing(created.id);
    } catch (e) {
      console.error("create block failed", e);
    }
  };

  if (error) {
    return <div className="text-xs text-destructive">load error: {error}</div>;
  }
  if (blocks === undefined) {
    return (
      <div className="flex items-center gap-2 py-1 text-xs text-muted-foreground">
        <Loader2 className="size-3 animate-spin" />
        loading…
      </div>
    );
  }
  if (blocks.length === 0) {
    if (depth === 0) {
      return (
        <div className="rounded-md border border-dashed bg-card/30 p-6 text-center text-sm text-muted-foreground">
          <p>No blocks yet on this page.</p>
          <button
            type="button"
            onClick={() => void addBlock()}
            className="mt-3 inline-flex items-center gap-1 rounded-md border bg-background px-2 py-1 text-xs text-foreground hover:bg-accent"
          >
            <Plus className="size-3" />
            Add first block
          </button>
        </div>
      );
    }
    return null;
  }

  return (
    <>
      <ul className="flex flex-col gap-0.5">
        {blocks.map((b) => (
          <BlockNode key={b.id} block={b} parent={parent} depth={depth} />
        ))}
      </ul>
      {depth === 0 && (
        <button
          type="button"
          onClick={() => void addBlock()}
          className="group mt-3 flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs text-muted-foreground/55 transition hover:bg-primary/5 hover:text-primary"
        >
          <Plus className="size-3.5 transition group-hover:rotate-90" />
          Add another block
          <span className="ml-auto opacity-0 transition group-hover:opacity-70">
            or press Enter
          </span>
        </button>
      )}
    </>
  );
}

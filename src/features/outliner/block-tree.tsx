import { useEffect, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Plus } from "lucide-react";
import { createBlock } from "@/lib/api";
import { listBlockChildren } from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { BlockNode } from "./block-node";
import { useOutliner } from "./outliner-store";

type Props = {
  pageUuid: string;
  parentUuid: string | null;
  depth: number;
  focusFirstBlockRequest?: number;
  emptyTitle?: string;
  emptyActionLabel?: string;
};

/** Renders one ordered sibling list and loads it from the backend cache. */
export function BlockChildren({
  pageUuid,
  parentUuid,
  depth,
  focusFirstBlockRequest = 0,
  emptyTitle = "No blocks yet on this page.",
  emptyActionLabel = "Add first block",
}: Props) {
  const store = useOutliner();
  const queryClient = useQueryClient();
  const handledFocusRequest = useRef(0);
  const containerUuid = parentUuid ?? pageUuid;
  const childrenQuery = useQuery({
    queryKey: queryKeys.children(containerUuid),
    queryFn: () => listBlockChildren(pageUuid, parentUuid),
  });
  const blocks = childrenQuery.data;

  useEffect(() => {
    if (
      focusFirstBlockRequest <= handledFocusRequest.current ||
      store.readOnly ||
      !blocks?.length
    ) {
      return;
    }
    handledFocusRequest.current = focusFirstBlockRequest;
    store.setEditing(blocks[0].uuid);
  }, [blocks, focusFirstBlockRequest, store]);

  const addBlock = async () => {
    try {
      const lastBlock = blocks && blocks.length > 0 ? blocks[blocks.length - 1] : null;
      const created = await createBlock({
        pageUuid,
        parentUuid,
        afterUuid: lastBlock?.uuid ?? null,
      });
      await queryClient.invalidateQueries({ queryKey: queryKeys.children(containerUuid) });
      store.setEditing(created.uuid);
    } catch (e) {
      console.error("create block failed", e);
    }
  };

  if (childrenQuery.error) {
    return (
      <div className="text-xs text-destructive">load error: {childrenQuery.error.message}</div>
    );
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
        <div className="rounded-md border border-dashed surface-card-soft p-6 text-center text-sm text-muted-foreground">
          <p>{emptyTitle}</p>
          {!store.readOnly && (
            <button
              type="button"
              onClick={() => void addBlock()}
              className="mt-3 inline-flex items-center gap-1 rounded-md border bg-background px-2 py-1 text-xs text-foreground hover:bg-accent"
            >
              <Plus className="size-3" />
              {emptyActionLabel}
            </button>
          )}
        </div>
      );
    }
    return null;
  }

  return (
    <>
      <ul className="flex flex-col gap-0.5">
        {blocks.map((block, index) => (
          <BlockNode key={block.uuid} block={block} depth={depth} ordinal={index + 1} />
        ))}
      </ul>
      {depth === 0 && !store.readOnly && (
        <button
          type="button"
          onClick={() => void addBlock()}
          className="group mt-3 flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs text-muted-foreground/55 transition hover:bg-primary/5 hover:text-primary"
        >
          <Plus className="size-3.5 transition group-hover:rotate-90" />
          Add another block
          <span className="reveal-on-hover ml-auto transition">or press Enter</span>
        </button>
      )}
    </>
  );
}

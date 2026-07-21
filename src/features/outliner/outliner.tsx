import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Loader2, Plus } from "lucide-react";
import { type MarkdownOpenHandler, useResolvedAttachmentImages } from "@/features/markdown";
import { PagePresentation } from "@/features/pages/page-presentation";
import { createBlock, getPageRenderSnapshot, type Block, type Page } from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { BlockNode } from "./block-node";
import { OutlinerProvider, useOutliner } from "./outliner-store";

type Props = {
  page: Page;
  initialEditingUuid?: string | null;
  focusRequest?: number;
  onOpenMarkdownLink: MarkdownOpenHandler;
  presentation?: PagePresentation;
  readOnly?: boolean;
};

type FlatBlock = { block: Block; depth: number; ordinal: number };

export function Outliner({
  page,
  initialEditingUuid = null,
  focusRequest = 0,
  onOpenMarkdownLink,
  presentation = PagePresentation.Editing,
  readOnly = presentation === PagePresentation.Reading,
}: Props) {
  const snapshotQuery = useQuery({
    queryKey: queryKeys.pageRender(page.uuid),
    queryFn: () => getPageRenderSnapshot(page.uuid),
  });
  const snapshot = snapshotQuery.data;
  const resolveImage = useResolvedAttachmentImages(snapshot?.images ?? []);

  if (snapshotQuery.isPending) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" />
        Loading outline…
      </div>
    );
  }
  if (snapshotQuery.error) {
    return (
      <div className="text-sm text-destructive">load error: {snapshotQuery.error.message}</div>
    );
  }
  if (!snapshot) {
    return <div className="text-sm text-muted-foreground">This page is no longer available.</div>;
  }

  return (
    <OutlinerProvider
      key={page.uuid}
      blocks={snapshot.document.blocks}
      initialEditingUuid={initialEditingUuid}
      initialEditingRequest={focusRequest}
      layout={page.layout}
      onOpenMarkdownLink={onOpenMarkdownLink}
      readOnly={readOnly}
      resolveImage={resolveImage}
    >
      <VirtualOutline
        blocks={snapshot.document.blocks}
        page={page}
        focusFirstBlockRequest={initialEditingUuid === null ? focusRequest : 0}
      />
    </OutlinerProvider>
  );
}

function VirtualOutline({
  blocks,
  page,
  focusFirstBlockRequest,
}: {
  blocks: readonly Block[];
  page: Page;
  focusFirstBlockRequest: number;
}) {
  const store = useOutliner();
  const queryClient = useQueryClient();
  const rootRef = useRef<HTMLUListElement>(null);
  const handledFocusRequest = useRef(0);
  const [scrollElement, setScrollElement] = useState<HTMLElement | null>(null);
  const [scrollMargin, setScrollMargin] = useState(0);
  const rows = useMemo(
    () => flattenBlocks(blocks, store.layout === "outline" ? store.isCollapsed : () => false),
    [blocks, store.isCollapsed, store.layout],
  );

  useLayoutEffect(() => {
    const root = rootRef.current;
    const scroll = root?.closest<HTMLElement>("[data-workspace-scroll]") ?? null;
    setScrollElement(scroll);
    if (!root || !scroll) return;
    const update = () => {
      const rootRect = root.getBoundingClientRect();
      const scrollRect = scroll.getBoundingClientRect();
      setScrollMargin(rootRect.top - scrollRect.top + scroll.scrollTop);
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(scroll);
    observer.observe(root);
    window.addEventListener("resize", update);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, [page.uuid]);

  useLayoutEffect(() => {
    const grouped = groupChildren(blocks, page.uuid);
    for (const [containerUuid, children] of grouped) {
      queryClient.setQueryData(queryKeys.children(containerUuid), children);
    }
  }, [blocks, page.uuid, queryClient]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    estimateSize: (index) => estimateBlockHeight(rows[index]?.block),
    getItemKey: (index) => rows[index]?.block.uuid ?? index,
    getScrollElement: () => scrollElement,
    // Keep enough neighbours for smooth keyboard/trackpad scrolling without
    // eagerly decoding a whole run of image blocks just below the viewport.
    overscan: 4,
    scrollMargin,
  });

  useEffect(() => {
    if (
      focusFirstBlockRequest <= handledFocusRequest.current ||
      store.readOnly ||
      rows.length === 0
    ) {
      return;
    }
    handledFocusRequest.current = focusFirstBlockRequest;
    store.setEditing(rows[0].block.uuid);
  }, [focusFirstBlockRequest, rows, store]);

  useEffect(() => {
    if (!store.editingUuid) return;
    const index = rows.findIndex(({ block }) => block.uuid === store.editingUuid);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
  }, [rows, store.editingUuid, virtualizer]);

  const addRootBlock = async () => {
    const roots = blocks.filter((block) => block.parentUuid === null);
    const last = roots.length > 0 ? roots[roots.length - 1] : null;
    await createBlock({
      pageUuid: page.uuid,
      parentUuid: null,
      afterUuid: last?.uuid ?? null,
    });
    await queryClient.invalidateQueries({ queryKey: queryKeys.pageRender(page.uuid) });
  };

  if (blocks.length === 0) {
    return (
      <div className="rounded-md border border-dashed bg-card/30 p-6 text-center text-sm text-muted-foreground">
        <p>
          {page.kind.kind === "journal"
            ? "Nothing captured for this day yet."
            : "No blocks yet on this page."}
        </p>
        {!store.readOnly && (
          <button
            type="button"
            onClick={() => void addRootBlock()}
            className="mt-3 inline-flex items-center gap-1 rounded-md border bg-background px-2 py-1 text-xs text-foreground hover:bg-accent"
          >
            <Plus className="size-3" />
            {page.kind.kind === "journal" ? "Start writing" : "Add first block"}
          </button>
        )}
      </div>
    );
  }

  return (
    <>
      <ul
        ref={rootRef}
        className="relative"
        style={{ height: virtualizer.getTotalSize(), contain: "layout style" }}
      >
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index];
          if (!row) return null;
          return (
            <BlockNode
              key={row.block.uuid}
              block={row.block}
              depth={row.depth}
              ordinal={row.ordinal}
              measureRef={virtualizer.measureElement}
              virtualIndex={item.index}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                paddingInlineStart: `${row.depth * 1.75}rem`,
                transform: `translateY(${item.start - virtualizer.options.scrollMargin}px)`,
              }}
            />
          );
        })}
      </ul>
      {!store.readOnly && (
        <button
          type="button"
          onClick={() => void addRootBlock()}
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

export function flattenBlocks(
  blocks: readonly Block[],
  isCollapsed: (uuid: string) => boolean,
): FlatBlock[] {
  const children = new Map<string, Block[]>();
  for (const block of blocks) {
    const key = block.parentUuid ?? block.pageUuid;
    const siblings = children.get(key) ?? [];
    siblings.push(block);
    children.set(key, siblings);
  }
  for (const siblings of children.values()) {
    siblings.sort((left, right) => left.orderKey.localeCompare(right.orderKey));
  }
  const rows: FlatBlock[] = [];
  const visited = new Set<string>();
  const append = (siblings: readonly Block[], depth: number) => {
    siblings.forEach((block, index) => {
      if (visited.has(block.uuid)) return;
      visited.add(block.uuid);
      rows.push({ block, depth, ordinal: index + 1 });
      if (!isCollapsed(block.uuid)) append(children.get(block.uuid) ?? [], depth + 1);
    });
  };
  const pageUuid = blocks[0]?.pageUuid;
  if (pageUuid) append(children.get(pageUuid) ?? [], 0);
  return rows;
}

function groupChildren(blocks: readonly Block[], pageUuid: string): Map<string, Block[]> {
  const grouped = new Map<string, Block[]>();
  grouped.set(pageUuid, []);
  for (const block of blocks) {
    grouped.set(block.uuid, grouped.get(block.uuid) ?? []);
    const key = block.parentUuid ?? pageUuid;
    const siblings = grouped.get(key) ?? [];
    siblings.push(block);
    grouped.set(key, siblings);
  }
  for (const siblings of grouped.values()) {
    siblings.sort((left, right) => left.orderKey.localeCompare(right.orderKey));
  }
  return grouped;
}

function estimateBlockHeight(block?: Block): number {
  if (!block) return 42;
  if (block.style.kind === "divider") return 34;
  if (block.style.kind.startsWith("heading_")) return 58;
  const imageCount = block.markdown.match(/notes-attachment:/g)?.length ?? 0;
  if (imageCount > 0) {
    // Inline previews are capped at 70vh. A realistic initial estimate keeps
    // the virtualizer from mounting and decoding many image rows before their
    // measured heights arrive.
    return Math.min(2_400, 42 + imageCount * 560);
  }
  const lines = Math.max(1, Math.ceil(block.markdown.length / 80));
  return Math.min(240, 38 + (lines - 1) * 24);
}

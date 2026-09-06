import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, RefreshCw, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ChatCard } from "@/features/chat/chat-card";
import type { AssistantController } from "@/features/chat/use-assistant-controller";
import { pageDisplayTitle } from "@/features/journal/journal-date";
import type { MarkdownOpenHandler } from "@/features/markdown";
import {
  contentText,
  contentUuid,
  findBacklinks,
  getBlock,
  getGraphSnapshot,
  getPage,
  type Content,
  type GraphItem,
  type GraphSnapshot,
  type Page,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";

type PanelProps = {
  page: Page | null;
  onOpenMarkdownLink: MarkdownOpenHandler;
  controller: AssistantController;
};

type GraphProps = {
  page: Page | null;
  onOpenContent: (content: Content, disposition?: OpenDisposition) => void | Promise<void>;
};

export function KnowledgePanel({ page, onOpenMarkdownLink, controller }: PanelProps) {
  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex shrink-0 items-center gap-2 px-1 pt-1">
        <span className="flex size-7 items-center justify-center rounded-xl bg-primary/10 text-primary">
          <Sparkles className="size-3.5" />
        </span>
        <div>
          <p className="text-xs font-semibold">Knowledge companion</p>
          <p className="text-[10px] eink:text-xs text-muted-foreground">Context-aware tools</p>
        </div>
      </div>
      <div className="min-h-0 flex-1 pt-1">
        <ChatCard page={page} onOpenMarkdownLink={onOpenMarkdownLink} controller={controller} />
      </div>
    </div>
  );
}

export function GraphWorkspace({
  page,
  onOpenContent,
  onClose,
}: GraphProps & { onClose: () => void }) {
  const graphQuery = useQuery({
    queryKey: queryKeys.graph(page?.uuid ?? null),
    queryFn: () => getGraphSnapshot(page?.uuid ?? null),
  });
  const backlinksQuery = useQuery({
    queryKey: queryKeys.backlinks(page?.uuid ?? "inactive"),
    queryFn: () => findBacklinks((page as Page).uuid),
    enabled: page !== null,
  });
  const snapshot = graphQuery.data ?? { items: [], edges: [] };
  const backlinks = backlinksQuery.data ?? [];
  const error = graphQuery.error ?? backlinksQuery.error;

  return (
    <div className="graph-workspace flex h-full min-h-0 flex-col bg-canvas">
      <div className="flex h-16 shrink-0 items-center gap-3 border-b border-border/60 px-5">
        <Button variant="ghost" size="icon-sm" aria-label="Back to notes" onClick={onClose}>
          <ArrowLeft className="size-4" />
        </Button>
        <div>
          <h2 className="text-sm font-semibold">Knowledge graph</h2>
          <p className="text-xs text-muted-foreground">
            {page ? `Focused around ${pageDisplayTitle(page)}` : "All pages and blocks"}
          </p>
        </div>
        <Button
          className="ml-auto"
          variant="ghost"
          size="sm"
          onClick={() => void Promise.all([graphQuery.refetch(), backlinksQuery.refetch()])}
        >
          <RefreshCw className="size-3.5" />
          Refresh
        </Button>
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="relative min-w-0 flex-1 overflow-hidden">
          {error && (
            <p className="absolute top-4 left-4 z-10 rounded-lg bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {String(error)}
            </p>
          )}
          <GraphView
            snapshot={snapshot}
            focusUuid={page?.uuid ?? null}
            onOpenItem={async (item, disposition) => {
              if (item.kind === "page") {
                const record = await getPage(item.uuid);
                if (record) await onOpenContent({ kind: "page", record }, disposition);
              } else {
                const record = await getBlock(item.uuid);
                if (record) await onOpenContent({ kind: "block", record }, disposition);
              }
            }}
          />
          <div className="pointer-events-none absolute bottom-5 left-5 flex gap-3 rounded-xl border border-border/60 surface-glass px-3 py-2 text-[10px] eink:text-xs text-muted-foreground shadow-sm">
            <span>
              <i className="mr-1 inline-block size-2 rounded-full bg-sky-500" /> Page
            </span>
            <span>
              <i className="mr-1 inline-block size-2 rounded-full bg-amber-500" /> Block
            </span>
            <span>Click an item to open it</span>
          </div>
        </div>
        <aside className="w-72 shrink-0 overflow-y-auto border-l border-border/60 bg-sidebar/65 p-4">
          <h3 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            Backlinks
          </h3>
          <p className="mt-1 text-xs text-muted-foreground">
            {page
              ? `Incoming links to ${pageDisplayTitle(page)}`
              : "Open a note before entering the graph to inspect its backlinks."}
          </p>
          {page && backlinks.length === 0 && (
            <p className="mt-4 text-xs text-muted-foreground">No incoming links.</p>
          )}
          <ul className="mt-4 space-y-2">
            {backlinks.map((backlink) => (
              <li key={contentUuid(backlink)}>
                <button
                  type="button"
                  onClick={(event) =>
                    void onOpenContent(backlink, dispositionFromShiftKey(event.shiftKey))
                  }
                  className="w-full rounded-xl border border-border/60 surface-card px-3 py-2.5 text-left text-xs transition hover:border-primary/25 hover:bg-primary/5"
                >
                  <span className="font-medium">
                    {contentText(backlink).slice(0, 48) || "Untitled"}
                  </span>
                  <span className="ml-1 text-muted-foreground">{backlink.kind}</span>
                </button>
              </li>
            ))}
          </ul>
        </aside>
      </div>
    </div>
  );
}

function GraphView({
  snapshot,
  focusUuid,
  onOpenItem,
}: {
  snapshot: GraphSnapshot;
  focusUuid: string | null;
  onOpenItem: (item: GraphItem, disposition: OpenDisposition) => void | Promise<void>;
}) {
  const positioned = useMemo(() => {
    const count = snapshot.items.length;
    return snapshot.items.map((item, index) => {
      const angle = count <= 1 ? 0 : (index / count) * Math.PI * 2 - Math.PI / 2;
      const radiusX = count <= 1 ? 0 : Math.min(430, 150 + count * 30);
      const radiusY = count <= 1 ? 0 : Math.min(250, 90 + count * 18);
      return {
        item,
        x: 600 + Math.cos(angle) * radiusX,
        y: 350 + Math.sin(angle) * radiusY,
      };
    });
  }, [snapshot.items]);
  const byUuid = new Map(positioned.map((position) => [position.item.uuid, position]));

  if (positioned.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        Graph is empty.
      </div>
    );
  }

  return (
    <svg
      viewBox="0 0 1200 700"
      preserveAspectRatio="xMidYMid meet"
      role="img"
      aria-label="Knowledge graph"
      className="h-full w-full surface-card-soft"
    >
      {snapshot.edges.map((edge) => {
        const source = byUuid.get(edge.sourceUuid);
        const target = byUuid.get(edge.targetUuid);
        return source && target ? (
          <line
            key={`${edge.sourceUuid}-${edge.targetUuid}-${edge.relation}`}
            x1={source.x}
            y1={source.y}
            x2={target.x}
            y2={target.y}
            className="stroke-border"
            strokeWidth="2"
          >
            <title>{edge.relation}</title>
          </line>
        ) : null;
      })}
      {positioned.map(({ item, x, y }) => (
        <g
          key={item.uuid}
          role="button"
          tabIndex={0}
          aria-label={item.label}
          onClick={(event) => void onOpenItem(item, dispositionFromShiftKey(event.shiftKey))}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              void onOpenItem(item, dispositionFromShiftKey(event.shiftKey));
            }
          }}
          className="cursor-pointer outline-none"
        >
          <circle
            cx={x}
            cy={y}
            r={item.uuid === focusUuid ? 18 : 13}
            className={item.kind === "block" ? "fill-amber-500" : "fill-sky-500"}
          />
          <text
            x={x}
            y={y + 34}
            textAnchor="middle"
            className="fill-foreground text-[14px] font-medium"
          >
            {item.label.slice(0, 28)}
          </text>
        </g>
      ))}
    </svg>
  );
}

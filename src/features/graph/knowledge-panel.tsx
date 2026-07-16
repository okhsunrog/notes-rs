import { useEffect, useMemo, useState } from "react";
import { GitFork, MessageCircle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ChatCard } from "@/features/chat/chat-card";
import { findBacklinks, getGraphSnapshot, type GraphSnapshot, type Node } from "@/lib/api";

type Props = {
  node: Node | null;
  onOpenNode: (node: Node) => void | Promise<void>;
};

export function KnowledgePanel({ node, onOpenNode }: Props) {
  const [tab, setTab] = useState<"chat" | "graph">("chat");
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex shrink-0 gap-1 rounded-md bg-muted p-1">
        <TabButton active={tab === "chat"} onClick={() => setTab("chat")}>
          <MessageCircle className="size-3.5" /> Chat
        </TabButton>
        <TabButton active={tab === "graph"} onClick={() => setTab("graph")}>
          <GitFork className="size-3.5" /> Graph
        </TabButton>
      </div>
      <div className={tab === "chat" ? "min-h-0 flex-1" : "hidden"}>
        <ChatCard />
      </div>
      {tab === "graph" && (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <GraphContext node={node} onOpenNode={onOpenNode} />
        </div>
      )}
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex flex-1 items-center justify-center gap-1 rounded px-2 py-1 text-xs ${active ? "bg-background shadow-sm" : "text-muted-foreground hover:text-foreground"}`}
    >
      {children}
    </button>
  );
}

function GraphContext({ node, onOpenNode }: Props) {
  const [snapshot, setSnapshot] = useState<GraphSnapshot>({ nodes: [], edges: [] });
  const [backlinks, setBacklinks] = useState<Node[]>([]);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);

  useEffect(() => {
    let cancelled = false;
    Promise.all([getGraphSnapshot(node?.id ?? null), node ? findBacklinks(node.id) : []])
      .then(([graph, incoming]) => {
        if (!cancelled) {
          setSnapshot(graph);
          setBacklinks(incoming);
          setError("");
        }
      })
      .catch((reason) => !cancelled && setError(String(reason)));
    return () => {
      cancelled = true;
    };
  }, [node, version]);

  return (
    <div className="space-y-4 py-2">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-sm font-semibold">Knowledge graph</h2>
          <p className="text-xs text-muted-foreground">
            {node ? `Around #${node.id}` : "Recent pages and entities"}
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          aria-label="Refresh graph"
          onClick={() => setVersion((value) => value + 1)}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </div>
      {error && <p className="text-xs text-destructive">{error}</p>}
      <GraphView snapshot={snapshot} focusId={node?.id ?? null} onOpenNode={onOpenNode} />
      <section>
        <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Backlinks
        </h3>
        {node === null ? (
          <p className="text-xs text-muted-foreground">Select a page to inspect backlinks.</p>
        ) : backlinks.length === 0 ? (
          <p className="text-xs text-muted-foreground">No incoming links.</p>
        ) : (
          <ul className="space-y-1">
            {backlinks.map((backlink) => (
              <li key={backlink.id}>
                <button
                  type="button"
                  onClick={() => void onOpenNode(backlink)}
                  className="w-full rounded border px-2 py-1.5 text-left text-xs hover:bg-accent"
                >
                  <span className="font-medium">
                    {backlink.title ?? (backlink.content.slice(0, 48) || `#${backlink.id}`)}
                  </span>
                  <span className="ml-1 text-muted-foreground">#{backlink.id}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function GraphView({
  snapshot,
  focusId,
  onOpenNode,
}: {
  snapshot: GraphSnapshot;
  focusId: number | null;
  onOpenNode: Props["onOpenNode"];
}) {
  const positioned = useMemo(() => {
    const count = snapshot.nodes.length;
    return snapshot.nodes.map((node, index) => {
      const angle = count <= 1 ? 0 : (index / count) * Math.PI * 2 - Math.PI / 2;
      const radius = count <= 1 ? 0 : 38;
      return { node, x: 50 + Math.cos(angle) * radius, y: 50 + Math.sin(angle) * radius };
    });
  }, [snapshot.nodes]);
  const byId = new Map(positioned.map((item) => [item.node.id, item]));
  if (positioned.length === 0)
    return (
      <div className="rounded border border-dashed p-5 text-center text-xs text-muted-foreground">
        Graph is empty.
      </div>
    );
  return (
    <svg
      viewBox="0 0 100 100"
      role="img"
      aria-label="Knowledge graph"
      className="aspect-square w-full rounded-md border bg-muted/20"
    >
      {snapshot.edges.map((edge) => {
        const source = byId.get(edge.src);
        const target = byId.get(edge.dst);
        return source && target ? (
          <line
            key={`${edge.src}-${edge.dst}-${edge.kind}`}
            x1={source.x}
            y1={source.y}
            x2={target.x}
            y2={target.y}
            className="stroke-border"
            strokeWidth="0.8"
          >
            <title>{edge.kind}</title>
          </line>
        ) : null;
      })}
      {positioned.map(({ node, x, y }) => (
        <g
          key={node.id}
          role="button"
          tabIndex={0}
          aria-label={node.title ?? `Node ${node.id}`}
          onClick={() => void onOpenNode(node)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") void onOpenNode(node);
          }}
          className="cursor-pointer"
        >
          <circle
            cx={x}
            cy={y}
            r={node.id === focusId ? 5 : 3.7}
            className={
              node.kind === "entity"
                ? "fill-amber-500"
                : node.id === focusId
                  ? "fill-foreground"
                  : "fill-sky-500"
            }
          />
          <text x={x} y={y + 7} textAnchor="middle" className="fill-foreground text-[3.2px]">
            {(node.title ?? `#${node.id}`).slice(0, 16)}
          </text>
        </g>
      ))}
    </svg>
  );
}

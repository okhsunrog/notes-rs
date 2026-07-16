import { useEffect, useMemo, useState } from "react";
import { ArrowLeft, RefreshCw, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ChatCard } from "@/features/chat/chat-card";
import { findBacklinks, getGraphSnapshot, type GraphSnapshot, type Node } from "@/lib/api";

type Props = {
  node: Node | null;
  onOpenNode: (node: Node) => void | Promise<void>;
};

export function KnowledgePanel({ node }: Props) {
  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex shrink-0 items-center gap-2 px-1 pt-1">
        <span className="flex size-7 items-center justify-center rounded-xl bg-primary/10 text-primary">
          <Sparkles className="size-3.5" />
        </span>
        <div>
          <p className="text-xs font-semibold">Knowledge companion</p>
          <p className="text-[10px] text-muted-foreground">Context-aware tools</p>
        </div>
      </div>
      <div className="min-h-0 flex-1 pt-1">
        <ChatCard node={node} />
      </div>
    </div>
  );
}

export function GraphWorkspace({ node, onOpenNode, onClose }: Props & { onClose: () => void }) {
  const [snapshot, setSnapshot] = useState<GraphSnapshot>({ nodes: [], edges: [] });
  const [backlinks, setBacklinks] = useState<Node[]>([]);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);

  useEffect(() => {
    let cancelled = false;
    Promise.all([getGraphSnapshot(null), node ? findBacklinks(node.id) : []])
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
    <div className="graph-workspace flex h-full min-h-0 flex-col bg-canvas">
      <div className="flex h-16 shrink-0 items-center gap-3 border-b border-border/60 px-5">
        <Button variant="ghost" size="icon-sm" aria-label="Back to notes" onClick={onClose}>
          <ArrowLeft className="size-4" />
        </Button>
        <div>
          <h2 className="text-sm font-semibold">Knowledge graph</h2>
          <p className="text-xs text-muted-foreground">
            {node ? `Focused around ${node.title ?? `node #${node.id}`}` : "All pages and entities"}
          </p>
        </div>
        <Button
          className="ml-auto"
          variant="ghost"
          size="sm"
          onClick={() => setVersion((value) => value + 1)}
        >
          <RefreshCw className="size-3.5" />
          Refresh
        </Button>
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="relative min-w-0 flex-1 overflow-hidden">
          {error && (
            <p className="absolute top-4 left-4 z-10 rounded-lg bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {error}
            </p>
          )}
          <GraphView snapshot={snapshot} focusId={node?.id ?? null} onOpenNode={onOpenNode} />
          <div className="pointer-events-none absolute bottom-5 left-5 flex gap-3 rounded-xl border border-border/60 bg-card/80 px-3 py-2 text-[10px] text-muted-foreground shadow-sm backdrop-blur">
            <span>
              <i className="mr-1 inline-block size-2 rounded-full bg-sky-500" /> Page
            </span>
            <span>
              <i className="mr-1 inline-block size-2 rounded-full bg-amber-500" /> Entity
            </span>
            <span>Click a node to open it</span>
          </div>
        </div>
        <aside className="w-72 shrink-0 overflow-y-auto border-l border-border/60 bg-sidebar/65 p-4">
          <h3 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            Backlinks
          </h3>
          <p className="mt-1 text-xs text-muted-foreground">
            {node
              ? `Incoming links to #${node.id}`
              : "Open a note before entering the graph to inspect its backlinks."}
          </p>
          {node && backlinks.length === 0 && (
            <p className="mt-4 text-xs text-muted-foreground">No incoming links.</p>
          )}
          <ul className="mt-4 space-y-2">
            {backlinks.map((backlink) => (
              <li key={backlink.id}>
                <button
                  type="button"
                  onClick={() => void onOpenNode(backlink)}
                  className="w-full rounded-xl border border-border/60 bg-card/55 px-3 py-2.5 text-left text-xs transition hover:border-primary/25 hover:bg-primary/5"
                >
                  <span className="font-medium">
                    {backlink.title ?? (backlink.content.slice(0, 48) || `#${backlink.id}`)}
                  </span>
                  <span className="ml-1 text-muted-foreground">#{backlink.id}</span>
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
      const radiusX = count <= 1 ? 0 : Math.min(430, 150 + count * 30);
      const radiusY = count <= 1 ? 0 : Math.min(250, 90 + count * 18);
      return {
        node,
        x: 600 + Math.cos(angle) * radiusX,
        y: 350 + Math.sin(angle) * radiusY,
      };
    });
  }, [snapshot.nodes]);
  const byId = new Map(positioned.map((item) => [item.node.id, item]));

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
      className="h-full w-full bg-card/20"
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
            strokeWidth="2"
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
          className="cursor-pointer outline-none"
        >
          <circle
            cx={x}
            cy={y}
            r={node.id === focusId ? 18 : 13}
            className={
              node.kind === "entity"
                ? "fill-amber-500"
                : node.id === focusId
                  ? "fill-foreground"
                  : "fill-sky-500"
            }
          />
          <text
            x={x}
            y={y + 34}
            textAnchor="middle"
            className="fill-foreground text-[14px] font-medium"
          >
            {(node.title ?? `#${node.id}`).slice(0, 28)}
          </text>
        </g>
      ))}
    </svg>
  );
}

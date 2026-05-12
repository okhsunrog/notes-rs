import { useState } from "react";
import { AppLayout } from "@/app/layout";
import { SearchCard } from "@/features/search/search-card";
import { EntitiesCard } from "@/features/entities/entities-card";
import { ChatCard } from "@/features/chat/chat-card";
import { PagesList } from "@/features/pages/pages-list";
import { PageView } from "@/features/pages/page-view";
import type { Node, SearchHit } from "@/lib/api";

function App() {
  const [status, setStatus] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [activeNode, setActiveNode] = useState<Node | null>(null);

  function applyUpdated(updated: Node) {
    setHits((hs) => hs.map((h) => (h.node.id === updated.id ? { ...h, node: updated } : h)));
    if (activeNode && activeNode.id === updated.id) {
      setActiveNode(updated);
    }
  }

  return (
    <AppLayout
      status={status}
      sidebar={
        <div className="flex h-full flex-col gap-4">
          <PagesList
            selectedId={activeNode?.id ?? null}
            onSelect={setActiveNode}
            onStatus={setStatus}
          />
          <EntitiesCard variant="compact" />
        </div>
      }
      center={
        activeNode ? (
          <PageView
            node={activeNode}
            onSaved={applyUpdated}
            onStatus={setStatus}
            onClose={() => setActiveNode(null)}
          />
        ) : (
          <div className="mx-auto max-w-3xl space-y-4">
            <div className="rounded-md border border-dashed bg-card/50 p-8 text-center text-sm text-muted-foreground">
              Select a page on the left, or create one — then start writing.
            </div>
            <SearchCard
              hits={hits}
              setHits={setHits}
              onOpenNode={setActiveNode}
              onStatus={setStatus}
            />
          </div>
        )
      }
      right={<ChatCard />}
    />
  );
}

export default App;

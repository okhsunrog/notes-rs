import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader2 } from "lucide-react";
import { AppLayout } from "@/app/layout";
import { Toaster } from "@/components/ui/sonner";
import { SearchCard } from "@/features/search/search-card";
import { EntitiesCard } from "@/features/entities/entities-card";
import { ChatCard } from "@/features/chat/chat-card";
import { PagesList } from "@/features/pages/pages-list";
import { PageView } from "@/features/pages/page-view";
import { isReady, type Node, type SearchHit } from "@/lib/api";

function App() {
  const [ready, setReady] = useState(false);
  const [status, setStatus] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [activeNode, setActiveNode] = useState<Node | null>(null);

  useEffect(() => {
    let cancelled = false;
    isReady()
      .then((r) => {
        if (!cancelled && r) setReady(true);
      })
      .catch(() => {
        /* startup state not yet registered: keep listening */
      });
    const unlistenPromise = listen("app:ready", () => {
      if (!cancelled) setReady(true);
    });
    return () => {
      cancelled = true;
      void unlistenPromise.then((un) => un());
    };
  }, []);

  function applyUpdated(updated: Node) {
    setHits((hs) => hs.map((h) => (h.node.id === updated.id ? { ...h, node: updated } : h)));
    if (activeNode && activeNode.id === updated.id) {
      setActiveNode(updated);
    }
  }

  if (!ready) {
    return (
      <div className="flex h-screen flex-col items-center justify-center gap-3 bg-background text-foreground">
        <Loader2 className="size-6 animate-spin text-muted-foreground" />
        <p className="text-sm text-muted-foreground">
          starting notes-rs…
          <br />
          first launch downloads the embedder (~2.3 GB)
        </p>
      </div>
    );
  }

  return (
    <>
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
      <Toaster position="bottom-right" />
    </>
  );
}

export default App;

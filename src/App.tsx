import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader2, Settings } from "lucide-react";
import { AppLayout } from "@/app/layout";
import { WindowControls } from "@/app/window-controls";
import { Toaster } from "@/components/ui/sonner";
import { SearchCard } from "@/features/search/search-card";
import { EntitiesCard } from "@/features/entities/entities-card";
import { ChatCard } from "@/features/chat/chat-card";
import { PagesList } from "@/features/pages/pages-list";
import { PageView } from "@/features/pages/page-view";
import { SettingsPage } from "@/features/settings/settings-page";
import { Button } from "@/components/ui/button";
import {
  getContainingPage,
  getStartupStatus,
  loadSettings,
  type Node,
  type SearchHit,
} from "@/lib/api";

function App() {
  const [ready, setReady] = useState(false);
  const [startupError, setStartupError] = useState("");
  const [status, setStatus] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [activeNode, setActiveNode] = useState<Node | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [windowDecorationMode, setWindowDecorationMode] = useState<"native" | "borderless" | "kde">(
    "native",
  );

  useEffect(() => {
    loadSettings()
      .then((settings) => setWindowDecorationMode(settings.windowDecorationMode))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    let cancelled = false;
    getStartupStatus()
      .then((result) => {
        if (cancelled) return;
        if (result.state === "ready") setReady(true);
        if (result.state === "error") setStartupError(result.message);
      })
      .catch(() => {
        /* startup state not yet registered: keep listening */
      });
    const unlistenPromise = listen("app:ready", () => {
      if (!cancelled) setReady(true);
    });
    const unlistenError = listen<string>("app:startup-error", ({ payload }) => {
      if (!cancelled) setStartupError(payload);
    });
    return () => {
      cancelled = true;
      void unlistenPromise.then((un) => un());
      void unlistenError.then((un) => un());
    };
  }, []);

  function applyUpdated(updated: Node) {
    setHits((hs) => hs.map((h) => (h.node.id === updated.id ? { ...h, node: updated } : h)));
    if (activeNode && activeNode.id === updated.id) {
      setActiveNode(updated);
    }
  }

  async function openSearchResult(node: Node) {
    try {
      const page = node.kind === "page" ? node : await getContainingPage(node.id);
      if (!page) {
        setStatus(`No containing page found for #${node.id}`);
        return;
      }
      setActiveNode(page);
      if (page.id !== node.id) {
        setStatus(`Opened ${page.title ?? "page"} containing #${node.id}`);
      }
    } catch (error) {
      setStatus(`open error: ${String(error)}`);
    }
  }

  if (settingsOpen) {
    return (
      <SettingsPage
        onBack={() => setSettingsOpen(false)}
        onDecorationModeChanged={setWindowDecorationMode}
      />
    );
  }

  if (!ready) {
    return (
      <div className="flex h-screen flex-col items-center justify-center gap-3 bg-background text-foreground">
        {windowDecorationMode === "borderless" && (
          <div
            data-tauri-drag-region
            className="fixed inset-x-0 top-0 flex h-10 justify-end border-b bg-background"
          >
            <WindowControls />
          </div>
        )}
        {startupError ? (
          <div className="max-w-lg rounded-md border border-destructive/40 bg-destructive/5 p-5">
            <h1 className="font-semibold text-destructive">notes-rs could not start</h1>
            <p className="mt-2 text-sm break-words text-muted-foreground">{startupError}</p>
            <p className="mt-3 text-xs text-muted-foreground">
              Update the provider configuration, then restart the app.
            </p>
            <Button className="mt-4" onClick={() => setSettingsOpen(true)}>
              <Settings className="size-4" />
              Open settings
            </Button>
          </div>
        ) : (
          <>
            <Loader2 className="size-6 animate-spin text-muted-foreground" />
            <p className="text-sm text-muted-foreground">starting notes-rs…</p>
          </>
        )}
      </div>
    );
  }

  return (
    <>
      <AppLayout
        status={status}
        headerActions={
          <>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Open settings"
              onClick={() => setSettingsOpen(true)}
            >
              <Settings className="size-4" />
            </Button>
            {windowDecorationMode === "borderless" && <WindowControls />}
          </>
        }
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
              key={activeNode.id}
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
                onOpenNode={openSearchResult}
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

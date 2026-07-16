import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Cloud, CloudOff, GitFork, Loader2, Redo2, Search, Settings, Undo2 } from "lucide-react";
import { AppLayout } from "@/app/layout";
import { WindowControls } from "@/app/window-controls";
import { Toaster } from "@/components/ui/sonner";
import { EntitiesCard } from "@/features/entities/entities-card";
import { GraphWorkspace, KnowledgePanel } from "@/features/graph/knowledge-panel";
import { HomeView } from "@/features/home/home-view";
import { SearchCard } from "@/features/search/search-card";
import { PagesList } from "@/features/pages/pages-list";
import { PageView } from "@/features/pages/page-view";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { getSyncStatus, loadSettings, type Node } from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { useAppShortcuts } from "@/app/use-app-shortcuts";
import { useStartupState } from "@/app/use-startup-state";
import { useNotesWorkspace } from "@/features/pages/use-notes-workspace";

const SettingsPage = lazy(() =>
  import("@/features/settings/settings-page").then((module) => ({
    default: module.SettingsPage,
  })),
);

function App() {
  const { ready, startupError } = useStartupState();
  const [status, setStatus] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [windowDecorationMode, setWindowDecorationMode] = useState<"native" | "borderless">(
    "native",
  );
  const [searchOpen, setSearchOpen] = useState(false);
  const [graphOpen, setGraphOpen] = useState(false);
  const workspace = useNotesWorkspace(ready, setStatus);

  const settingsQuery = useQuery({
    queryKey: queryKeys.settings,
    queryFn: loadSettings,
  });
  const syncQuery = useQuery({
    queryKey: queryKeys.syncStatus,
    queryFn: getSyncStatus,
    enabled: ready,
  });
  const createNewNote = useCallback(async () => {
    setGraphOpen(false);
    await workspace.createNewNote();
  }, [workspace]);

  const openNode = useCallback(
    async (node: Node) => {
      setGraphOpen(false);
      await workspace.openNode(node);
    },
    [workspace],
  );

  useEffect(() => {
    if (settingsQuery.data) {
      setWindowDecorationMode(settingsQuery.data.windowDecorationMode);
    }
  }, [settingsQuery.data]);

  useAppShortcuts({
    enabled: ready,
    createNote: () => void createNewNote(),
    openSearch: () => setSearchOpen(true),
    undo: () => void workspace.moveHistory("undo"),
    redo: () => void workspace.moveHistory("redo"),
  });

  if (settingsOpen) {
    return (
      <Suspense
        fallback={
          <div className="app-shell flex h-full items-center justify-center">
            <Loader2 className="size-5 animate-spin text-muted-foreground" />
          </div>
        }
      >
        <SettingsPage
          onBack={() => setSettingsOpen(false)}
          onDecorationModeChanged={setWindowDecorationMode}
          dataAvailable={ready}
          onDataChanged={() => {
            workspace.resetWorkspace();
            setStatus("Imported archive.");
          }}
        />
      </Suspense>
    );
  }

  if (!ready) {
    return (
      <div className="app-shell flex h-full flex-col items-center justify-center gap-3 text-foreground">
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
            {syncQuery.data?.state !== "disabled" && (
              <span
                title={syncQuery.data?.message ?? `Sync ${syncQuery.data?.state}`}
                className={`mr-1 flex h-8 items-center gap-1.5 rounded-xl border px-2.5 text-xs ${
                  syncQuery.data?.state === "online"
                    ? "border-emerald-500/20 bg-emerald-500/8 text-emerald-600"
                    : syncQuery.data?.state === "error"
                      ? "border-destructive/20 bg-destructive/5 text-destructive"
                      : "border-border/60 bg-card/55 text-muted-foreground"
                }`}
              >
                {syncQuery.data?.state === "online" ? (
                  <Cloud className="size-3.5" />
                ) : (
                  <CloudOff className="size-3.5" />
                )}
                <span className="hidden lg:inline">{syncQuery.data?.state}</span>
                {!!syncQuery.data?.pendingOperations && (
                  <span className="tabular-nums">{syncQuery.data.pendingOperations}</span>
                )}
              </span>
            )}
            <Button
              variant={graphOpen ? "secondary" : "ghost"}
              size="sm"
              aria-label={graphOpen ? "Close knowledge graph" : "Open knowledge graph"}
              aria-pressed={graphOpen}
              onClick={() => setGraphOpen((open) => !open)}
              className="hidden h-8 gap-1.5 rounded-xl px-2.5 sm:flex"
            >
              <GitFork className="size-3.5" />
              <span className="text-xs">Graph</span>
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setSearchOpen(true)}
              className="mr-2 hidden h-8 rounded-xl border border-border/60 bg-card/55 px-3 text-muted-foreground shadow-sm hover:bg-card sm:flex"
            >
              <Search className="size-3.5" />
              <span className="text-xs">Search</span>
              <kbd className="ml-3 rounded bg-muted px-1.5 py-0.5 text-[9px]">Ctrl K</kbd>
            </Button>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Undo structural change"
              disabled={workspace.history[0] === 0}
              onClick={() => void workspace.moveHistory("undo")}
            >
              <Undo2 className="size-4" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Redo structural change"
              disabled={workspace.history[1] === 0}
              onClick={() => void workspace.moveHistory("redo")}
            >
              <Redo2 className="size-4" />
            </Button>
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
              selectedUuid={workspace.activePageUuid}
              onCreate={createNewNote}
              onSelect={workspace.selectPage}
              onStatus={setStatus}
            />
            <EntitiesCard variant="compact" />
          </div>
        }
        center={
          workspace.activeNode ? (
            <PageView
              key={workspace.activeNode.uuid}
              node={workspace.activeNode}
              initialBlockId={
                workspace.newNote?.pageUuid === workspace.activeNode.uuid
                  ? workspace.newNote.blockId
                  : null
              }
              autoFocusTitle={workspace.newNote?.pageUuid === workspace.activeNode.uuid}
              onSaved={workspace.applyUpdated}
              onStatus={setStatus}
              onClose={workspace.closePage}
              onDelete={workspace.removePage}
            />
          ) : (
            <HomeView
              creating={workspace.creatingNote}
              hits={workspace.hits}
              setHits={workspace.setHits}
              onCreate={createNewNote}
              onOpenNode={openNode}
              onStatus={setStatus}
            />
          )
        }
        right={<KnowledgePanel node={workspace.activeNode} onOpenNode={openNode} />}
        fullWorkspace={
          graphOpen ? (
            <GraphWorkspace
              node={workspace.activeNode}
              onOpenNode={openNode}
              onClose={() => setGraphOpen(false)}
            />
          ) : undefined
        }
      />
      <Dialog open={searchOpen} onOpenChange={setSearchOpen}>
        <DialogContent className="search-dialog top-[18%] max-w-2xl translate-y-0 rounded-2xl border-border/60 bg-background/95 p-3 shadow-2xl backdrop-blur-xl">
          <div className="sr-only">
            <DialogTitle>Search notes</DialogTitle>
            <DialogDescription>Search all notes and blocks.</DialogDescription>
          </div>
          <SearchCard
            variant="dialog"
            hits={workspace.hits}
            setHits={workspace.setHits}
            onOpenNode={async (node) => {
              await openNode(node);
              setSearchOpen(false);
            }}
            onStatus={setStatus}
          />
        </DialogContent>
      </Dialog>
      <Toaster
        position="bottom-right"
        mobileOffset={{
          right: "calc(1rem + var(--safe-area-inset-right))",
          bottom: "calc(1rem + var(--safe-area-inset-bottom))",
          left: "calc(1rem + var(--safe-area-inset-left))",
        }}
      />
    </>
  );
}

export default App;

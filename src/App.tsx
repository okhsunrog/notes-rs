import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { GitFork, Loader2, Redo2, Search, Settings, Undo2 } from "lucide-react";
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
import {
  getContainingPage,
  getHistoryStatus,
  getNodeByUuid,
  getStartupStatus,
  loadSettings,
  createNote,
  deletePage,
  redo,
  undo,
  type Node,
  type SearchHit,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";

const SettingsPage = lazy(() =>
  import("@/features/settings/settings-page").then((module) => ({
    default: module.SettingsPage,
  })),
);

function App() {
  const queryClient = useQueryClient();
  const [ready, setReady] = useState(false);
  const [startupError, setStartupError] = useState("");
  const [status, setStatus] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [activePageUuid, setActivePageUuid] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [windowDecorationMode, setWindowDecorationMode] = useState<"native" | "borderless">(
    "native",
  );
  const [creatingNote, setCreatingNote] = useState(false);
  const creatingNoteRef = useRef(false);
  const [newNote, setNewNote] = useState<{ pageUuid: string; blockId: number | null } | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [graphOpen, setGraphOpen] = useState(false);

  const settingsQuery = useQuery({
    queryKey: queryKeys.settings,
    queryFn: loadSettings,
  });
  const historyQuery = useQuery({
    queryKey: queryKeys.history,
    queryFn: getHistoryStatus,
    enabled: ready,
  });
  const activeNodeQuery = useQuery({
    queryKey: queryKeys.node(activePageUuid ?? "inactive"),
    queryFn: () => getNodeByUuid(activePageUuid as string),
    enabled: activePageUuid !== null,
  });
  const activeNode = activeNodeQuery.data ?? null;
  const history = historyQuery.data ?? ([0, 0] as const);

  const createNewNote = useCallback(async () => {
    if (creatingNoteRef.current) return;
    creatingNoteRef.current = true;
    setCreatingNote(true);
    try {
      const note = await createNote();
      queryClient.setQueryData(queryKeys.node(note.page.uuid), note.page);
      queryClient.setQueryData(queryKeys.children(note.page.uuid), [note.initialBlock]);
      await queryClient.invalidateQueries({ queryKey: queryKeys.pages });
      setNewNote({ pageUuid: note.page.uuid, blockId: note.initialBlock.id });
      setActivePageUuid(note.page.uuid);
      setGraphOpen(false);
      setHits([]);
      setStatus("New note ready — name it, then press Enter to write.");
      window.dispatchEvent(new Event("notes-rs:show-main"));
    } catch (error) {
      setStatus(`create error: ${String(error)}`);
    } finally {
      creatingNoteRef.current = false;
      setCreatingNote(false);
    }
  }, [queryClient]);

  const moveHistory = useCallback(
    async (direction: "undo" | "redo") => {
      try {
        const changed = direction === "undo" ? await undo() : await redo();
        if (changed) {
          setActivePageUuid(null);
          setHits([]);
          setStatus(direction === "undo" ? "Undid structural change." : "Redid structural change.");
          await queryClient.invalidateQueries({ queryKey: queryKeys.root });
        }
      } catch (error) {
        setStatus(`${direction} error: ${String(error)}`);
      }
    },
    [queryClient],
  );

  useEffect(() => {
    if (settingsQuery.data) {
      setWindowDecorationMode(settingsQuery.data.windowDecorationMode);
    }
  }, [settingsQuery.data]);

  useEffect(() => {
    if (!ready) return;
    const keydown = (event: KeyboardEvent) => {
      const target = event.target;
      if (target instanceof Element && target.matches("input, textarea, [contenteditable=true]"))
        return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        void moveHistory(event.shiftKey ? "redo" : "undo");
      }
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [ready, moveHistory]);

  useEffect(() => {
    if (!ready) return;
    const keydown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setSearchOpen(true);
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        void createNewNote();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [ready, createNewNote]);

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
    queryClient.setQueryData(queryKeys.node(updated.uuid), updated);
    void queryClient.invalidateQueries({ queryKey: queryKeys.pages });
  }

  async function openSearchResult(node: Node) {
    try {
      const page = node.kind === "page" ? node : await getContainingPage(node.id);
      if (!page) {
        setStatus(`No containing page found for #${node.id}`);
        return;
      }
      queryClient.setQueryData(queryKeys.node(page.uuid), page);
      setActivePageUuid(page.uuid);
      setGraphOpen(false);
      window.dispatchEvent(new Event("notes-rs:show-main"));
      if (page.id !== node.id) {
        setStatus(`Opened ${page.title ?? "page"} containing #${node.id}`);
      }
    } catch (error) {
      setStatus(`open error: ${String(error)}`);
    }
  }

  async function removePage(node: Node) {
    if (
      !window.confirm(
        `Delete “${node.title ?? "untitled"}” and all of its blocks? A backup will be created first.`,
      )
    )
      return;
    try {
      if (await deletePage(node.id)) {
        setActivePageUuid(null);
        queryClient.removeQueries({ queryKey: queryKeys.node(node.uuid), exact: true });
        setHits((current) => current.filter((hit) => hit.node.id !== node.id));
        setStatus("Page deleted; a recovery backup was created.");
      }
    } catch (error) {
      setStatus(`delete error: ${String(error)}`);
    }
  }

  if (settingsOpen) {
    return (
      <Suspense
        fallback={
          <div className="flex h-screen items-center justify-center bg-background">
            <Loader2 className="size-5 animate-spin text-muted-foreground" />
          </div>
        }
      >
        <SettingsPage
          onBack={() => setSettingsOpen(false)}
          onDecorationModeChanged={setWindowDecorationMode}
          dataAvailable={ready}
          onDataChanged={() => {
            setActivePageUuid(null);
            setHits([]);
            setStatus("Imported archive.");
            void queryClient.invalidateQueries({ queryKey: queryKeys.root });
          }}
        />
      </Suspense>
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
              disabled={history[0] === 0}
              onClick={() => void moveHistory("undo")}
            >
              <Undo2 className="size-4" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Redo structural change"
              disabled={history[1] === 0}
              onClick={() => void moveHistory("redo")}
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
              selectedUuid={activePageUuid}
              onCreate={createNewNote}
              onSelect={(page) => {
                setNewNote(null);
                queryClient.setQueryData(queryKeys.node(page.uuid), page);
                setActivePageUuid(page.uuid);
                window.dispatchEvent(new Event("notes-rs:show-main"));
              }}
              onStatus={setStatus}
            />
            <EntitiesCard variant="compact" />
          </div>
        }
        center={
          activeNode ? (
            <PageView
              key={activeNode.uuid}
              node={activeNode}
              initialBlockId={newNote?.pageUuid === activeNode.uuid ? newNote.blockId : null}
              autoFocusTitle={newNote?.pageUuid === activeNode.uuid}
              onSaved={applyUpdated}
              onStatus={setStatus}
              onClose={() => setActivePageUuid(null)}
              onDelete={removePage}
            />
          ) : (
            <HomeView
              creating={creatingNote}
              hits={hits}
              setHits={setHits}
              onCreate={createNewNote}
              onOpenNode={openSearchResult}
              onStatus={setStatus}
            />
          )
        }
        right={<KnowledgePanel node={activeNode} onOpenNode={openSearchResult} />}
        fullWorkspace={
          graphOpen ? (
            <GraphWorkspace
              node={activeNode}
              onOpenNode={openSearchResult}
              onClose={() => setGraphOpen(false)}
            />
          ) : undefined
        }
      />
      <Dialog open={searchOpen} onOpenChange={setSearchOpen}>
        <DialogContent className="top-[18%] max-h-[70vh] max-w-2xl translate-y-0 overflow-y-auto rounded-2xl border-border/60 bg-background/95 p-3 shadow-2xl backdrop-blur-xl">
          <div className="sr-only">
            <DialogTitle>Search notes</DialogTitle>
            <DialogDescription>Search all notes and blocks.</DialogDescription>
          </div>
          <SearchCard
            variant="dialog"
            hits={hits}
            setHits={setHits}
            onOpenNode={async (node) => {
              await openSearchResult(node);
              setSearchOpen(false);
            }}
            onStatus={setStatus}
          />
        </DialogContent>
      </Dialog>
      <Toaster position="bottom-right" />
    </>
  );
}

export default App;

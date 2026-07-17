import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Bot,
  Cloud,
  CloudOff,
  GitFork,
  Loader2,
  Redo2,
  Search,
  Settings,
  Undo2,
} from "lucide-react";
import { AppLayout } from "@/app/layout";
import { WindowControls } from "@/app/window-controls";
import { Toaster } from "@/components/ui/sonner";
import { KnowledgePanel } from "@/features/graph/knowledge-panel";
import { SearchCard } from "@/features/search/search-card";
import { PagesList } from "@/features/pages/pages-list";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import {
  getPage,
  getSyncStatus,
  loadSettings,
  type Content,
  type WindowDecorationMode,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { useAppShortcuts } from "@/app/use-app-shortcuts";
import { useStartupState } from "@/app/use-startup-state";
import { useNotesWorkspace } from "@/features/pages/use-notes-workspace";
import type { JournalDate } from "@/lib/api";
import { useAssistantController } from "@/features/chat/use-assistant-controller";
import { Workbench } from "@/features/workspace/workbench";
import {
  DockVisibility,
  PaneContentKind,
  currentDisposition,
  graphTarget,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";

const SettingsPage = lazy(() =>
  import("@/features/settings/settings-page").then((module) => ({
    default: module.SettingsPage,
  })),
);

function App() {
  const { ready, startupError } = useStartupState();
  const [status, setStatus] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [windowDecorationMode, setWindowDecorationMode] = useState<WindowDecorationMode>("native");
  const [searchOpen, setSearchOpen] = useState(false);
  const [editorRequest, setEditorRequest] = useState(0);
  const showEditor = useCallback(() => setEditorRequest((request) => request + 1), []);
  const workspace = useNotesWorkspace(ready, setStatus, showEditor);
  const assistant = useAssistantController();
  const graphOpen = workspace.activePane.content.kind === PaneContentKind.Graph;

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
    await workspace.createNewNote();
  }, [workspace]);

  const openContent = useCallback(
    async (content: Content, disposition?: OpenDisposition) => {
      await workspace.openContent(content, disposition);
    },
    [workspace],
  );
  const openJournal = useCallback(
    async (date: JournalDate, disposition?: OpenDisposition) => {
      await workspace.openJournal(date, disposition);
    },
    [workspace],
  );

  useEffect(() => {
    if (settingsQuery.data) {
      setWindowDecorationMode(settingsQuery.data.windowDecorationMode);
    }
  }, [settingsQuery.data]);

  useEffect(() => {
    if (!status) return;
    const timeout = window.setTimeout(
      () => setStatus(""),
      status.toLowerCase().includes("error") ? 8_000 : 4_000,
    );
    return () => window.clearTimeout(timeout);
  }, [status]);

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
          onDataChanged={(openPageUuid) => {
            workspace.resetWorkspace();
            if (!openPageUuid) return;
            void getPage(openPageUuid)
              .then((page) => {
                if (!page) {
                  setStatus("import error: imported page was not found");
                  return;
                }
                workspace.selectPage(page);
                setSettingsOpen(false);
                setStatus("Opened the imported Logseq workspace.");
              })
              .catch((error: unknown) => setStatus(`import error: ${String(error)}`));
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
              Open Settings to reset invalid device configuration, then restart the app.
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
        editorRequest={editorRequest}
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
              onClick={() => {
                if (graphOpen) {
                  workspace.goPaneBack(workspace.activePane.id);
                } else {
                  workspace.openTarget(graphTarget(workspace.activePageUuid), currentDisposition);
                }
              }}
              className="hidden h-8 gap-1.5 rounded-xl px-2.5 sm:flex"
            >
              <GitFork className="size-3.5" />
              <span className="text-xs">Graph</span>
            </Button>
            <Button
              data-assistant-toggle
              variant="ghost"
              size="sm"
              aria-label={
                workspace.windowWorkspace.assistantDock.visibility === DockVisibility.Hidden
                  ? "Show Assistant"
                  : "Hide Assistant"
              }
              aria-expanded={
                workspace.windowWorkspace.assistantDock.visibility !== DockVisibility.Hidden
              }
              aria-controls="assistant-dock-content"
              onClick={() =>
                workspace.setDockVisibility(
                  workspace.windowWorkspace.assistantDock.visibility === DockVisibility.Hidden
                    ? DockVisibility.Open
                    : DockVisibility.Hidden,
                )
              }
            >
              <Bot className="size-4" />
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
              disabled={workspace.history.undoCount === 0}
              onClick={() => void workspace.moveHistory("undo")}
            >
              <Undo2 className="size-4" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Redo structural change"
              disabled={workspace.history.redoCount === 0}
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
          <PagesList
            selectedUuid={workspace.activePageUuid}
            activeJournalDate={
              workspace.pendingJournalDate ??
              (workspace.activePage?.kind.kind === "journal"
                ? workspace.activePage.kind.date
                : null)
            }
            onCreate={createNewNote}
            onOpenJournal={openJournal}
            onQuickCapture={workspace.quickCapture}
            journalBusy={workspace.journalBusy}
            onSelect={workspace.selectPage}
            onStatus={setStatus}
          />
        }
        workbench={
          <Workbench
            state={workspace.windowWorkspace}
            creatingNote={workspace.creatingNote}
            journalBusy={workspace.journalBusy}
            newNote={workspace.newNote}
            hits={workspace.hits}
            setHits={workspace.setHits}
            onStatus={setStatus}
            onCreate={createNewNote}
            onOpenContent={openContent}
            onOpenJournal={openJournal}
            onCaptureJournal={workspace.captureJournal}
            onSaved={workspace.applyUpdated}
            onDelete={workspace.removePage}
            onOpenMarkdownLink={workspace.openMarkdownLink}
            onFocusPane={workspace.focusPane}
            onShowCompactPane={workspace.showCompactPane}
            onClosePane={workspace.closePane}
            onPaneBack={workspace.goPaneBack}
            onPaneForward={workspace.goPaneForward}
            onResizeSplit={(splitId, ratio) => workspace.resizeSplit(splitId, ratio)}
            onPresentationChange={workspace.setPagePresentation}
          />
        }
        assistant={
          <KnowledgePanel
            page={workspace.activePage}
            onOpenMarkdownLink={workspace.openMarkdownLink}
            controller={assistant}
          />
        }
        assistantVisibility={workspace.windowWorkspace.assistantDock.visibility}
        assistantWidth={workspace.windowWorkspace.assistantDock.width}
        assistantBusy={assistant.state.busy}
        onAssistantVisibilityChange={workspace.setDockVisibility}
        onAssistantWidthChange={workspace.setDockWidth}
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
            onOpenContent={async (content, disposition) => {
              await openContent(content, disposition);
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

import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Bot, GitFork, Loader2, Redo2, Search, Settings, Undo2 } from "lucide-react";
import { AppLayout } from "@/app/layout";
import { useCompactLayout } from "@/app/use-compact-layout";
import { WindowControls } from "@/app/window-controls";
import { useWindowCorners } from "@/app/use-window-corners";
import { KnowledgePanel } from "@/features/graph/knowledge-panel";
import { SyncStatusIndicator } from "@/features/sync/sync-status-indicator";
import { SearchCard } from "@/features/search/search-card";
import { PagesList } from "@/features/pages/pages-list";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { getPage, getSyncStatus, loadSettings, type WindowDecorationMode } from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { useAppShortcuts } from "@/app/use-app-shortcuts";
import { useStartupState } from "@/app/use-startup-state";
import { useNotesWorkspace } from "@/features/pages/use-notes-workspace";
import { useAssistantController } from "@/features/chat/use-assistant-controller";
import { Workbench } from "@/features/workspace/workbench";
import { WorkspaceControllerProvider } from "@/features/workspace/workspace-controller";
import { useWorkspaceStore } from "@/features/workspace/workspace-store";
import { notifyError, notifySuccess } from "@/lib/notify";
import {
  DockVisibility,
  PaneContentKind,
  currentDisposition,
  graphTarget,
} from "@/features/workspace/workspace-model";

const SettingsPage = lazy(() =>
  import("@/features/settings/settings-page").then((module) => ({
    default: module.SettingsPage,
  })),
);

function App() {
  const compact = useCompactLayout();
  const { ready, startupError } = useStartupState();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const closeSettings = useCallback(() => setSettingsOpen(false), []);
  const [windowDecorationMode, setWindowDecorationMode] = useState<WindowDecorationMode>("native");
  const [searchOpen, setSearchOpen] = useState(false);
  const [editorRequest, setEditorRequest] = useState(0);
  const [assistantRequest, setAssistantRequest] = useState(0);
  const showEditor = useCallback(() => setEditorRequest((request) => request + 1), []);
  const workspace = useNotesWorkspace(ready, showEditor);
  const assistant = useAssistantController();
  const activePane = workspace.activePane;
  const assistantDock = useWorkspaceStore((state) => state.assistantDock);
  const dispatchWorkspace = useWorkspaceStore((state) => state.dispatch);
  const graphOpen = activePane.content.kind === PaneContentKind.Graph;

  const settingsQuery = useQuery({
    queryKey: queryKeys.settings,
    queryFn: loadSettings,
  });
  const syncQuery = useQuery({
    queryKey: queryKeys.syncStatus,
    queryFn: getSyncStatus,
    enabled: ready,
  });
  useWindowCorners(
    windowDecorationMode === "borderless" &&
      !!settingsQuery.data?.capabilities.windowCornerRounding,
    settingsQuery.data?.windowCornerRadius ?? 10,
  );
  useEffect(() => {
    if (settingsQuery.data) {
      setWindowDecorationMode(settingsQuery.data.activeWindowDecorationMode);
    }
  }, [settingsQuery.data]);

  useAppShortcuts({
    enabled: ready,
    createNote: () => void workspace.createNewNote(),
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
          onBack={closeSettings}
          onDecorationModeChanged={setWindowDecorationMode}
          dataAvailable={ready}
          onDataChanged={(openPageUuid) => {
            workspace.resetWorkspace();
            if (!openPageUuid) return;
            void getPage(openPageUuid)
              .then((page) => {
                if (!page) {
                  notifyError("import", "imported page was not found");
                  return;
                }
                workspace.selectPage(page);
                setSettingsOpen(false);
                notifySuccess("Opened the imported Logseq workspace.");
              })
              .catch((error: unknown) => notifyError("import", error));
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
            <h1 className="font-semibold text-destructive">Tangleaf could not start</h1>
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
            <p className="text-sm text-muted-foreground">Starting Tangleaf…</p>
          </>
        )}
      </div>
    );
  }

  return (
    <WorkspaceControllerProvider controller={workspace.controller}>
      <AppLayout
        editorRequest={editorRequest}
        assistantRequest={assistantRequest}
        searchOpen={searchOpen}
        onCloseSearch={() => setSearchOpen(false)}
        workbenchIsHome={activePane.content.kind === PaneContentKind.Home}
        onNavigateBack={() => {
          if (activePane.back.length > 0) {
            dispatchWorkspace({ type: "go_back", paneId: activePane.id });
          } else {
            workspace.closePage();
          }
        }}
        onOpenHome={workspace.closePage}
        headerActions={
          <>
            {syncQuery.data && syncQuery.data.state !== "disabled" && (
              <SyncStatusIndicator
                status={syncQuery.data}
                onOpenSettings={() => setSettingsOpen(true)}
              />
            )}
            {compact ? (
              <>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Search notes"
                  onClick={() => setSearchOpen(true)}
                >
                  <Search className="size-4" />
                </Button>
                <Button
                  data-assistant-toggle
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Open Assistant"
                  onClick={() => {
                    dispatchWorkspace({
                      type: "set_dock_visibility",
                      visibility: DockVisibility.Open,
                    });
                    setAssistantRequest((request) => request + 1);
                  }}
                >
                  <Bot className="size-4" />
                </Button>
              </>
            ) : (
              <>
                <Button
                  variant={graphOpen ? "secondary" : "ghost"}
                  size="sm"
                  aria-label={graphOpen ? "Close knowledge graph" : "Open knowledge graph"}
                  aria-pressed={graphOpen}
                  onClick={() => {
                    if (graphOpen) {
                      dispatchWorkspace({ type: "go_back", paneId: activePane.id });
                    } else {
                      dispatchWorkspace({
                        type: "open_target",
                        target: graphTarget(workspace.activePageUuid),
                        disposition: currentDisposition,
                      });
                    }
                  }}
                  className="h-8 gap-1.5 rounded-xl px-2.5"
                >
                  <GitFork className="size-3.5" />
                  <span className="text-xs">Graph</span>
                </Button>
                <Button
                  data-assistant-toggle
                  variant="ghost"
                  size="sm"
                  aria-label={
                    assistantDock.visibility === DockVisibility.Hidden
                      ? "Show Assistant"
                      : "Hide Assistant"
                  }
                  aria-expanded={assistantDock.visibility !== DockVisibility.Hidden}
                  aria-controls="assistant-dock-content"
                  onClick={() =>
                    dispatchWorkspace({
                      type: "set_dock_visibility",
                      visibility:
                        assistantDock.visibility === DockVisibility.Hidden
                          ? DockVisibility.Open
                          : DockVisibility.Hidden,
                    })
                  }
                >
                  <Bot className="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSearchOpen(true)}
                  className="mr-2 h-8 rounded-xl border border-border/60 bg-card/55 px-3 text-muted-foreground shadow-sm hover:bg-card"
                >
                  <Search className="size-3.5" />
                  <span className="text-xs">Search</span>
                  <kbd className="ml-3 rounded bg-muted px-1.5 py-0.5 text-[9px]">Ctrl K</kbd>
                </Button>
              </>
            )}
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
            onCreate={workspace.createNewNote}
            onOpenJournal={workspace.openJournal}
            onQuickCapture={workspace.quickCapture}
            allNotesActive={activePane.content.kind === PaneContentKind.AllNotes}
            onOpenAllNotes={workspace.openAllNotes}
            journalBusy={workspace.journalBusy}
            onSelect={workspace.selectPage}
          />
        }
        workbench={
          <Workbench
            creatingNote={workspace.creatingNote}
            journalBusy={workspace.journalBusy}
            newNote={workspace.newNote}
          />
        }
        assistant={
          <KnowledgePanel
            page={workspace.activePage}
            onOpenMarkdownLink={workspace.openMarkdownLink}
            controller={assistant}
          />
        }
        assistantVisibility={assistantDock.visibility}
        assistantWidth={assistantDock.width}
        assistantBusy={assistant.state.busy}
        onAssistantVisibilityChange={(visibility) =>
          dispatchWorkspace({ type: "set_dock_visibility", visibility })
        }
        onAssistantWidthChange={(width) => dispatchWorkspace({ type: "set_dock_width", width })}
      />
      <Dialog open={searchOpen} onOpenChange={setSearchOpen}>
        <DialogContent
          showCloseButton={false}
          className={
            compact
              ? "search-dialog search-dialog-compact top-[var(--safe-area-inset-top)] left-0 h-[calc(100dvh-var(--safe-area-inset-top))] max-w-none translate-x-0 translate-y-0 gap-0 overflow-hidden rounded-none border-0 bg-background/95 p-0 shadow-2xl backdrop-blur-xl sm:max-w-none"
              : "search-dialog top-[18%] max-w-2xl translate-y-0 gap-0 overflow-hidden rounded-2xl border-border/60 bg-background/95 p-0 shadow-2xl backdrop-blur-xl"
          }
        >
          <div className="sr-only">
            <DialogTitle>Search notes</DialogTitle>
            <DialogDescription>Search all notes and blocks.</DialogDescription>
          </div>
          <SearchCard
            variant="dialog"
            onOpenContent={workspace.openContent}
            onDismiss={() => setSearchOpen(false)}
          />
        </DialogContent>
      </Dialog>
    </WorkspaceControllerProvider>
  );
}

export default App;

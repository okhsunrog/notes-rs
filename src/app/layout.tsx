import { useCallback, useEffect, useReducer, type ReactNode } from "react";
import { onBackButtonPress } from "@tauri-apps/api/app";
import { ArrowLeft, Bot, ChevronLeft, Sparkles, X } from "lucide-react";
import { Group, Panel, Separator, usePanelRef } from "react-resizable-panels";
import { Button } from "@/components/ui/button";
import { DockVisibility } from "@/features/workspace/workspace-model";
import { cn } from "@/lib/utils";
import {
  CompactRegion,
  CompactBackToHomeContext,
  compactNavigationReducer,
  initialCompactNavigation,
  workbenchIsVisible,
} from "./compact-navigation";
import { useCompactLayout } from "./use-compact-layout";

type Props = {
  headerActions?: ReactNode;
  sidebar: ReactNode;
  workbench: ReactNode;
  assistant: ReactNode;
  assistantVisibility: DockVisibility;
  assistantWidth: number;
  assistantBusy: boolean;
  onAssistantVisibilityChange: (visibility: DockVisibility) => void;
  onAssistantWidthChange: (width: number) => void;
  workbenchIsHome: boolean;
  editorRequest: number;
  assistantRequest: number;
  searchOpen: boolean;
  onCloseSearch: () => void;
  onNavigateBack: () => void;
  onOpenHome: () => void;
};

export function AppLayout({
  headerActions,
  sidebar,
  workbench,
  assistant,
  assistantVisibility,
  assistantWidth,
  assistantBusy,
  onAssistantVisibilityChange,
  onAssistantWidthChange,
  workbenchIsHome,
  editorRequest,
  assistantRequest,
  searchOpen,
  onCloseSearch,
  onNavigateBack,
  onOpenHome,
}: Props) {
  const compact = useCompactLayout();
  const [compactNavigation, dispatchCompactNavigation] = useReducer(
    compactNavigationReducer,
    initialCompactNavigation,
  );
  const compactRegion = compactNavigation.region;
  const sidebarRef = usePanelRef();
  const workbenchRef = usePanelRef();
  const assistantRef = usePanelRef();
  const assistantVisible = assistantVisibility !== DockVisibility.Hidden;

  useEffect(() => {
    if (editorRequest > 0) dispatchCompactNavigation({ type: "open_editor" });
  }, [editorRequest]);
  useEffect(() => {
    if (assistantRequest > 0) dispatchCompactNavigation({ type: "open_assistant" });
  }, [assistantRequest]);
  useEffect(() => {
    if (workbenchIsHome && compactRegion === CompactRegion.Editor) {
      dispatchCompactNavigation({ type: "open_home" });
    }
  }, [compactRegion, workbenchIsHome]);
  useEffect(() => {
    if (!assistantVisible && compactRegion === CompactRegion.Assistant) {
      dispatchCompactNavigation({ type: "close_assistant" });
    }
  }, [assistantVisible, compactRegion]);

  const openHome = useCallback(() => dispatchCompactNavigation({ type: "open_home" }), []);
  const returnToDashboard = useCallback(() => {
    onOpenHome();
    openHome();
  }, [onOpenHome, openHome]);
  const navigateCompactBack = useCallback(() => {
    if (searchOpen) {
      onCloseSearch();
    } else if (compactRegion === CompactRegion.Assistant) {
      dispatchCompactNavigation({ type: "close_assistant" });
    } else if (compactRegion === CompactRegion.Editor) {
      onNavigateBack();
    }
  }, [compactRegion, onCloseSearch, onNavigateBack, searchOpen]);

  useEffect(() => {
    if (
      !compact ||
      (compactRegion === CompactRegion.Home && !searchOpen) ||
      !navigator.userAgent.toLocaleLowerCase().includes("android")
    ) {
      return;
    }
    let disposed = false;
    let unlisten: (() => Promise<void>) | undefined;
    void onBackButtonPress(() => navigateCompactBack())
      .then((listener) => {
        if (disposed) void listener.unregister();
        else unlisten = () => listener.unregister();
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      void unlisten?.();
    };
  }, [compact, compactRegion, navigateCompactBack, searchOpen]);

  useEffect(() => {
    const notes = sidebarRef.current;
    const center = workbenchRef.current;
    const dock = assistantRef.current;
    if (!notes || !center || !dock) return;

    if (compact) {
      if (compactRegion === CompactRegion.Assistant) {
        dock.expand();
        dock.resize("100%");
        notes.collapse();
        center.collapse();
      } else {
        center.expand();
        center.resize("100%");
        notes.collapse();
        dock.collapse();
      }
      return;
    }

    notes.expand();
    center.expand();
    notes.resize("21%");
    if (assistantVisible) {
      dock.expand();
      dock.resize(assistantVisibility === DockVisibility.Rail ? "48px" : `${assistantWidth}px`);
    } else {
      dock.collapse();
    }
  }, [
    assistantRef,
    assistantVisibility,
    assistantVisible,
    assistantWidth,
    compact,
    compactRegion,
    sidebarRef,
    workbenchRef,
  ]);

  useEffect(() => {
    if (assistantVisible) return;
    let focusFrame = 0;
    const layoutFrame = requestAnimationFrame(() => {
      focusFrame = requestAnimationFrame(() => {
        document.querySelector<HTMLButtonElement>("[data-assistant-toggle]")?.focus();
      });
    });
    return () => {
      cancelAnimationFrame(layoutFrame);
      cancelAnimationFrame(focusFrame);
    };
  }, [assistantVisible]);

  const hideAssistantAndRestoreFocus = () => {
    onAssistantVisibilityChange(DockVisibility.Hidden);
  };

  return (
    <CompactBackToHomeContext.Provider
      value={compact && compactRegion === CompactRegion.Editor ? returnToDashboard : null}
    >
      <div className="app-shell flex h-full flex-col overflow-hidden text-foreground">
        <header
          data-tauri-drag-region
          className={cn(
            "relative z-20 flex h-14 shrink-0 items-center justify-between px-3",
            compact && "h-12",
          )}
        >
          <button
            type="button"
            aria-label="Open dashboard"
            className="group -ml-1 flex items-center gap-2.5 rounded-xl px-2 py-1 text-left outline-none transition-colors hover:bg-accent/60 focus-visible:ring-[3px] focus-visible:ring-ring/50"
            onClick={() => {
              returnToDashboard();
            }}
          >
            <div className="brand-mark flex size-8 items-center justify-center rounded-xl text-primary-foreground shadow-sm">
              <Sparkles className="size-4" />
            </div>
            <div className="leading-none">
              <h1 className="text-[15px] font-semibold tracking-[-0.02em]">notes-rs</h1>
              <p
                className={cn(
                  "mt-1 text-[10px] font-medium tracking-wide text-muted-foreground",
                  compact && "hidden",
                )}
              >
                connected thinking
              </p>
            </div>
          </button>
          <div className="flex items-center gap-1">{headerActions}</div>
        </header>

        <div
          className={cn(
            "workspace-frame relative mx-2 mb-2 min-h-0 flex-1 overflow-hidden rounded-2xl border shadow-xl shadow-black/5",
            compact && "mx-0 mb-0 rounded-none border-x-0 border-b-0 shadow-none",
          )}
        >
          <Group orientation="horizontal" className="h-full min-h-0" disabled={compact}>
            <Panel
              id="navigation-sidebar"
              panelRef={sidebarRef}
              defaultSize="21%"
              minSize={compact ? 0 : "16%"}
              maxSize={compact ? "100%" : "30%"}
              collapsible
              collapsedSize={0}
            >
              <aside
                aria-hidden={compact ? true : undefined}
                inert={compact ? true : undefined}
                className="sidebar-surface h-full overflow-y-auto p-3"
              >
                {sidebar}
              </aside>
            </Panel>

            <Separator
              className={cn(
                "group relative w-px bg-border/70 transition hover:bg-primary/40 after:absolute after:inset-y-0 after:-left-1 after:w-2",
                compact && "hidden",
              )}
            />

            <Panel
              id="workbench"
              panelRef={workbenchRef}
              defaultSize="56%"
              minSize={compact ? 0 : "38%"}
              collapsible
              collapsedSize={0}
            >
              <main
                aria-hidden={compact && !workbenchIsVisible(compactRegion) ? true : undefined}
                inert={compact && !workbenchIsVisible(compactRegion) ? true : undefined}
                className="canvas-surface h-full overflow-hidden"
              >
                {workbench}
              </main>
            </Panel>

            <Separator
              className={cn(
                "group relative w-px bg-border/70 transition hover:bg-primary/40 after:absolute after:inset-y-0 after:-left-1 after:w-2",
                (!assistantVisible || compact || assistantVisibility === DockVisibility.Rail) &&
                  "hidden",
              )}
            />
            <Panel
              id="assistant-dock"
              panelRef={assistantRef}
              defaultSize={
                assistantVisibility === DockVisibility.Rail ? "48px" : `${assistantWidth}px`
              }
              minSize={
                compact || !assistantVisible
                  ? 0
                  : assistantVisibility === DockVisibility.Rail
                    ? 48
                    : 320
              }
              maxSize={
                compact
                  ? "100%"
                  : !assistantVisible
                    ? 0
                    : assistantVisibility === DockVisibility.Rail
                      ? 48
                      : 480
              }
              groupResizeBehavior="preserve-pixel-size"
              disabled={!compact && assistantVisibility !== DockVisibility.Open}
              collapsible
              collapsedSize={0}
              onResize={(size, _id, previous) => {
                if (
                  previous &&
                  !compact &&
                  assistantVisibility === DockVisibility.Open &&
                  size.inPixels >= 320
                ) {
                  onAssistantWidthChange(size.inPixels);
                }
              }}
            >
              {(assistantVisible || compact) &&
                (compact || assistantVisibility === DockVisibility.Open ? (
                  <section
                    id="assistant-dock-content"
                    aria-hidden={
                      compact && compactRegion !== CompactRegion.Assistant ? true : undefined
                    }
                    inert={compact && compactRegion !== CompactRegion.Assistant ? true : undefined}
                    className={cn(
                      "inspector-surface flex h-full min-h-0 flex-col overflow-hidden p-3",
                      compact && "pb-[calc(0.75rem+var(--safe-area-inset-bottom))]",
                    )}
                  >
                    <div
                      className={cn("mb-1 flex h-7 justify-end gap-1", compact && "justify-start")}
                    >
                      {compact && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="xs"
                          aria-label="Back from Assistant"
                          onClick={() => dispatchCompactNavigation({ type: "close_assistant" })}
                          className="gap-1 rounded-lg px-1.5"
                        >
                          <ArrowLeft className="size-3.5" />
                          Back
                        </Button>
                      )}
                      {!compact && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          aria-label="Collapse Assistant to rail"
                          onClick={() => onAssistantVisibilityChange(DockVisibility.Rail)}
                        >
                          <ChevronLeft className="size-3.5" />
                        </Button>
                      )}
                      {!compact && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          aria-label="Hide Assistant"
                          onClick={hideAssistantAndRestoreFocus}
                        >
                          <X className="size-3.5" />
                        </Button>
                      )}
                    </div>
                    <div className="min-h-0 flex-1">{assistant}</div>
                  </section>
                ) : (
                  <AssistantRail
                    busy={assistantBusy}
                    onOpen={() => onAssistantVisibilityChange(DockVisibility.Open)}
                    onHide={hideAssistantAndRestoreFocus}
                  />
                ))}
            </Panel>
          </Group>
        </div>
      </div>
    </CompactBackToHomeContext.Provider>
  );
}

function AssistantRail({
  busy,
  onOpen,
  onHide,
}: {
  busy: boolean;
  onOpen: () => void;
  onHide: () => void;
}) {
  return (
    <section className="inspector-surface flex h-full w-12 flex-col items-center gap-2 py-3">
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        aria-label="Expand Assistant"
        aria-expanded={false}
        aria-controls="assistant-dock-content"
        onClick={onOpen}
        className="relative rounded-xl text-primary"
      >
        <Bot className="size-4" />
        {busy && (
          <span className="absolute top-1 right-1 size-2 animate-pulse rounded-full bg-primary" />
        )}
      </Button>
      <span className="[writing-mode:vertical-rl] text-[9px] font-semibold tracking-widest text-muted-foreground uppercase">
        Assistant
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        aria-label="Hide Assistant"
        onClick={onHide}
        className="mt-auto"
      >
        <X className="size-3" />
      </Button>
    </section>
  );
}

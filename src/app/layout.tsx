import { useEffect, useState, type ReactNode } from "react";
import { Bot, ChevronLeft, Files, Network, Sparkles, X } from "lucide-react";
import { Group, Panel, Separator, usePanelRef } from "react-resizable-panels";
import { Button } from "@/components/ui/button";
import { DockVisibility } from "@/features/workspace/workspace-model";
import { cn } from "@/lib/utils";
import { useCompactLayout } from "./use-compact-layout";

enum CompactRegion {
  Notes = "notes",
  Workbench = "workbench",
  Assistant = "assistant",
}

type Props = {
  status?: string;
  headerActions?: ReactNode;
  sidebar: ReactNode;
  workbench: ReactNode;
  assistant: ReactNode;
  assistantVisibility: DockVisibility;
  assistantWidth: number;
  assistantBusy: boolean;
  onAssistantVisibilityChange: (visibility: DockVisibility) => void;
  onAssistantWidthChange: (width: number) => void;
  editorRequest: number;
};

export function AppLayout({
  status,
  headerActions,
  sidebar,
  workbench,
  assistant,
  assistantVisibility,
  assistantWidth,
  assistantBusy,
  onAssistantVisibilityChange,
  onAssistantWidthChange,
  editorRequest,
}: Props) {
  const compact = useCompactLayout();
  const [compactRegion, setCompactRegion] = useState(CompactRegion.Workbench);
  const sidebarRef = usePanelRef();
  const workbenchRef = usePanelRef();
  const assistantRef = usePanelRef();
  const assistantVisible = assistantVisibility !== DockVisibility.Hidden;

  useEffect(() => setCompactRegion(CompactRegion.Workbench), [editorRequest]);
  useEffect(() => {
    if (!assistantVisible && compactRegion === CompactRegion.Assistant) {
      setCompactRegion(CompactRegion.Workbench);
    }
  }, [assistantVisible, compactRegion]);

  useEffect(() => {
    const notes = sidebarRef.current;
    const center = workbenchRef.current;
    const dock = assistantRef.current;
    if (!notes || !center || !dock) return;

    if (compact) {
      if (compactRegion === CompactRegion.Notes) {
        notes.expand();
        notes.resize("100%");
        center.collapse();
        dock.collapse();
      } else if (compactRegion === CompactRegion.Assistant && assistantVisible) {
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
    <div className="app-shell flex h-full flex-col overflow-hidden text-foreground">
      <header
        data-tauri-drag-region
        className="relative z-20 flex h-14 shrink-0 items-center justify-between px-3"
      >
        <div data-tauri-drag-region className="flex items-center gap-2.5 pl-1">
          <div className="brand-mark flex size-8 items-center justify-center rounded-xl text-primary-foreground shadow-sm">
            <Sparkles className="size-4" />
          </div>
          <div data-tauri-drag-region className="leading-none">
            <h1 className="text-[15px] font-semibold tracking-[-0.02em]">notes-rs</h1>
            <p className="mt-1 text-[10px] font-medium tracking-wide text-muted-foreground">
              connected thinking
            </p>
          </div>
        </div>
        <div className="pointer-events-none absolute inset-x-1/3 flex justify-center">
          {status && (
            <span className="max-w-sm truncate rounded-full border border-border/60 bg-card/70 px-3 py-1 text-[11px] text-muted-foreground shadow-sm backdrop-blur">
              {status}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1">{headerActions}</div>
      </header>

      <div className="workspace-frame relative mx-2 mb-2 min-h-0 flex-1 overflow-hidden rounded-2xl border shadow-xl shadow-black/5">
        <Group
          orientation="horizontal"
          className={cn(
            "h-full min-h-0",
            compact && "pb-[calc(3.5rem+var(--safe-area-inset-bottom))]",
          )}
        >
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
              aria-hidden={compact && compactRegion !== CompactRegion.Notes ? true : undefined}
              inert={compact && compactRegion !== CompactRegion.Notes ? true : undefined}
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
              aria-hidden={compact && compactRegion !== CompactRegion.Workbench ? true : undefined}
              inert={compact && compactRegion !== CompactRegion.Workbench ? true : undefined}
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
            {assistantVisible &&
              (compact || assistantVisibility === DockVisibility.Open ? (
                <section
                  id="assistant-dock-content"
                  aria-hidden={
                    compact && compactRegion !== CompactRegion.Assistant ? true : undefined
                  }
                  inert={compact && compactRegion !== CompactRegion.Assistant ? true : undefined}
                  className="inspector-surface flex h-full min-h-0 flex-col overflow-hidden p-3"
                >
                  <div className="mb-1 flex h-7 justify-end gap-1">
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
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      aria-label="Hide Assistant"
                      onClick={hideAssistantAndRestoreFocus}
                    >
                      <X className="size-3.5" />
                    </Button>
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

        {compact && (
          <nav
            className={cn(
              "absolute inset-x-0 bottom-0 z-20 grid h-[calc(3.5rem+var(--safe-area-inset-bottom))] border-t bg-card/95 px-3 pb-[var(--safe-area-inset-bottom)] backdrop-blur",
              assistantVisible ? "grid-cols-3" : "grid-cols-2",
            )}
            aria-label="Workspace panels"
          >
            <CompactTab
              active={compactRegion === CompactRegion.Notes}
              icon={<Files className="size-4" />}
              label="Notes"
              onClick={() => setCompactRegion(CompactRegion.Notes)}
            />
            <CompactTab
              active={compactRegion === CompactRegion.Workbench}
              icon={<Network className="size-4" />}
              label="Workspace"
              onClick={() => setCompactRegion(CompactRegion.Workbench)}
            />
            {assistantVisible && (
              <CompactTab
                active={compactRegion === CompactRegion.Assistant}
                icon={<Bot className="size-4" />}
                label="Assistant"
                busy={assistantBusy}
                onClick={() => setCompactRegion(CompactRegion.Assistant)}
              />
            )}
          </nav>
        )}
      </div>
    </div>
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

function CompactTab({
  active,
  icon,
  label,
  busy = false,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  busy?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`relative flex flex-col items-center justify-center gap-0.5 text-[11px] font-medium transition ${active ? "text-primary" : "text-muted-foreground hover:text-foreground"}`}
    >
      {icon}
      {busy && <span className="absolute top-2 right-1/3 size-1.5 rounded-full bg-primary" />}
      {label}
    </button>
  );
}

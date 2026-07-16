import { useEffect, useState, type ReactNode } from "react";
import { Bot, Files, Network, Sparkles } from "lucide-react";
import { Group, Panel, Separator } from "react-resizable-panels";

type Props = {
  status?: string;
  headerActions?: ReactNode;
  sidebar: ReactNode;
  center: ReactNode;
  right: ReactNode;
  fullWorkspace?: ReactNode;
};

export function AppLayout({ status, headerActions, sidebar, center, right, fullWorkspace }: Props) {
  const [compact, setCompact] = useState(() => window.innerWidth < 1000);
  const [compactPanel, setCompactPanel] = useState<"notes" | "editor" | "assistant">("editor");

  useEffect(() => {
    const resize = () => setCompact(window.innerWidth < 1000);
    const showMain = () => setCompactPanel("editor");
    window.addEventListener("resize", resize);
    window.addEventListener("notes-rs:show-main", showMain);
    return () => {
      window.removeEventListener("resize", resize);
      window.removeEventListener("notes-rs:show-main", showMain);
    };
  }, []);

  return (
    <div className="app-shell flex h-screen flex-col overflow-hidden bg-background text-foreground">
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

      {fullWorkspace ? (
        <div className="workspace-frame mx-2 mb-2 min-h-0 flex-1 overflow-hidden rounded-2xl border shadow-xl shadow-black/5">
          {fullWorkspace}
        </div>
      ) : compact ? (
        <div className="workspace-frame mx-2 mb-2 flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl border shadow-xl shadow-black/5">
          <div className="min-h-0 flex-1 overflow-hidden">
            {compactPanel === "notes" && (
              <aside className="sidebar-surface h-full overflow-y-auto p-3">{sidebar}</aside>
            )}
            {compactPanel === "editor" && (
              <main className="canvas-surface h-full overflow-y-auto">{center}</main>
            )}
            {compactPanel === "assistant" && (
              <section className="inspector-surface h-full overflow-hidden p-3">{right}</section>
            )}
          </div>
          <nav
            className="grid h-14 shrink-0 grid-cols-3 border-t bg-card/90 px-3 backdrop-blur"
            aria-label="Workspace panels"
          >
            <CompactTab
              active={compactPanel === "notes"}
              icon={<Files className="size-4" />}
              label="Notes"
              onClick={() => setCompactPanel("notes")}
            />
            <CompactTab
              active={compactPanel === "editor"}
              icon={<Network className="size-4" />}
              label="Editor"
              onClick={() => setCompactPanel("editor")}
            />
            <CompactTab
              active={compactPanel === "assistant"}
              icon={<Bot className="size-4" />}
              label="Assistant"
              onClick={() => setCompactPanel("assistant")}
            />
          </nav>
        </div>
      ) : (
        <div className="workspace-frame mx-2 mb-2 min-h-0 flex-1 overflow-hidden rounded-2xl border shadow-xl shadow-black/5">
          <Group orientation="horizontal" className="h-full min-h-0">
            <Panel defaultSize="21%" minSize="16%" maxSize="30%">
              <aside className="sidebar-surface h-full overflow-y-auto p-3">{sidebar}</aside>
            </Panel>

            <Separator className="group relative w-px bg-border/70 transition hover:bg-primary/40 after:absolute after:inset-y-0 after:-left-1 after:w-2" />

            <Panel defaultSize="56%" minSize="38%">
              <main className="canvas-surface h-full overflow-y-auto">{center}</main>
            </Panel>

            <Separator className="group relative w-px bg-border/70 transition hover:bg-primary/40 after:absolute after:inset-y-0 after:-left-1 after:w-2" />

            <Panel defaultSize="23%" minSize="18%" maxSize="38%" collapsible>
              <section className="inspector-surface h-full overflow-hidden p-3">{right}</section>
            </Panel>
          </Group>
        </div>
      )}
    </div>
  );
}

function CompactTab({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex flex-col items-center justify-center gap-0.5 text-[11px] font-medium transition ${active ? "text-primary" : "text-muted-foreground hover:text-foreground"}`}
    >
      {icon}
      {label}
    </button>
  );
}

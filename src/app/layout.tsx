import { useEffect, useState, type ReactNode } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";

type Props = {
  status?: string;
  headerActions?: ReactNode;
  sidebar: ReactNode;
  center: ReactNode;
  right: ReactNode;
};

export function AppLayout({ status, headerActions, sidebar, center, right }: Props) {
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
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="flex h-12 shrink-0 items-center justify-between border-b px-4"
      >
        <h1 className="text-lg font-semibold tracking-tight">notes-rs</h1>
        <div className="flex items-center gap-3">
          {status && <span className="text-xs text-muted-foreground">{status}</span>}
          {headerActions}
        </div>
      </header>

      {compact ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <nav
            className="grid shrink-0 grid-cols-3 border-b bg-muted/30 p-1"
            aria-label="Workspace panels"
          >
            {(["notes", "editor", "assistant"] as const).map((panel) => (
              <button
                key={panel}
                type="button"
                onClick={() => setCompactPanel(panel)}
                className={`rounded px-3 py-1.5 text-xs capitalize ${compactPanel === panel ? "bg-background font-medium shadow-sm" : "text-muted-foreground"}`}
              >
                {panel}
              </button>
            ))}
          </nav>
          <div className="min-h-0 flex-1 overflow-hidden">
            {compactPanel === "notes" && (
              <aside className="h-full overflow-y-auto p-3">{sidebar}</aside>
            )}
            {compactPanel === "editor" && (
              <main className="h-full overflow-y-auto p-4">{center}</main>
            )}
            {compactPanel === "assistant" && (
              <section className="h-full overflow-hidden p-3">{right}</section>
            )}
          </div>
        </div>
      ) : (
        <Group orientation="horizontal" className="flex-1">
          <Panel defaultSize="20%" minSize="12%" maxSize="35%">
            <aside className="h-full overflow-y-auto border-r p-3">{sidebar}</aside>
          </Panel>

          <Separator className="w-px bg-border transition hover:bg-accent" />

          <Panel defaultSize="55%" minSize="30%">
            <main className="h-full overflow-y-auto p-4">{center}</main>
          </Panel>

          <Separator className="w-px bg-border transition hover:bg-accent" />

          <Panel defaultSize="25%" minSize="15%" maxSize="45%" collapsible>
            <section className="h-full overflow-hidden border-l p-3">{right}</section>
          </Panel>
        </Group>
      )}
    </div>
  );
}

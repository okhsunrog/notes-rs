import type { ReactNode } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";

type Props = {
  status?: string;
  headerActions?: ReactNode;
  sidebar: ReactNode;
  center: ReactNode;
  right: ReactNode;
};

export function AppLayout({ status, headerActions, sidebar, center, right }: Props) {
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
    </div>
  );
}

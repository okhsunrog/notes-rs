import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, ArrowRight, Columns2, Loader2, X } from "lucide-react";
import { Group, Panel, Separator, usePanelRef } from "react-resizable-panels";
import { useCompactLayout } from "@/app/use-compact-layout";
import { Button } from "@/components/ui/button";
import { EmptyJournalView } from "@/features/journal/empty-journal-view";
import { GraphWorkspace } from "@/features/graph/knowledge-panel";
import { HomeView } from "@/features/home/home-view";
import { PagePresentation } from "@/features/pages/page-presentation";
import { PageView } from "@/features/pages/page-view";
import { usePageSessionRegistry } from "@/features/pages/page-session";
import { getPage, type SearchHit } from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import {
  PaneContentKind,
  WorkspaceNodeKind,
  currentDisposition,
  leafPaneIds,
  type PaneContent,
  type PaneId,
  type WorkspaceNode,
} from "./workspace-model";
import { useWorkspaceController } from "./workspace-controller";
import { useWorkspaceStore } from "./workspace-store";

type WorkbenchProps = {
  creatingNote: boolean;
  journalBusy: boolean;
  newNote: { pageUuid: string; blockUuid: string | null } | null;
  hits: SearchHit[];
  setHits: React.Dispatch<React.SetStateAction<SearchHit[]>>;
};

export function Workbench(props: WorkbenchProps) {
  const compact = useCompactLayout();
  const tree = useWorkspaceStore((state) => state.tree);
  const compactVisiblePaneId = useWorkspaceStore((state) => state.compactVisiblePaneId);
  const dispatch = useWorkspaceStore((state) => state.dispatch);
  const paneIds = leafPaneIds(tree);
  return (
    <div className="flex h-full min-h-0 flex-col">
      {compact && paneIds.length > 1 && (
        <div
          role="tablist"
          aria-label="Open workspace panes"
          className="flex h-10 shrink-0 items-center justify-center gap-1 border-b border-border/60 bg-card/70 px-2"
        >
          {paneIds.map((paneId, index) => (
            <Button
              key={paneId}
              type="button"
              role="tab"
              variant={compactVisiblePaneId === paneId ? "secondary" : "ghost"}
              size="xs"
              aria-selected={compactVisiblePaneId === paneId}
              onClick={() => dispatch({ type: "show_compact_pane", paneId })}
              className="rounded-lg"
            >
              <Columns2 className="size-3" />
              {index === 0 ? "Primary" : "Beside"}
            </Button>
          ))}
        </div>
      )}
      <div className="min-h-0 flex-1">
        <WorkspaceTree node={tree} compact={compact} {...props} />
      </div>
    </div>
  );
}

function WorkspaceTree({
  node,
  compact,
  ...props
}: WorkbenchProps & { node: WorkspaceNode; compact: boolean }) {
  if (node.kind === WorkspaceNodeKind.Pane) {
    return <PaneFrame paneId={node.paneId} compact={compact} {...props} />;
  }
  return <SplitFrame node={node} compact={compact} {...props} />;
}

function SplitFrame({
  node,
  compact,
  ...props
}: WorkbenchProps & {
  node: Extract<WorkspaceNode, { kind: WorkspaceNodeKind.Split }>;
  compact: boolean;
}) {
  const firstRef = usePanelRef();
  const secondRef = usePanelRef();
  const compactVisiblePaneId = useWorkspaceStore((state) => state.compactVisiblePaneId);
  const dispatch = useWorkspaceStore((state) => state.dispatch);
  const firstVisible = containsPane(node.first, compactVisiblePaneId);

  useEffect(() => {
    const first = firstRef.current;
    const second = secondRef.current;
    if (!first || !second) return;
    if (compact) {
      if (firstVisible) {
        first.expand();
        first.resize("100%");
        second.collapse();
      } else {
        second.expand();
        second.resize("100%");
        first.collapse();
      }
      return;
    }
    first.expand();
    second.expand();
    first.resize(`${node.ratio * 100}%`);
    second.resize(`${(1 - node.ratio) * 100}%`);
  }, [compact, firstRef, firstVisible, node.ratio, secondRef]);

  return (
    <Group orientation={node.axis} id={node.splitId} className="h-full min-h-0">
      <Panel
        id={`${node.splitId}-first`}
        panelRef={firstRef}
        defaultSize={`${node.ratio * 100}%`}
        minSize={compact ? 0 : "28%"}
        collapsible={compact}
        collapsedSize={0}
        onResize={(size, _id, previous) => {
          if (!compact && previous) {
            dispatch({
              type: "resize_split",
              splitId: node.splitId,
              ratio: size.asPercentage / 100,
            });
          }
        }}
      >
        <WorkspaceTree node={node.first} compact={compact} {...props} />
      </Panel>
      <Separator
        className={cn(
          "group relative bg-border/70 transition hover:bg-primary/50 focus-visible:bg-primary",
          node.axis === "horizontal"
            ? "w-px after:absolute after:inset-y-0 after:-left-1 after:w-2"
            : "h-px after:absolute after:inset-x-0 after:-top-1 after:h-2",
          compact && "hidden",
        )}
      />
      <Panel
        id={`${node.splitId}-second`}
        panelRef={secondRef}
        defaultSize={`${(1 - node.ratio) * 100}%`}
        minSize={compact ? 0 : "28%"}
        collapsible={compact}
        collapsedSize={0}
      >
        <WorkspaceTree node={node.second} compact={compact} {...props} />
      </Panel>
    </Group>
  );
}

function PaneFrame({
  paneId,
  compact,
  ...props
}: WorkbenchProps & { paneId: PaneId; compact: boolean }) {
  const pane = useWorkspaceStore((state) => state.panes[paneId]);
  const active = useWorkspaceStore((state) => state.activePaneId === paneId);
  const primary = useWorkspaceStore((state) => state.primaryPaneId === paneId);
  const compactVisible = useWorkspaceStore((state) => state.compactVisiblePaneId === paneId);
  const dispatch = useWorkspaceStore((state) => state.dispatch);
  if (!pane) return null;

  return (
    <section
      aria-label={primary ? "Primary workspace pane" : "Adjacent workspace pane"}
      aria-hidden={compact && !compactVisible ? true : undefined}
      inert={compact && !compactVisible ? true : undefined}
      data-pane-id={paneId}
      data-active={active || undefined}
      className={cn(
        "flex h-full min-h-0 flex-col bg-canvas transition-shadow",
        active && "ring-1 ring-inset ring-primary/15",
        compact && !compactVisible && "pointer-events-none invisible",
      )}
      onPointerDown={() => {
        if (!active) dispatch({ type: "focus_pane", paneId });
      }}
    >
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border/50 bg-card/55 px-2">
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          disabled={pane.back.length === 0}
          aria-label="Go back in pane"
          onClick={() => dispatch({ type: "go_back", paneId })}
        >
          <ArrowLeft className="size-3.5" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          disabled={pane.forward.length === 0}
          aria-label="Go forward in pane"
          onClick={() => dispatch({ type: "go_forward", paneId })}
        >
          <ArrowRight className="size-3.5" />
        </Button>
        <span className="ml-1 truncate text-[10px] font-medium tracking-wide text-muted-foreground uppercase">
          {paneLabel(pane.content, primary)}
        </span>
        {!primary && (
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            className="ml-auto"
            aria-label="Close adjacent pane"
            onClick={() => dispatch({ type: "close_pane", paneId })}
          >
            <X className="size-3.5" />
          </Button>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <PaneSurface paneId={paneId} content={pane.content} {...props} />
      </div>
    </section>
  );
}

function PaneSurface({
  paneId,
  content,
  ...props
}: WorkbenchProps & { paneId: PaneId; content: PaneContent }) {
  const controller = useWorkspaceController();
  const dispatch = useWorkspaceStore((state) => state.dispatch);
  switch (content.kind) {
    case PaneContentKind.Home:
      return (
        <HomeView
          creating={props.creatingNote}
          journalBusy={props.journalBusy}
          hits={props.hits}
          setHits={props.setHits}
          onCreate={controller.createNewNote}
          onOpenJournal={controller.openJournal}
          onOpenContent={controller.openContent}
        />
      );
    case PaneContentKind.Page:
      return <PagePane paneId={paneId} content={content} {...props} />;
    case PaneContentKind.Graph:
      return <GraphPane content={content} paneId={paneId} />;
    case PaneContentKind.JournalDay:
      return (
        <EmptyJournalView
          date={content.date}
          busy={props.journalBusy}
          onCapture={controller.captureJournal}
          onClose={() => dispatch({ type: "close_pane", paneId })}
          onOpenDate={(date, disposition) =>
            controller.openJournal(date, disposition ?? currentDisposition)
          }
        />
      );
  }
}

function PagePane({
  paneId,
  content,
  ...props
}: WorkbenchProps & {
  paneId: PaneId;
  content: Extract<PaneContent, { kind: PaneContentKind.Page }>;
}) {
  const pageSessions = usePageSessionRegistry();
  const controller = useWorkspaceController();
  const active = useWorkspaceStore((state) => state.activePaneId === paneId);
  const dispatch = useWorkspaceStore((state) => state.dispatch);
  const pageQuery = useQuery({
    queryKey: queryKeys.page(content.pageUuid),
    queryFn: () => getPage(content.pageUuid),
  });
  const page = pageQuery.data;

  useEffect(() => {
    if (pageQuery.isSuccess && page === null) {
      pageSessions.discardPage(content.pageUuid);
    }
  }, [content.pageUuid, page, pageQuery.isSuccess, pageSessions]);

  if (pageQuery.isPending) {
    return <PaneLoading />;
  }
  if (pageQuery.error) {
    return <PaneError message={String(pageQuery.error)} />;
  }
  if (!page) {
    return <PaneError message="This page no longer exists." />;
  }

  return (
    <PageView
      key={page.uuid}
      paneId={paneId}
      page={page}
      initialBlockUuid={
        props.newNote?.pageUuid === page.uuid ? props.newNote.blockUuid : content.blockUuid
      }
      autoFocusTitle={active && props.newNote?.pageUuid === page.uuid}
      presentation={content.presentation}
      onPresentationChange={(presentation) =>
        dispatch({ type: "set_page_presentation", paneId, presentation })
      }
      onSaved={controller.onSaved}
      onClose={() => dispatch({ type: "close_pane", paneId })}
      onDelete={controller.onDelete}
      onOpenMarkdownLink={controller.openMarkdownLink}
      onOpenJournalDate={(date, disposition) =>
        controller.openJournal(date, disposition ?? currentDisposition)
      }
      journalBusy={props.journalBusy}
    />
  );
}

function GraphPane({
  content,
  paneId,
}: {
  paneId: PaneId;
  content: Extract<PaneContent, { kind: PaneContentKind.Graph }>;
}) {
  const controller = useWorkspaceController();
  const dispatch = useWorkspaceStore((state) => state.dispatch);
  const pageQuery = useQuery({
    queryKey: queryKeys.page(content.focusPageUuid ?? "graph-root"),
    queryFn: () => getPage(content.focusPageUuid as string),
    enabled: content.focusPageUuid !== null,
  });
  return (
    <GraphWorkspace
      page={pageQuery.data ?? null}
      onOpenContent={controller.openContent}
      onClose={() => dispatch({ type: "go_back", paneId })}
    />
  );
}

function PaneLoading() {
  return (
    <div className="flex h-full min-h-64 items-center justify-center">
      <Loader2 className="size-5 animate-spin text-muted-foreground" />
    </div>
  );
}

function PaneError({ message }: { message: string }) {
  return (
    <div className="flex h-full min-h-64 items-center justify-center p-6 text-sm text-destructive">
      {message}
    </div>
  );
}

function paneLabel(content: PaneContent, primary: boolean) {
  const location = primary ? "Primary" : "Beside";
  switch (content.kind) {
    case PaneContentKind.Home:
      return `${location} · Home`;
    case PaneContentKind.Page:
      return `${location} · ${content.presentation === PagePresentation.Reading ? "Reading" : "Page"}`;
    case PaneContentKind.Graph:
      return `${location} · Graph`;
    case PaneContentKind.JournalDay:
      return `${location} · Journal`;
  }
}

function containsPane(node: WorkspaceNode, paneId: PaneId): boolean {
  if (node.kind === WorkspaceNodeKind.Pane) return node.paneId === paneId;
  return containsPane(node.first, paneId) || containsPane(node.second, paneId);
}

import { QueryClient } from "@tanstack/react-query";
import { events, type DomainEvent } from "@/lib/bindings";

const root = ["backend"] as const;

export const queryKeys = {
  root,
  pages: [...root, "pages"] as const,
  entities: [...root, "entities"] as const,
  nodes: [...root, "node"] as const,
  node: (uuid: string) => [...root, "node", uuid] as const,
  childrenRoot: [...root, "children"] as const,
  children: (parentUuid: string) => [...root, "children", parentUuid] as const,
  graphRoot: [...root, "graph"] as const,
  graph: (focusUuid?: string | null) => [...root, "graph", focusUuid ?? "all"] as const,
  backlinksRoot: [...root, "backlinks"] as const,
  backlinks: (uuid: string) => [...root, "backlinks", uuid] as const,
  attachmentsRoot: [...root, "attachments"] as const,
  attachments: (parentUuid: string) => [...root, "attachments", parentUuid] as const,
  history: [...root, "history"] as const,
  backgroundStatus: [...root, "background-status"] as const,
  settings: [...root, "settings"] as const,
  syncStatus: [...root, "sync-status"] as const,
};

export function createAppQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: Number.POSITIVE_INFINITY,
        retry: 1,
        refetchOnWindowFocus: false,
      },
    },
  });
}

export async function applyDomainEvent(queryClient: QueryClient, event: DomainEvent) {
  const invalidate = (queryKey: readonly unknown[]) => queryClient.invalidateQueries({ queryKey });

  switch (event.kind) {
    case "node_changed":
    case "node_deleted": {
      if (event.kind === "node_deleted") {
        for (const uuid of event.node_uuids) {
          queryClient.removeQueries({ queryKey: queryKeys.node(uuid), exact: true });
        }
      }
      await Promise.all([
        ...event.node_uuids.map((uuid) => invalidate(queryKeys.node(uuid))),
        ...event.parent_uuids.map((uuid) => invalidate(queryKeys.children(uuid))),
        invalidate(queryKeys.pages),
        invalidate(queryKeys.entities),
        invalidate(queryKeys.graphRoot),
        invalidate(queryKeys.backlinksRoot),
        invalidate(queryKeys.attachmentsRoot),
        invalidate(queryKeys.backgroundStatus),
      ]);
      return;
    }
    case "graph_changed":
      await Promise.all([
        invalidate(queryKeys.graphRoot),
        invalidate(queryKeys.backlinksRoot),
        invalidate(queryKeys.entities),
        invalidate(queryKeys.backgroundStatus),
      ]);
      return;
    case "history_changed":
      await invalidate(queryKeys.history);
      return;
    case "background_status_changed":
      await invalidate(queryKeys.backgroundStatus);
      return;
    case "settings_changed":
      await invalidate(queryKeys.settings);
      return;
    case "sync_status_changed":
      await invalidate(queryKeys.syncStatus);
      return;
    case "workspace_changed":
      await invalidate(queryKeys.root);
  }
}

export function listenForDomainEvents(queryClient: QueryClient) {
  return events.domainEvent.listen(({ payload }) => {
    void applyDomainEvent(queryClient, payload);
  });
}

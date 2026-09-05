import { QueryClient } from "@tanstack/react-query";
import { events, type DomainEvent } from "@/lib/bindings";

const root = ["backend"] as const;

export const queryKeys = {
  root,
  pages: [...root, "pages"] as const,
  pageList: (filter: "notes" | "journals" | "all", limit: number) =>
    [...root, "pages", filter, limit] as const,
  journals: [...root, "journals"] as const,
  journal: (date: string) => [...root, "journals", date] as const,
  pageRoot: [...root, "page"] as const,
  page: (uuid: string) => [...root, "page", uuid] as const,
  pageDocumentRoot: [...root, "page-document"] as const,
  pageDocument: (uuid: string) => [...root, "page-document", uuid] as const,
  pageRenderRoot: [...root, "page-render"] as const,
  pageRender: (uuid: string) => [...root, "page-render", uuid] as const,
  blockRoot: [...root, "block"] as const,
  block: (uuid: string) => [...root, "block", uuid] as const,
  childrenRoot: [...root, "children"] as const,
  children: (containerUuid: string) => [...root, "children", containerUuid] as const,
  graphRoot: [...root, "graph"] as const,
  graph: (focusUuid?: string | null) => [...root, "graph", focusUuid ?? "all"] as const,
  backlinksRoot: [...root, "backlinks"] as const,
  backlinks: (uuid: string) => [...root, "backlinks", uuid] as const,
  attachmentsRoot: [...root, "attachments"] as const,
  attachments: (ownerUuid: string) => [...root, "attachments", ownerUuid] as const,
  attachmentImagesRoot: [...root, "attachments", "images"] as const,
  attachmentImages: (attachmentUuids: readonly string[]) =>
    [...root, "attachments", "images", ...attachmentUuids] as const,
  history: [...root, "history"] as const,
  settings: [...root, "settings"] as const,
  inputCapabilities: [...root, "input-capabilities"] as const,
  syncStatus: [...root, "sync-status"] as const,
  serverAi: [...root, "server-ai"] as const,
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
    case "pages_changed":
      await Promise.all([
        invalidate(queryKeys.pages),
        invalidate(queryKeys.journals),
        invalidate(queryKeys.graphRoot),
        invalidate(queryKeys.backlinksRoot),
        ...event.page_uuids.map((uuid) => invalidate(queryKeys.page(uuid))),
      ]);
      return;
    case "blocks_changed":
      await Promise.all([
        invalidate(queryKeys.pageDocumentRoot),
        invalidate(queryKeys.pageRenderRoot),
        ...event.block_uuids.map((uuid) => invalidate(queryKeys.block(uuid))),
        ...event.container_uuids.map((uuid) => invalidate(queryKeys.children(uuid))),
      ]);
      return;
    case "pages_deleted": {
      for (const uuid of event.page_uuids) {
        queryClient.removeQueries({ queryKey: queryKeys.page(uuid), exact: true });
        queryClient.removeQueries({ queryKey: queryKeys.pageDocument(uuid), exact: true });
        queryClient.removeQueries({ queryKey: queryKeys.pageRender(uuid), exact: true });
        queryClient.removeQueries({ queryKey: queryKeys.children(uuid), exact: true });
      }
      await Promise.all([
        invalidate(queryKeys.pages),
        invalidate(queryKeys.journals),
        invalidate(queryKeys.attachmentsRoot),
        invalidate(queryKeys.graphRoot),
        invalidate(queryKeys.backlinksRoot),
      ]);
      return;
    }
    case "blocks_deleted": {
      for (const uuid of event.block_uuids) {
        queryClient.removeQueries({ queryKey: queryKeys.block(uuid), exact: true });
        queryClient.removeQueries({ queryKey: queryKeys.children(uuid), exact: true });
      }
      await Promise.all([
        invalidate(queryKeys.pageDocumentRoot),
        invalidate(queryKeys.pageRenderRoot),
        ...event.container_uuids.map((uuid) => invalidate(queryKeys.children(uuid))),
        invalidate(queryKeys.attachmentsRoot),
        invalidate(queryKeys.graphRoot),
        invalidate(queryKeys.backlinksRoot),
      ]);
      return;
    }
    case "graph_changed":
      await Promise.all([invalidate(queryKeys.graphRoot), invalidate(queryKeys.backlinksRoot)]);
      return;
    case "structure_changed":
      await Promise.all([
        invalidate(queryKeys.pageDocumentRoot),
        invalidate(queryKeys.pageRenderRoot),
        ...event.block_uuids.map((uuid) => invalidate(queryKeys.block(uuid))),
        invalidate(queryKeys.childrenRoot),
      ]);
      return;
    case "attachments_changed":
      await Promise.all([
        invalidate(queryKeys.attachmentImagesRoot),
        invalidate(queryKeys.pageRenderRoot),
        ...event.owner_uuids.map((uuid) => invalidate(queryKeys.attachments(uuid))),
      ]);
      return;
    case "history_changed":
      await invalidate(queryKeys.history);
      return;
    case "settings_changed":
      await invalidate(queryKeys.settings);
      return;
    case "sync_status_changed":
      await invalidate(queryKeys.syncStatus);
      return;
    case "server_ai_changed":
      await invalidate(queryKeys.serverAi);
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

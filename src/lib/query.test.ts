import { describe, expect, it } from "vite-plus/test";
import { QueryClient } from "@tanstack/react-query";
import { applyDomainEvent, queryKeys } from "./query";

describe("domain event query invalidation", () => {
  it("invalidates the changed node, its parent children and derived views", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.node("node-a"), { uuid: "node-a" });
    client.setQueryData(queryKeys.node("node-b"), { uuid: "node-b" });
    client.setQueryData(queryKeys.children("parent-a"), []);
    client.setQueryData(queryKeys.pages, []);
    client.setQueryData(queryKeys.graph(null), { nodes: [], edges: [] });

    await applyDomainEvent(client, {
      kind: "node_changed",
      node_uuids: ["node-a"],
      parent_uuids: ["parent-a"],
      node_kinds: ["block"],
    });

    expect(client.getQueryState(queryKeys.node("node-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.node("node-b"))?.isInvalidated).toBe(false);
    expect(client.getQueryState(queryKeys.children("parent-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.pages)?.isInvalidated).toBe(false);
    expect(client.getQueryState(queryKeys.graph(null))?.isInvalidated).toBe(false);
  });

  it("removes deleted node snapshots", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.node("deleted"), { uuid: "deleted" });

    await applyDomainEvent(client, {
      kind: "node_deleted",
      node_uuids: ["deleted"],
      parent_uuids: [],
    });

    expect(client.getQueryData(queryKeys.node("deleted"))).toBeUndefined();
  });

  it("invalidates the complete backend cache after a workspace replacement", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.settings, { windowDecorationMode: "native" });
    client.setQueryData(queryKeys.history, [1, 0]);

    await applyDomainEvent(client, { kind: "workspace_changed" });

    expect(client.getQueryState(queryKeys.settings)?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.history)?.isInvalidated).toBe(true);
  });

  it("invalidates server AI state after a remote setting mutation", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.serverAi, { generationState: "active" });

    await applyDomainEvent(client, { kind: "server_ai_changed" });

    expect(client.getQueryState(queryKeys.serverAi)?.isInvalidated).toBe(true);
  });

  it("invalidates structure without refetching graph data", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.children("parent-a"), []);
    client.setQueryData(queryKeys.children("parent-b"), []);
    client.setQueryData(queryKeys.graph(null), { nodes: [], edges: [] });

    await applyDomainEvent(client, {
      kind: "structure_changed",
      node_uuids: ["moved"],
    });

    expect(client.getQueryState(queryKeys.children("parent-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.children("parent-b"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.graph(null))?.isInvalidated).toBe(false);
  });

  it("invalidates only attachments for affected parents", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.attachments("parent-a"), []);
    client.setQueryData(queryKeys.attachments("parent-b"), []);

    await applyDomainEvent(client, {
      kind: "attachments_changed",
      parent_uuids: ["parent-a"],
    });

    expect(client.getQueryState(queryKeys.attachments("parent-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.attachments("parent-b"))?.isInvalidated).toBe(false);
  });
});

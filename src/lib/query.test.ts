import { describe, expect, it } from "vite-plus/test";
import { QueryClient } from "@tanstack/react-query";
import { applyDomainEvent, queryKeys } from "./query";

describe("domain event query invalidation", () => {
  it("invalidates a changed page and title-derived views", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.pages, []);
    client.setQueryData(queryKeys.journals, []);
    client.setQueryData(queryKeys.journal("2026-07-17"), { uuid: "page-a" });
    client.setQueryData(queryKeys.page("page-a"), { uuid: "page-a" });
    client.setQueryData(queryKeys.graph(null), { items: [], edges: [] });

    await applyDomainEvent(client, {
      kind: "pages_changed",
      page_uuids: ["page-a"],
    });

    expect(client.getQueryState(queryKeys.pages)?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.journals)?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.journal("2026-07-17"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.page("page-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.graph(null))?.isInvalidated).toBe(true);
  });

  it("invalidates a changed block and its containing child list", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.block("block-a"), { uuid: "block-a" });
    client.setQueryData(queryKeys.block("block-b"), { uuid: "block-b" });
    client.setQueryData(queryKeys.children("parent-a"), []);
    client.setQueryData(queryKeys.pages, []);
    client.setQueryData(queryKeys.graph(null), { items: [], edges: [] });

    await applyDomainEvent(client, {
      kind: "blocks_changed",
      block_uuids: ["block-a"],
      container_uuids: ["parent-a"],
    });

    expect(client.getQueryState(queryKeys.block("block-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.block("block-b"))?.isInvalidated).toBe(false);
    expect(client.getQueryState(queryKeys.children("parent-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.pages)?.isInvalidated).toBe(false);
    expect(client.getQueryState(queryKeys.graph(null))?.isInvalidated).toBe(false);
  });

  it("removes deleted block snapshots", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.block("deleted"), { uuid: "deleted" });

    await applyDomainEvent(client, {
      kind: "blocks_deleted",
      block_uuids: ["deleted"],
      container_uuids: [],
    });

    expect(client.getQueryData(queryKeys.block("deleted"))).toBeUndefined();
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
    client.setQueryData(queryKeys.graph(null), { items: [], edges: [] });

    await applyDomainEvent(client, {
      kind: "structure_changed",
      block_uuids: ["moved"],
    });

    expect(client.getQueryState(queryKeys.children("parent-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.children("parent-b"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.graph(null))?.isInvalidated).toBe(false);
  });

  it("invalidates only attachments for affected owners", async () => {
    const client = new QueryClient();
    client.setQueryData(queryKeys.attachments("parent-a"), []);
    client.setQueryData(queryKeys.attachments("parent-b"), []);

    await applyDomainEvent(client, {
      kind: "attachments_changed",
      owner_uuids: ["parent-a"],
    });

    expect(client.getQueryState(queryKeys.attachments("parent-a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(queryKeys.attachments("parent-b"))?.isInvalidated).toBe(false);
  });
});

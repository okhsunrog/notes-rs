import { describe, expect, it, vi } from "vite-plus/test";
import type { PaneId } from "@/features/workspace/workspace-model";
import { encodeDocument } from "@/features/document/document-codec";
import { PageSessionRegistry } from "./page-session";

const persisted = (text: string, revision: string) => ({ text, revision });
const paneId = (value: string) => value as PaneId;
const document = (buffer: string, revision: string) => {
  const encoded = encodeDocument([
    {
      uuid: `block-${revision}`,
      parentUuid: null,
      style: { kind: "paragraph" },
      markdown: buffer,
    },
  ]);
  return { buffer: encoded.markdown, sourceMap: encoded.sourceMap, revision };
};

describe("PageSessionRegistry", () => {
  it("grants one idempotent writer lease per page and ignores stale releases", () => {
    const registry = new PageSessionRegistry();
    const first = registry.createWriterLeaseToken();
    const second = registry.createWriterLeaseToken();
    const other = registry.createWriterLeaseToken();
    const paneA = paneId("pane-a");
    const paneB = paneId("pane-b");

    expect(registry.acquireWriter("page", paneA, first)).toBe(true);
    expect(registry.acquireWriter("page", paneA, first)).toBe(true);
    expect(registry.acquireWriter("page", paneA, second)).toBe(true);
    expect(registry.acquireWriter("page", paneB, other)).toBe(false);
    expect(registry.getSnapshot("page").writerPaneId).toBe(paneA);

    registry.releaseWriter("page", paneA, first);
    expect(registry.getSnapshot("page").writerPaneId).toBe(paneA);
    registry.releaseWriter("page", paneB, second);
    expect(registry.getSnapshot("page").writerPaneId).toBe(paneA);
    registry.releaseWriter("page", paneA, second);
    expect(registry.getSnapshot("page").writerPaneId).toBeNull();
  });

  it("publishes title and block drafts immediately and retains them across consumers", () => {
    const registry = new PageSessionRegistry();
    const listener = vi.fn();
    registry.subscribe("page", listener);

    registry.editTitle("page", "Draft title", persisted("Persisted title", "t1"));
    registry.editBlock("page", "block", "Draft body", persisted("Persisted body", "b1"));

    const firstConsumer = registry.getSnapshot("page");
    expect(firstConsumer.title?.draft).toBe("Draft title");
    expect(firstConsumer.blocks.block?.draft).toBe("Draft body");
    expect(listener).toHaveBeenCalledTimes(2);

    // A remounted editor or linked Reading pane observes the same registry,
    // while no persisted page/block snapshot has been copied into clean state.
    const remountedConsumer = registry.getSnapshot("page");
    expect(remountedConsumer).toBe(firstConsumer);
    expect(new PageSessionRegistry().getSnapshot("page").title).toBeNull();
  });

  it("keeps typing during an in-flight acknowledgement dirty", () => {
    const registry = new PageSessionRegistry();
    registry.editBlock("page", "block", "first draft", persisted("old", "r1"));
    const attempt = registry.beginBlockSave("page", "block");
    expect(attempt).toMatchObject({ draft: "first draft", expectedRevision: "r1" });

    registry.editBlock("page", "block", "newer draft", persisted("old", "r1"));
    registry.acknowledgeBlockSave("page", "block", attempt!, persisted("first draft", "r2"));

    const overlay = registry.getSnapshot("page").blocks.block;
    expect(overlay).toMatchObject({
      draft: "newer draft",
      baseText: "first draft",
      baseRevision: "r2",
      inFlight: null,
    });
    expect(registry.beginBlockSave("page", "block")).toMatchObject({
      draft: "newer draft",
      expectedRevision: "r2",
    });
  });

  it("clears only the exact acknowledged draft", () => {
    const registry = new PageSessionRegistry();
    registry.editTitle("page", "draft", persisted("old", "r1"));
    const attempt = registry.beginTitleSave("page")!;

    registry.acknowledgeTitleSave("page", attempt, persisted("normalized draft", "r2"));

    expect(registry.getSnapshot("page").title).toBeNull();
  });

  it("accepts clean-equivalent snapshots and exposes divergent remote conflicts", () => {
    const registry = new PageSessionRegistry();
    registry.editBlock("page", "block", "mine", persisted("base", "r1"));

    registry.acceptBlockSnapshot("page", "block", persisted("base", "r2"));
    expect(registry.getSnapshot("page").blocks.block).toMatchObject({
      draft: "mine",
      baseRevision: "r2",
      conflict: null,
    });

    registry.acceptBlockSnapshot("page", "block", persisted("remote", "r3"));
    expect(registry.getSnapshot("page").blocks.block?.conflict).toEqual({
      remoteText: "remote",
      remoteRevision: "r3",
    });

    registry.keepLocalBlock("page", "block");
    expect(registry.getSnapshot("page").blocks.block).toMatchObject({
      draft: "mine",
      baseText: "remote",
      baseRevision: "r3",
      conflict: null,
    });

    registry.acceptBlockSnapshot("page", "block", persisted("remote-2", "r4"));
    registry.useRemoteBlock("page", "block");
    expect(registry.getSnapshot("page").blocks.block).toBeUndefined();
  });

  it("treats an event carrying the in-flight text as acknowledgement without losing newer text", () => {
    const registry = new PageSessionRegistry();
    registry.editBlock("page", "block", "saving", persisted("base", "r1"));
    registry.beginBlockSave("page", "block");
    registry.editBlock("page", "block", "typed later", persisted("base", "r1"));

    registry.acceptBlockSnapshot("page", "block", persisted("saving", "r2"));

    expect(registry.getSnapshot("page").blocks.block).toMatchObject({
      draft: "typed later",
      baseText: "saving",
      baseRevision: "r2",
      inFlight: null,
      conflict: null,
    });
  });

  it("turns an authoritative snapshot fetched after a rejected save into a conflict", () => {
    const registry = new PageSessionRegistry();
    registry.editTitle("page", "mine", persisted("base", "r1"));
    const attempt = registry.beginTitleSave("page")!;

    registry.failTitleSave("page", attempt);
    registry.acceptTitleSnapshot("page", persisted("remote", "r2"));

    expect(registry.getSnapshot("page").title).toMatchObject({
      draft: "mine",
      inFlight: null,
      conflict: { remoteText: "remote", remoteRevision: "r2" },
    });
  });

  it("publishes the exact continuous document draft with its typed base", () => {
    const registry = new PageSessionRegistry();
    const base = document("Persisted", "d1");

    registry.editDocument("page", "# Exact\n\n- [ ] draft", base);

    expect(registry.getSnapshot("page").document).toMatchObject({
      draft: "# Exact\n\n- [ ] draft",
      baseBuffer: "Persisted",
      baseRevision: "d1",
      baseSourceMap: base.sourceMap,
    });
  });

  it("rebases a successful document save without losing newer typing", () => {
    const registry = new PageSessionRegistry();
    const base = document("Base", "d1");
    registry.editDocument("page", "First draft", base);
    const attempt = registry.beginDocumentSave("page")!;

    registry.editDocument("page", "Newer typing", base);
    const saved = document("First draft", "d2");
    registry.acknowledgeDocumentSave("page", attempt, saved);

    expect(registry.getSnapshot("page").document).toMatchObject({
      draft: "Newer typing",
      baseBuffer: "First draft",
      baseRevision: "d2",
      baseSourceMap: saved.sourceMap,
      inFlight: null,
    });
    expect(registry.beginDocumentSave("page")).toMatchObject({
      draft: "Newer typing",
      expectedRevision: "d2",
      baseSourceMap: saved.sourceMap,
    });
  });

  it("settles an in-flight save when its domain refresh arrives before the RPC response", () => {
    const registry = new PageSessionRegistry();
    const base = document("Base", "d1");
    registry.editDocument("page", "Saved draft", base);
    const attempt = registry.beginDocumentSave("page")!;

    registry.acceptDocumentSnapshot("page", document("Saved draft", "d2"));

    expect(registry.getSnapshot("page").document).toBeNull();
    registry.acknowledgeDocumentSave("page", attempt, document("Saved draft", "d2"));
    expect(registry.beginDocumentSave("page")).toBeNull();
  });

  it("turns invalidated remote document data into a conflict without overwriting the draft", () => {
    const registry = new PageSessionRegistry();
    registry.editDocument("page", "Mine", document("Base", "d1"));

    const remote = document("Remote", "d2");
    registry.acceptDocumentSnapshot("page", remote);

    expect(registry.getSnapshot("page").document).toMatchObject({
      draft: "Mine",
      conflict: { remote },
    });
    registry.keepLocalDocument("page");
    expect(registry.getSnapshot("page").document).toMatchObject({
      draft: "Mine",
      baseBuffer: "Remote",
      baseRevision: "d2",
      conflict: null,
    });

    registry.acceptDocumentSnapshot("page", document("Remote again", "d3"));
    registry.useRemoteDocument("page");
    expect(registry.getSnapshot("page").document).toBeNull();
  });

  it("discards drafts and writer ownership when a remotely deleted page disappears", () => {
    const registry = new PageSessionRegistry();
    const token = registry.createWriterLeaseToken();
    registry.acquireWriter("page", paneId("pane-a"), token);
    registry.editBlock("page", "block", "mine", persisted("base", "r1"));

    registry.discardPage("page");

    expect(registry.getSnapshot("page")).toMatchObject({
      writerPaneId: null,
      title: null,
      blocks: {},
      document: null,
    });
  });
});

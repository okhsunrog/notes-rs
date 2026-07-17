import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { PaneId } from "@/features/workspace/workspace-model";
import type { DocumentSourceMap } from "@/features/document/document-codec";
import type { ContentRevision, DocumentRevision } from "@/lib/api";

export type PersistedTextSnapshot = Readonly<{
  text: string;
  revision: ContentRevision;
}>;

export type DraftConflict = Readonly<{
  remoteText: string;
  remoteRevision: ContentRevision;
}>;

export type DraftSaveAttempt = Readonly<{
  id: number;
  draft: string;
  expectedRevision: ContentRevision;
}>;

export type DraftOverlay = Readonly<{
  draft: string;
  baseText: string;
  baseRevision: ContentRevision;
  conflict: DraftConflict | null;
  inFlight: DraftSaveAttempt | null;
}>;

export type PersistedDocumentSnapshot = Readonly<{
  buffer: string;
  sourceMap: DocumentSourceMap;
  revision: DocumentRevision;
}>;

export type DocumentDraftConflict = Readonly<{
  remote: PersistedDocumentSnapshot;
}>;

export type DocumentSaveAttempt = Readonly<{
  id: number;
  draft: string;
  baseSourceMap: DocumentSourceMap;
  expectedRevision: DocumentRevision;
}>;

export type DocumentDraftOverlay = Readonly<{
  draft: string;
  baseBuffer: string;
  baseSourceMap: DocumentSourceMap;
  baseRevision: DocumentRevision;
  conflict: DocumentDraftConflict | null;
  inFlight: DocumentSaveAttempt | null;
}>;

export type PageSessionSnapshot = Readonly<{
  writerPaneId: PaneId | null;
  title: DraftOverlay | null;
  blocks: Readonly<Record<string, DraftOverlay>>;
  document: DocumentDraftOverlay | null;
}>;

export type WriterLeaseToken = Readonly<{ readonly id: symbol }>;

type DraftTarget = Readonly<
  | { kind: "title" }
  | {
      kind: "block";
      blockUuid: string;
    }
>;

type SessionRecord = {
  snapshot: PageSessionSnapshot;
  writerTokens: Set<WriterLeaseToken>;
};

const EMPTY_BLOCKS: Readonly<Record<string, DraftOverlay>> = Object.freeze({});
const EMPTY_SESSION: PageSessionSnapshot = Object.freeze({
  writerPaneId: null,
  title: null,
  blocks: EMPTY_BLOCKS,
  document: null,
});

/**
 * Window-local ownership for unsaved text only. Rust/SQLite and TanStack Query
 * remain the owners of every clean persisted snapshot.
 */
export class PageSessionRegistry {
  readonly #sessions = new Map<string, SessionRecord>();
  readonly #listeners = new Map<string, Set<() => void>>();
  #nextAttemptId = 1;

  createWriterLeaseToken(): WriterLeaseToken {
    return Object.freeze({ id: Symbol("page-writer-lease") });
  }

  getSnapshot(pageUuid: string): PageSessionSnapshot {
    return this.#sessions.get(pageUuid)?.snapshot ?? EMPTY_SESSION;
  }

  subscribe(pageUuid: string, listener: () => void): () => void {
    let listeners = this.#listeners.get(pageUuid);
    if (!listeners) {
      listeners = new Set();
      this.#listeners.set(pageUuid, listeners);
    }
    listeners.add(listener);
    return () => {
      listeners?.delete(listener);
      if (listeners?.size === 0) this.#listeners.delete(pageUuid);
    };
  }

  acquireWriter(pageUuid: string, paneId: PaneId, token: WriterLeaseToken): boolean {
    const record = this.#record(pageUuid);
    const owner = record.snapshot.writerPaneId;
    if (owner !== null && owner !== paneId) return false;
    if (record.writerTokens.has(token)) return true;
    record.writerTokens.add(token);
    if (owner === null) {
      this.#publish(pageUuid, record, { ...record.snapshot, writerPaneId: paneId });
    }
    return true;
  }

  releaseWriter(pageUuid: string, paneId: PaneId, token: WriterLeaseToken): void {
    const record = this.#sessions.get(pageUuid);
    if (!record || record.snapshot.writerPaneId !== paneId || !record.writerTokens.delete(token)) {
      return;
    }
    if (record.writerTokens.size === 0) {
      this.#publish(pageUuid, record, { ...record.snapshot, writerPaneId: null });
    }
  }

  editTitle(pageUuid: string, draft: string, persisted: PersistedTextSnapshot): void {
    this.#edit(pageUuid, { kind: "title" }, draft, persisted);
  }

  editBlock(
    pageUuid: string,
    blockUuid: string,
    draft: string,
    persisted: PersistedTextSnapshot,
  ): void {
    this.#edit(pageUuid, { kind: "block", blockUuid }, draft, persisted);
  }

  acceptTitleSnapshot(pageUuid: string, persisted: PersistedTextSnapshot): void {
    this.#acceptPersisted(pageUuid, { kind: "title" }, persisted);
  }

  acceptBlockSnapshot(pageUuid: string, blockUuid: string, persisted: PersistedTextSnapshot): void {
    this.#acceptPersisted(pageUuid, { kind: "block", blockUuid }, persisted);
  }

  beginTitleSave(pageUuid: string): DraftSaveAttempt | null {
    return this.#beginSave(pageUuid, { kind: "title" });
  }

  beginBlockSave(pageUuid: string, blockUuid: string): DraftSaveAttempt | null {
    return this.#beginSave(pageUuid, { kind: "block", blockUuid });
  }

  acknowledgeTitleSave(
    pageUuid: string,
    attempt: DraftSaveAttempt,
    persisted: PersistedTextSnapshot,
  ): void {
    this.#acknowledgeSave(pageUuid, { kind: "title" }, attempt, persisted);
  }

  acknowledgeBlockSave(
    pageUuid: string,
    blockUuid: string,
    attempt: DraftSaveAttempt,
    persisted: PersistedTextSnapshot,
  ): void {
    this.#acknowledgeSave(pageUuid, { kind: "block", blockUuid }, attempt, persisted);
  }

  failTitleSave(pageUuid: string, attempt: DraftSaveAttempt): void {
    this.#failSave(pageUuid, { kind: "title" }, attempt);
  }

  failBlockSave(pageUuid: string, blockUuid: string, attempt: DraftSaveAttempt): void {
    this.#failSave(pageUuid, { kind: "block", blockUuid }, attempt);
  }

  useRemoteTitle(pageUuid: string): void {
    this.#useRemote(pageUuid, { kind: "title" });
  }

  useRemoteBlock(pageUuid: string, blockUuid: string): void {
    this.#useRemote(pageUuid, { kind: "block", blockUuid });
  }

  keepLocalTitle(pageUuid: string): void {
    this.#keepLocal(pageUuid, { kind: "title" });
  }

  keepLocalBlock(pageUuid: string, blockUuid: string): void {
    this.#keepLocal(pageUuid, { kind: "block", blockUuid });
  }

  editDocument(pageUuid: string, draft: string, persisted: PersistedDocumentSnapshot): void {
    const record = this.#record(pageUuid);
    const current = record.snapshot.document;
    if (!current) {
      if (draft === persisted.buffer) return;
      this.#setDocument(pageUuid, record, {
        draft,
        baseBuffer: persisted.buffer,
        baseSourceMap: persisted.sourceMap,
        baseRevision: persisted.revision,
        conflict: null,
        inFlight: null,
      });
      return;
    }
    if (current.draft === draft) return;
    if (draft === current.baseBuffer && current.conflict === null && current.inFlight === null) {
      this.#setDocument(pageUuid, record, null);
      return;
    }
    this.#setDocument(pageUuid, record, { ...current, draft });
  }

  acceptDocumentSnapshot(pageUuid: string, persisted: PersistedDocumentSnapshot): void {
    const record = this.#sessions.get(pageUuid);
    const current = record?.snapshot.document;
    if (!record || !current) return;
    if (persisted.buffer === current.draft) {
      this.#setDocument(pageUuid, record, null);
      return;
    }
    if (current.inFlight?.draft === persisted.buffer) {
      if (current.draft === current.inFlight.draft) {
        this.#setDocument(pageUuid, record, null);
        return;
      }
      this.#setDocument(pageUuid, record, {
        ...current,
        baseBuffer: persisted.buffer,
        baseSourceMap: persisted.sourceMap,
        baseRevision: persisted.revision,
        conflict: null,
        inFlight: null,
      });
      return;
    }
    if (persisted.buffer === current.baseBuffer) {
      if (
        persisted.revision !== current.baseRevision ||
        persisted.sourceMap !== current.baseSourceMap
      ) {
        this.#setDocument(pageUuid, record, {
          ...current,
          baseSourceMap: persisted.sourceMap,
          baseRevision: persisted.revision,
        });
      }
      return;
    }
    if (
      current.conflict?.remote.buffer === persisted.buffer &&
      current.conflict.remote.revision === persisted.revision
    ) {
      return;
    }
    this.#setDocument(pageUuid, record, {
      ...current,
      conflict: { remote: persisted },
      inFlight: null,
    });
  }

  beginDocumentSave(pageUuid: string): DocumentSaveAttempt | null {
    const record = this.#sessions.get(pageUuid);
    const current = record?.snapshot.document;
    if (!record || !current || current.conflict || current.inFlight) return null;
    const attempt = Object.freeze({
      id: this.#nextAttemptId++,
      draft: current.draft,
      baseSourceMap: current.baseSourceMap,
      expectedRevision: current.baseRevision,
    });
    this.#setDocument(pageUuid, record, { ...current, inFlight: attempt });
    return attempt;
  }

  acknowledgeDocumentSave(
    pageUuid: string,
    attempt: DocumentSaveAttempt,
    persisted: PersistedDocumentSnapshot,
  ): void {
    const record = this.#sessions.get(pageUuid);
    const current = record?.snapshot.document;
    if (!record || !current || current.inFlight?.id !== attempt.id) return;
    if (current.draft === attempt.draft || current.draft === persisted.buffer) {
      this.#setDocument(pageUuid, record, null);
      return;
    }
    this.#setDocument(pageUuid, record, {
      ...current,
      baseBuffer: persisted.buffer,
      baseSourceMap: persisted.sourceMap,
      baseRevision: persisted.revision,
      conflict: null,
      inFlight: null,
    });
  }

  failDocumentSave(pageUuid: string, attempt: DocumentSaveAttempt): void {
    const record = this.#sessions.get(pageUuid);
    const current = record?.snapshot.document;
    if (!record || !current || current.inFlight?.id !== attempt.id) return;
    this.#setDocument(pageUuid, record, { ...current, inFlight: null });
  }

  useRemoteDocument(pageUuid: string): void {
    const record = this.#sessions.get(pageUuid);
    if (!record?.snapshot.document?.conflict) return;
    this.#setDocument(pageUuid, record, null);
  }

  keepLocalDocument(pageUuid: string): void {
    const record = this.#sessions.get(pageUuid);
    const current = record?.snapshot.document;
    if (!record || !current?.conflict) return;
    const remote = current.conflict.remote;
    this.#setDocument(pageUuid, record, {
      ...current,
      baseBuffer: remote.buffer,
      baseSourceMap: remote.sourceMap,
      baseRevision: remote.revision,
      conflict: null,
      inFlight: null,
    });
  }

  discardBlock(pageUuid: string, blockUuid: string): void {
    const record = this.#sessions.get(pageUuid);
    if (!record || !record.snapshot.blocks[blockUuid]) return;
    this.#setOverlay(pageUuid, record, { kind: "block", blockUuid }, null);
  }

  discardPage(pageUuid: string): void {
    if (!this.#sessions.delete(pageUuid)) return;
    for (const listener of this.#listeners.get(pageUuid) ?? []) listener();
  }

  #edit(
    pageUuid: string,
    target: DraftTarget,
    draft: string,
    persisted: PersistedTextSnapshot,
  ): void {
    const record = this.#record(pageUuid);
    const current = getOverlay(record.snapshot, target);
    if (!current) {
      if (draft === persisted.text) return;
      this.#setOverlay(pageUuid, record, target, {
        draft,
        baseText: persisted.text,
        baseRevision: persisted.revision,
        conflict: null,
        inFlight: null,
      });
      return;
    }
    if (current.draft === draft) return;
    if (draft === current.baseText && current.conflict === null && current.inFlight === null) {
      this.#setOverlay(pageUuid, record, target, null);
      return;
    }
    this.#setOverlay(pageUuid, record, target, { ...current, draft });
  }

  #acceptPersisted(pageUuid: string, target: DraftTarget, persisted: PersistedTextSnapshot): void {
    const record = this.#sessions.get(pageUuid);
    if (!record) return;
    const current = getOverlay(record.snapshot, target);
    if (!current) return;

    if (persisted.text === current.draft) {
      this.#setOverlay(pageUuid, record, target, null);
      return;
    }
    if (current.inFlight?.draft === persisted.text) {
      this.#setOverlay(pageUuid, record, target, {
        ...current,
        baseText: persisted.text,
        baseRevision: persisted.revision,
        conflict: null,
        inFlight: null,
      });
      return;
    }
    if (persisted.text === current.baseText) {
      if (persisted.revision !== current.baseRevision) {
        this.#setOverlay(pageUuid, record, target, {
          ...current,
          baseRevision: persisted.revision,
        });
      }
      return;
    }
    if (
      current.conflict?.remoteText === persisted.text &&
      current.conflict.remoteRevision === persisted.revision
    ) {
      return;
    }
    this.#setOverlay(pageUuid, record, target, {
      ...current,
      conflict: {
        remoteText: persisted.text,
        remoteRevision: persisted.revision,
      },
      inFlight: null,
    });
  }

  #beginSave(pageUuid: string, target: DraftTarget): DraftSaveAttempt | null {
    const record = this.#sessions.get(pageUuid);
    if (!record) return null;
    const current = getOverlay(record.snapshot, target);
    if (!current || current.conflict || current.inFlight) return null;
    const attempt = Object.freeze({
      id: this.#nextAttemptId++,
      draft: current.draft,
      expectedRevision: current.baseRevision,
    });
    this.#setOverlay(pageUuid, record, target, { ...current, inFlight: attempt });
    return attempt;
  }

  #acknowledgeSave(
    pageUuid: string,
    target: DraftTarget,
    attempt: DraftSaveAttempt,
    persisted: PersistedTextSnapshot,
  ): void {
    const record = this.#sessions.get(pageUuid);
    if (!record) return;
    const current = getOverlay(record.snapshot, target);
    if (!current || current.inFlight?.id !== attempt.id) return;
    if (current.draft === attempt.draft) {
      this.#setOverlay(pageUuid, record, target, null);
      return;
    }
    if (current.draft === persisted.text) {
      this.#setOverlay(pageUuid, record, target, null);
      return;
    }
    this.#setOverlay(pageUuid, record, target, {
      ...current,
      baseText: persisted.text,
      baseRevision: persisted.revision,
      conflict: null,
      inFlight: null,
    });
  }

  #failSave(pageUuid: string, target: DraftTarget, attempt: DraftSaveAttempt): void {
    const record = this.#sessions.get(pageUuid);
    if (!record) return;
    const current = getOverlay(record.snapshot, target);
    if (!current || current.inFlight?.id !== attempt.id) return;
    this.#setOverlay(pageUuid, record, target, { ...current, inFlight: null });
  }

  #useRemote(pageUuid: string, target: DraftTarget): void {
    const record = this.#sessions.get(pageUuid);
    if (!record || !getOverlay(record.snapshot, target)?.conflict) return;
    this.#setOverlay(pageUuid, record, target, null);
  }

  #keepLocal(pageUuid: string, target: DraftTarget): void {
    const record = this.#sessions.get(pageUuid);
    if (!record) return;
    const current = getOverlay(record.snapshot, target);
    if (!current?.conflict) return;
    this.#setOverlay(pageUuid, record, target, {
      ...current,
      baseText: current.conflict.remoteText,
      baseRevision: current.conflict.remoteRevision,
      conflict: null,
      inFlight: null,
    });
  }

  #record(pageUuid: string): SessionRecord {
    let record = this.#sessions.get(pageUuid);
    if (!record) {
      record = { snapshot: EMPTY_SESSION, writerTokens: new Set() };
      this.#sessions.set(pageUuid, record);
    }
    return record;
  }

  #setOverlay(
    pageUuid: string,
    record: SessionRecord,
    target: DraftTarget,
    overlay: DraftOverlay | null,
  ): void {
    const snapshot = record.snapshot;
    const next =
      target.kind === "title"
        ? { ...snapshot, title: overlay }
        : {
            ...snapshot,
            blocks: updateBlockOverlay(snapshot.blocks, target.blockUuid, overlay),
          };
    this.#publish(pageUuid, record, next);
  }

  #setDocument(
    pageUuid: string,
    record: SessionRecord,
    document: DocumentDraftOverlay | null,
  ): void {
    this.#publish(pageUuid, record, { ...record.snapshot, document });
  }

  #publish(pageUuid: string, record: SessionRecord, snapshot: PageSessionSnapshot): void {
    if (record.snapshot === snapshot) return;
    record.snapshot = snapshot;
    if (
      snapshot.writerPaneId === null &&
      snapshot.title === null &&
      snapshot.document === null &&
      Object.keys(snapshot.blocks).length === 0
    ) {
      this.#sessions.delete(pageUuid);
    }
    for (const listener of this.#listeners.get(pageUuid) ?? []) listener();
  }
}

function getOverlay(snapshot: PageSessionSnapshot, target: DraftTarget): DraftOverlay | null {
  return target.kind === "title" ? snapshot.title : (snapshot.blocks[target.blockUuid] ?? null);
}

function updateBlockOverlay(
  blocks: Readonly<Record<string, DraftOverlay>>,
  blockUuid: string,
  overlay: DraftOverlay | null,
): Readonly<Record<string, DraftOverlay>> {
  if (overlay) return { ...blocks, [blockUuid]: overlay };
  if (!(blockUuid in blocks)) return blocks;
  const next = { ...blocks };
  delete next[blockUuid];
  return Object.keys(next).length === 0 ? EMPTY_BLOCKS : next;
}

const PageSessionContext = createContext<PageSessionRegistry | null>(null);

export function PageSessionProvider({
  children,
  registry: suppliedRegistry,
}: {
  children: ReactNode;
  registry?: PageSessionRegistry;
}) {
  const [registry] = useState(() => suppliedRegistry ?? new PageSessionRegistry());
  return <PageSessionContext.Provider value={registry}>{children}</PageSessionContext.Provider>;
}

export function usePageSessionRegistry(): PageSessionRegistry {
  const registry = useContext(PageSessionContext);
  if (!registry) throw new Error("usePageSessionRegistry must be used inside PageSessionProvider");
  return registry;
}

export function useTitleDraftOverlay(pageUuid: string): DraftOverlay | null {
  const registry = usePageSessionRegistry();
  return useSyncExternalStore(
    (listener) => registry.subscribe(pageUuid, listener),
    () => registry.getSnapshot(pageUuid).title,
    () => registry.getSnapshot(pageUuid).title,
  );
}

export function useBlockDraftOverlay(pageUuid: string, blockUuid: string): DraftOverlay | null {
  const registry = usePageSessionRegistry();
  return useSyncExternalStore(
    (listener) => registry.subscribe(pageUuid, listener),
    () => registry.getSnapshot(pageUuid).blocks[blockUuid] ?? null,
    () => registry.getSnapshot(pageUuid).blocks[blockUuid] ?? null,
  );
}

export function useDocumentDraftOverlay(pageUuid: string): DocumentDraftOverlay | null {
  const registry = usePageSessionRegistry();
  return useSyncExternalStore(
    (listener) => registry.subscribe(pageUuid, listener),
    () => registry.getSnapshot(pageUuid).document,
    () => registry.getSnapshot(pageUuid).document,
  );
}

export function usePageWriterPaneId(pageUuid: string): PaneId | null {
  const registry = usePageSessionRegistry();
  return useSyncExternalStore(
    (listener) => registry.subscribe(pageUuid, listener),
    () => registry.getSnapshot(pageUuid).writerPaneId,
    () => registry.getSnapshot(pageUuid).writerPaneId,
  );
}

export function usePageWriterLease(pageUuid: string, paneId: PaneId, enabled: boolean): boolean {
  const registry = usePageSessionRegistry();
  const writerPaneId = usePageWriterPaneId(pageUuid);
  const token = useRef<WriterLeaseToken | null>(null);
  if (token.current === null) token.current = registry.createWriterLeaseToken();

  useEffect(() => {
    if (!enabled || token.current === null) return;
    const leaseToken = token.current;
    registry.acquireWriter(pageUuid, paneId, leaseToken);
    return () => registry.releaseWriter(pageUuid, paneId, leaseToken);
  }, [enabled, pageUuid, paneId, registry]);

  return enabled && writerPaneId === paneId;
}

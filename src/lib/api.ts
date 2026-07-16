import { commands } from "@/lib/bindings";
import type {
  BackgroundFailure,
  BackgroundStatus,
  BlockContent,
  ChatEvent,
  Edge,
  EmbeddingProvider,
  GraphSnapshot,
  Node,
  SearchHit,
  RerankProvider,
  SecretKey,
  SettingsSnapshot,
  SettingsUpdate,
  StartupStatus,
} from "@/lib/bindings";

export type {
  BackgroundFailure,
  BackgroundStatus,
  BlockContent,
  ChatEvent,
  Edge,
  EmbeddingProvider,
  GraphSnapshot,
  Node,
  SearchHit,
  RerankProvider,
  SecretKey,
  SettingsSnapshot,
  SettingsUpdate,
  StartupStatus,
};

export type Mode = "fts" | "vec" | "hybrid" | "agentic";

export type ToolCallView = {
  id: string;
  name: string;
  args: unknown;
  result?: string;
};

/** UI-only conversation state; the transport receives only role and text. */
export type ChatTurn = {
  role: "user" | "assistant";
  text: string;
  tools?: ToolCallView[];
  usage?: { inputTokens: number; outputTokens: number; totalTokens: number };
};

export function createNode(args: {
  kind: Node["kind"];
  title: string | null;
  content: string;
  contentJson: string | null;
}) {
  return commands.createNode(args.kind, args.title, args.content, args.contentJson);
}

export function updateNode(args: {
  id: number;
  title: string | null;
  content: string;
  contentJson: string | null;
}) {
  return commands.updateNode(args.id, args.title, args.content, args.contentJson);
}

export const setBlockContent = (uuid: string, block: BlockContent) =>
  commands.setBlockContent(uuid, block);
export const renamePage = commands.renamePage;
export const createNote = commands.createNote;
export const splitBlock = (id: number, parts: BlockContent[]) => commands.splitBlock(id, parts);
export const isReady = commands.isReady;
export const getStartupStatus = commands.startupStatus;
export const loadSettings = commands.loadSettings;
export const saveSettings = commands.saveSettings;

export function testCompletionProvider(request: {
  protocol: "openai" | "anthropic";
  baseUrl: string;
  model: string;
  apiKey?: string;
  keyScope?: "chat" | "extraction";
}) {
  return commands.testCompletionProvider({
    ...request,
    apiKey: request.apiKey ?? null,
    keyScope: request.keyScope ?? null,
  });
}

export const restartApp = commands.restartApp;
export const getBackgroundStatus = commands.backgroundStatus;
export const setBackgroundPaused = commands.setBackgroundPaused;
export const retryBackgroundJobs = commands.retryBackgroundJobs;
export const clearBackgroundJobs = commands.clearBackgroundJobs;
export const listEntities = (limit = 30) => commands.listEntities(limit);
export const listPages = (limit = 200) => commands.listPages(limit);
export const createPage = commands.createPage;
export const deletePage = commands.deletePage;
export const findBacklinks = (id: number, kind: string | null = null) =>
  commands.findBacklinks(id, kind);
export const getGraphSnapshot = (focusId: number | null = null) => commands.graphSnapshot(focusId);
export const exportData = commands.exportData;
export const importData = commands.importData;
export const createBackup = commands.createBackup;
export const chooseSyncDirectory = commands.chooseSyncDirectory;
export const syncPush = commands.syncPush;
export const syncPull = commands.syncPull;
export const attachFile = commands.attachFile;
export const listAttachments = commands.listAttachments;
export const openAttachment = commands.openAttachment;
export const deleteAttachment = commands.deleteAttachment;
export const getHistoryStatus = commands.historyStatus;
export const undo = commands.undo;
export const redo = commands.redo;
export const getNode = commands.getNode;
export const getContainingPage = commands.getContainingPage;
export const listBlockChildren = commands.listBlockChildren;

export function createBlock(args: {
  parentId: number | null;
  position: number | null;
  content: string;
  contentJson: string | null;
}) {
  return commands.createBlock(args.parentId, args.position, args.content, args.contentJson);
}

export const indentBlock = commands.indentBlock;
export const outdentBlock = commands.outdentBlock;
export const moveBlockUp = commands.moveBlockUp;
export const moveBlockDown = commands.moveBlockDown;
export const deleteBlock = commands.deleteBlock;

export function replaceBlockRefs(args: {
  blockId: number;
  wikilinkTitles: string[];
  blockUuids: string[];
}) {
  return commands.replaceBlockRefs(args.blockId, args.wikilinkTitles, args.blockUuids);
}

export const getOrCreatePageByTitle = commands.getOrCreatePageByTitle;
export const getPageByTitle = commands.getPageByTitle;
export const getNodeByUuid = commands.getNodeByUuid;
export const searchPagesByTitle = (query: string, limit = 8) =>
  commands.searchPagesByTitle(query, limit);
export const searchBlocksFts = (query: string, limit = 8) => commands.searchBlocksFts(query, limit);

export function search(mode: Mode, query: string, limit = 20) {
  switch (mode) {
    case "fts":
      return commands.searchFts(query, limit);
    case "vec":
      return commands.searchVec(query, limit);
    case "hybrid":
      return commands.searchHybrid(query, limit);
    case "agentic":
      return commands.searchAgentic(query, limit);
  }
}

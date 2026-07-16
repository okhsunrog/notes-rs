import { commands } from "@/lib/bindings";
import type {
  BackgroundFailure,
  BackgroundStatus,
  BlockContent,
  ChatEvent,
  CommandError,
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

type CommandOutcome<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

export class CommandFailure extends Error {
  readonly code: CommandError["code"];

  constructor(error: CommandError) {
    super(error.message);
    this.name = "CommandFailure";
    this.code = error.code;
  }
}

export async function unwrapCommand<T>(outcome: Promise<CommandOutcome<T>>): Promise<T> {
  const result = await outcome;
  if (result.status === "error") throw new CommandFailure(result.error);
  return result.data;
}

function checkedCommand<Args extends unknown[], Value>(
  command: (...args: Args) => Promise<CommandOutcome<Value>>,
) {
  return (...args: Args) => unwrapCommand(command(...args));
}

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
  return unwrapCommand(commands.createNode(args.kind, args.title, args.content, args.contentJson));
}

export function updateNode(args: {
  id: number;
  title: string | null;
  content: string;
  contentJson: string | null;
}) {
  return unwrapCommand(commands.updateNode(args.id, args.title, args.content, args.contentJson));
}

export const setBlockContent = (uuid: string, block: BlockContent) =>
  unwrapCommand(commands.setBlockContent(uuid, block));
export const renamePage = checkedCommand(commands.renamePage);
export const createNote = checkedCommand(commands.createNote);
export const splitBlock = (id: number, parts: BlockContent[]) =>
  unwrapCommand(commands.splitBlock(id, parts));
export const isReady = commands.isReady;
export const getStartupStatus = commands.startupStatus;
export const loadSettings = checkedCommand(commands.loadSettings);
export const saveSettings = checkedCommand(commands.saveSettings);

export function testCompletionProvider(request: {
  protocol: "openai" | "anthropic";
  baseUrl: string;
  model: string;
  apiKey?: string;
  keyScope?: "chat" | "extraction";
}) {
  return unwrapCommand(
    commands.testCompletionProvider({
      ...request,
      apiKey: request.apiKey ?? null,
      keyScope: request.keyScope ?? null,
    }),
  );
}

export const restartApp = commands.restartApp;
export const chatStream = checkedCommand(commands.chatStream);
export const cancelChat = commands.cancelChat;
export const getBackgroundStatus = checkedCommand(commands.backgroundStatus);
export const setBackgroundPaused = commands.setBackgroundPaused;
export const retryBackgroundJobs = checkedCommand(commands.retryBackgroundJobs);
export const clearBackgroundJobs = checkedCommand(commands.clearBackgroundJobs);
export const listEntities = (limit = 30) => unwrapCommand(commands.listEntities(limit));
export const listPages = (limit = 200) => unwrapCommand(commands.listPages(limit));
export const createPage = checkedCommand(commands.createPage);
export const deletePage = checkedCommand(commands.deletePage);
export const findBacklinks = (id: number, kind: string | null = null) =>
  unwrapCommand(commands.findBacklinks(id, kind));
export const getGraphSnapshot = (focusId: number | null = null) =>
  unwrapCommand(commands.graphSnapshot(focusId));
export const exportData = checkedCommand(commands.exportData);
export const importData = checkedCommand(commands.importData);
export const createBackup = checkedCommand(commands.createBackup);
export const chooseSyncDirectory = checkedCommand(commands.chooseSyncDirectory);
export const syncPush = checkedCommand(commands.syncPush);
export const syncPull = checkedCommand(commands.syncPull);
export const attachFile = checkedCommand(commands.attachFile);
export const listAttachments = checkedCommand(commands.listAttachments);
export const openAttachment = checkedCommand(commands.openAttachment);
export const deleteAttachment = checkedCommand(commands.deleteAttachment);
export const getHistoryStatus = checkedCommand(commands.historyStatus);
export const undo = checkedCommand(commands.undo);
export const redo = checkedCommand(commands.redo);
export const getNode = checkedCommand(commands.getNode);
export const getContainingPage = checkedCommand(commands.getContainingPage);
export const listBlockChildren = checkedCommand(commands.listBlockChildren);

export function createBlock(args: {
  parentId: number | null;
  position: number | null;
  content: string;
  contentJson: string | null;
}) {
  return unwrapCommand(
    commands.createBlock(args.parentId, args.position, args.content, args.contentJson),
  );
}

export const indentBlock = checkedCommand(commands.indentBlock);
export const outdentBlock = checkedCommand(commands.outdentBlock);
export const moveBlockUp = checkedCommand(commands.moveBlockUp);
export const moveBlockDown = checkedCommand(commands.moveBlockDown);
export const deleteBlock = checkedCommand(commands.deleteBlock);

export function replaceBlockRefs(args: {
  blockId: number;
  wikilinkTitles: string[];
  blockUuids: string[];
}) {
  return unwrapCommand(
    commands.replaceBlockRefs(args.blockId, args.wikilinkTitles, args.blockUuids),
  );
}

export const getOrCreatePageByTitle = checkedCommand(commands.getOrCreatePageByTitle);
export const getPageByTitle = checkedCommand(commands.getPageByTitle);
export const getNodeByUuid = checkedCommand(commands.getNodeByUuid);
export const searchPagesByTitle = (query: string, limit = 8) =>
  unwrapCommand(commands.searchPagesByTitle(query, limit));
export const searchBlocksFts = (query: string, limit = 8) =>
  unwrapCommand(commands.searchBlocksFts(query, limit));

export function search(mode: Mode, query: string, limit = 20) {
  switch (mode) {
    case "fts":
      return unwrapCommand(commands.searchFts(query, limit));
    case "vec":
      return unwrapCommand(commands.searchVec(query, limit));
    case "hybrid":
      return unwrapCommand(commands.searchHybrid(query, limit));
    case "agentic":
      return unwrapCommand(commands.searchAgentic(query, limit));
  }
}

import { commands } from "@/lib/bindings";
import type {
  AiIndexStatus,
  AiProviderProbeResult,
  AiProviderSettings,
  AiProviderSettingsUpdate,
  AiRuntimeSettings,
  Attachment,
  AttachmentImageDescriptor,
  AttachmentImageMime,
  AttachmentOwner,
  Block,
  BlockContent,
  BlockStyle,
  ChatEvent,
  CommandError,
  CompletionProtocol,
  Content,
  ContentRevision,
  CreatedNote,
  DocumentRevision,
  DocumentUnitDraft,
  GraphEdge,
  GraphItem,
  GraphSnapshot,
  HistoryStatus,
  ImportDiagnostic,
  JournalDate,
  JournalListLimit,
  LogseqImportAvailability,
  LogseqImportBlocker,
  LogseqImportCommitResult,
  LogseqImportDiagnosticPage,
  LogseqImportPreview,
  LogseqImportProgress,
  LogseqImportReportSummary,
  LogseqImportStage,
  LogseqDrawingConversionState,
  ObjectKind,
  Page,
  PageDocumentSnapshot,
  PageKind,
  PageLayout,
  PageListFilter,
  SearchHit,
  SecretKey,
  SearchMode,
  SettingsSnapshot,
  SettingsUpdate,
  StartupStatus,
  SyncStatus,
  TaskState,
  WindowDecorationMode,
} from "@/lib/bindings";

export type {
  AiIndexStatus,
  AiProviderProbeResult,
  AiProviderSettings,
  AiProviderSettingsUpdate,
  AiRuntimeSettings,
  Attachment,
  AttachmentImageDescriptor,
  AttachmentImageMime,
  AttachmentOwner,
  Block,
  BlockContent,
  BlockStyle,
  ChatEvent,
  CompletionProtocol,
  Content,
  ContentRevision,
  CreatedNote,
  DocumentRevision,
  DocumentUnitDraft,
  GraphEdge,
  GraphItem,
  GraphSnapshot,
  HistoryStatus,
  ImportDiagnostic,
  JournalDate,
  JournalListLimit,
  LogseqImportAvailability,
  LogseqImportBlocker,
  LogseqImportCommitResult,
  LogseqImportDiagnosticPage,
  LogseqImportPreview,
  LogseqImportProgress,
  LogseqImportReportSummary,
  LogseqImportStage,
  LogseqDrawingConversionState,
  ObjectKind,
  Page,
  PageDocumentSnapshot,
  PageKind,
  PageLayout,
  PageListFilter,
  SearchHit,
  SecretKey,
  SearchMode,
  SettingsSnapshot,
  SettingsUpdate,
  StartupStatus,
  SyncStatus,
  TaskState,
  WindowDecorationMode,
};

type CommandOutcome<T> = { status: "ok"; data: T } | { status: "error"; error: unknown };

const COMMAND_ERROR_CODES = new Set<CommandError["code"]>([
  "invalid_input",
  "not_found",
  "conflict",
  "unavailable",
  "internal",
]);

function isCommandError(error: unknown): error is CommandError {
  if (typeof error !== "object" || error === null) return false;
  const candidate = error as { code?: unknown; message?: unknown };
  return (
    typeof candidate.code === "string" &&
    COMMAND_ERROR_CODES.has(candidate.code as CommandError["code"]) &&
    typeof candidate.message === "string"
  );
}

function unknownErrorMessage(error: unknown) {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  try {
    return JSON.stringify(error) ?? String(error);
  } catch {
    return String(error);
  }
}

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
  if (result.status === "error") {
    if (isCommandError(result.error)) throw new CommandFailure(result.error);
    if (result.error instanceof Error) throw result.error;
    throw new Error(unknownErrorMessage(result.error));
  }
  return result.data;
}

function checkedCommand<Args extends unknown[], Value>(
  command: (...args: Args) => Promise<CommandOutcome<Value>>,
) {
  return (...args: Args) => unwrapCommand(command(...args));
}

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

export function contentUuid(content: Content) {
  return content.record.uuid;
}

export function contentPageUuid(content: Content) {
  return content.kind === "page" ? content.record.uuid : content.record.pageUuid;
}

export function contentText(content: Content) {
  if (content.kind === "block") return content.record.markdown;
  return content.record.kind.kind === "journal"
    ? content.record.kind.date
    : (content.record.title ?? "");
}

export const setBlockContent = checkedCommand(commands.setBlockContent);
export const setBlockStyle = checkedCommand(commands.setBlockStyle);
export const setTaskState = checkedCommand(commands.setTaskState);
export const renamePage = checkedCommand(commands.renamePage);
export const setPageLayout = checkedCommand(commands.setPageLayout);
export const createNote = checkedCommand(commands.createNote);
export const ensureJournal = checkedCommand(commands.ensureJournal);
export const getJournal = checkedCommand(commands.getJournal);
export const appendToJournal = checkedCommand(commands.appendToJournal);
export const splitBlock = checkedCommand(commands.splitBlock);
export const isReady = commands.isReady;
export const getStartupStatus = commands.startupStatus;
export const getMobileSystemInfo = checkedCommand(commands.mobileSystemInfo);
export const setSystemBarsStyle = checkedCommand(commands.setSystemBarsStyle);
export const loadSettings = checkedCommand(commands.loadSettings);
export const saveSettings = checkedCommand(commands.saveSettings);
export const resetSettings = checkedCommand(commands.resetSettings);

export const restartApp = commands.restartApp;
export const chatStream = checkedCommand(commands.chatStream);
export const cancelChat = commands.cancelChat;
export const getSyncStatus = commands.syncStatus;
export const getServerAiStatus = checkedCommand(commands.serverAiStatus);
export const saveServerAiSettings = checkedCommand(commands.saveServerAiSettings);
export const saveServerAiProvider = checkedCommand(commands.saveServerAiProvider);
export const probeServerAiProvider = checkedCommand(commands.probeServerAiProvider);
export const reindexServerAi = checkedCommand(commands.reindexServerAi);
export function listPages(options: { filter?: PageListFilter; limit?: number } = {}) {
  return unwrapCommand(commands.listPages(options.filter ?? null, options.limit ?? null));
}

export function listJournals(
  options: {
    beforeDate?: JournalDate;
    limit?: JournalListLimit;
  } = {},
) {
  return unwrapCommand(commands.listJournals(options.beforeDate ?? null, options.limit ?? null));
}
export const createPage = checkedCommand(commands.createPage);
export const deletePage = checkedCommand(commands.deletePage);
export const findBacklinks = checkedCommand(commands.findBacklinks);
export const getGraphSnapshot = checkedCommand(commands.graphSnapshot);
export const exportData = checkedCommand(commands.exportData);
export const importData = checkedCommand(commands.importData);
export const getLogseqImportCapability = commands.logseqImportCapability;
export const prepareLogseqImport = checkedCommand(commands.prepareLogseqImport);
export const getLogseqImportDiagnostics = checkedCommand(commands.logseqImportDiagnostics);
export const commitLogseqImport = checkedCommand(commands.commitLogseqImport);
export const discardLogseqImport = commands.discardLogseqImport;
export const createBackup = checkedCommand(commands.createBackup);
export const attachFile = checkedCommand(commands.attachFile);
export const listAttachments = checkedCommand(commands.listAttachments);
export const resolveAttachmentImages = checkedCommand(commands.resolveAttachmentImages);
export const openAttachment = checkedCommand(commands.openAttachment);
export const deleteAttachment = checkedCommand(commands.deleteAttachment);
export const getHistoryStatus = checkedCommand(commands.historyStatus);
export const undo = checkedCommand(commands.undo);
export const redo = checkedCommand(commands.redo);
export const getPage = checkedCommand(commands.getPage);
export const getPageDocument = checkedCommand(commands.getPageDocument);
export const replacePageDocument = checkedCommand(commands.replacePageDocument);
export const getBlock = checkedCommand(commands.getBlock);
export const getContainingPage = checkedCommand(commands.getContainingPage);
export const listBlockChildren = checkedCommand(commands.listBlockChildren);

export function createBlock(args: {
  pageUuid: string;
  parentUuid: string | null;
  afterUuid: string | null;
  style?: BlockStyle;
  markdown?: string;
}) {
  return unwrapCommand(
    commands.createBlock(
      args.pageUuid,
      args.parentUuid,
      args.afterUuid,
      args.style ?? { kind: "paragraph" },
      args.markdown ?? "",
    ),
  );
}

export const indentBlock = checkedCommand(commands.indentBlock);
export const outdentBlock = checkedCommand(commands.outdentBlock);
export const moveBlockUp = checkedCommand(commands.moveBlockUp);
export const moveBlockDown = checkedCommand(commands.moveBlockDown);
export const deleteBlock = checkedCommand(commands.deleteBlock);

export const getOrCreatePageByTitle = checkedCommand(commands.getOrCreatePageByTitle);
export const getPageByTitle = checkedCommand(commands.getPageByTitle);
export const searchPagesByTitle = (query: string, limit = 8) =>
  unwrapCommand(commands.searchPagesByTitle(query, limit));
export const searchBlocksFts = (query: string, limit = 8) =>
  unwrapCommand(commands.searchBlocksFts(query, limit));

export function search(mode: SearchMode, query: string, limit = 20) {
  return unwrapCommand(commands.searchNotes(mode, query, limit));
}

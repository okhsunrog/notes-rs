import { invoke } from "@tauri-apps/api/core";

export type Node = {
  id: number;
  uuid: string;
  kind: string;
  title: string | null;
  content: string;
  content_json: string | null;
  parent_id: number | null;
  position: number | null;
  created_at: number;
  updated_at: number;
};

export type SearchHit = { node: Node; score: number };
export type Edge = { src: number; dst: number; kind: string; weight: number; created_at: number };
export type GraphSnapshot = { nodes: Node[]; edges: Edge[] };
export type BackgroundStatus = {
  paused: boolean;
  embeddingsPending: number;
  embeddingsFailed: number;
  extractionsPending: number;
  extractionsFailed: number;
  failures: BackgroundFailure[];
};
export type BackgroundFailure = {
  queue: "embedding" | "extraction";
  nodeId: number;
  nodeTitle: string | null;
  retryCount: number;
  lastAttempt: number | null;
  failureKind: string;
  lastError: string;
  terminal: boolean;
};
export type Mode = "fts" | "vec" | "hybrid" | "agentic";
export type StartupStatus =
  | { state: "starting"; message: string }
  | { state: "ready" }
  | { state: "error"; message: string };

export type SettingsSnapshot = {
  localOnly: boolean;
  entityExtractionEnabled: boolean;
  queryRewritingEnabled: boolean;
  chatModel: string;
  chatProtocol: "openai" | "anthropic";
  chatBaseUrl: string;
  extractionModel: string;
  extractionProtocol: "inherit" | "openai" | "anthropic";
  extractionBaseUrl: string;
  embeddingProvider: string;
  embeddingModel: string;
  embeddingNdims: string;
  rerankProvider: string;
  rerankModel: string;
  openrouterBaseUrl: string;
  openaiBaseUrl: string;
  windowDecorationMode: "native" | "borderless" | "kde";
  syncDirectory: string;
  kdeDecorationsAvailable: boolean;
  configuredKeys: string[];
  localModelsAvailable: boolean;
  configPath: string;
};

export type SettingsUpdate = Omit<
  SettingsSnapshot,
  "configuredKeys" | "localModelsAvailable" | "kdeDecorationsAvailable" | "configPath"
> & {
  apiKeys: Record<string, string>;
  clearKeys: string[];
};

export type ChatEvent =
  | { kind: "text_delta"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "tool_start"; id: string; name: string; args: unknown }
  | { kind: "tool_end"; id: string; result: string }
  | { kind: "usage"; inputTokens: number; outputTokens: number; totalTokens: number }
  | { kind: "cancelled" }
  | { kind: "done"; text: string }
  | { kind: "error"; message: string };

export type ToolCallView = {
  id: string;
  name: string;
  args: unknown;
  result?: string;
};

export type ChatTurn = {
  role: "user" | "assistant";
  text: string;
  tools?: ToolCallView[];
  usage?: { inputTokens: number; outputTokens: number; totalTokens: number };
};

const SEARCH_CMD: Record<Mode, string> = {
  fts: "search_fts",
  vec: "search_vec",
  hybrid: "search_hybrid",
  agentic: "search_agentic",
};

export function createNode(args: {
  kind: string;
  title: string | null;
  content: string;
  contentJson: string | null;
}) {
  return invoke<Node>("create_node", args);
}

export function updateNode(args: {
  id: number;
  title: string | null;
  content: string;
  contentJson: string | null;
}) {
  return invoke<void>("update_node", args);
}

export type BlockContent = {
  content: string;
  wikilinkTitles: string[];
  blockUuids: string[];
};

export function updateBlockWithRefs(id: number, block: BlockContent) {
  return invoke<[Node, number]>("update_block_with_refs", { id, block });
}

export function splitBlock(id: number, parts: BlockContent[]) {
  return invoke<Node[]>("split_block", { id, parts });
}

export function isReady() {
  return invoke<boolean>("is_ready");
}

export function getStartupStatus() {
  return invoke<StartupStatus>("startup_status");
}

export function loadSettings() {
  return invoke<SettingsSnapshot>("load_settings");
}

export function saveSettings(update: SettingsUpdate) {
  return invoke<SettingsSnapshot>("save_settings", { update });
}

export function testCompletionProvider(request: {
  protocol: "openai" | "anthropic";
  baseUrl: string;
  model: string;
  apiKey?: string;
}) {
  return invoke<{ latencyMs: number; response: string }>("test_completion_provider", { request });
}

export function restartApp() {
  return invoke<void>("restart_app");
}

export function getBackgroundStatus() {
  return invoke<BackgroundStatus>("background_status");
}

export function setBackgroundPaused(paused: boolean) {
  return invoke<void>("set_background_paused", { paused });
}

export function retryBackgroundJobs() {
  return invoke<void>("retry_background_jobs");
}

export function clearBackgroundJobs() {
  return invoke<void>("clear_background_jobs");
}

export function listEntities(limit = 30) {
  return invoke<Node[]>("list_entities", { limit });
}

export function listPages(limit = 200) {
  return invoke<Node[]>("list_pages", { limit });
}

export function createPage(title: string) {
  return invoke<Node>("create_page", { title });
}

export function deletePage(id: number) {
  return invoke<boolean>("delete_page", { id });
}

export function findBacklinks(id: number, kind: string | null = null) {
  return invoke<Node[]>("find_backlinks", { id, kind });
}

export function getGraphSnapshot(focusId: number | null = null) {
  return invoke<GraphSnapshot>("graph_snapshot", { focusId });
}

export function exportData() {
  return invoke<string | null>("export_data");
}

export function importData() {
  return invoke<string | null>("import_data");
}

export function createBackup() {
  return invoke<string>("create_backup");
}

export function chooseSyncDirectory() {
  return invoke<string | null>("choose_sync_directory");
}

export function syncPush() {
  return invoke<string>("sync_push");
}

export function syncPull() {
  return invoke<string>("sync_pull");
}

export function attachFile(parentId: number) {
  return invoke<Node | null>("attach_file", { parentId });
}

export function listAttachments(parentId: number) {
  return invoke<Node[]>("list_attachments", { parentId });
}

export function openAttachment(id: number) {
  return invoke<void>("open_attachment", { id });
}

export function deleteAttachment(id: number) {
  return invoke<boolean>("delete_attachment", { id });
}

export function getHistoryStatus() {
  return invoke<[number, number]>("history_status");
}

export function undo() {
  return invoke<boolean>("undo");
}

export function redo() {
  return invoke<boolean>("redo");
}

export function getNode(id: number) {
  return invoke<Node | null>("get_node", { id });
}

export function getContainingPage(id: number) {
  return invoke<Node | null>("get_containing_page", { id });
}

export function listBlockChildren(parentId: number) {
  return invoke<Node[]>("list_block_children", { parentId });
}

export function createBlock(args: {
  parentId: number | null;
  position: number | null;
  content: string;
  contentJson: string | null;
}) {
  return invoke<Node>("create_block", args);
}

export function moveBlock(args: {
  id: number;
  newParentId: number | null;
  newPosition: number | null;
}) {
  return invoke<Node>("move_block", args);
}

export function reorderBlock(id: number, direction: "up" | "down") {
  return invoke<Node>("reorder_block", { id, direction });
}

export function deleteBlock(id: number) {
  return invoke<boolean>("delete_block", { id });
}

/** Replace all outgoing ref edges from `blockId`. Returns count of broken
 * `((uuid))` refs that pointed at non-existent blocks. */
export function replaceBlockRefs(args: {
  blockId: number;
  wikilinkTitles: string[];
  blockUuids: string[];
}) {
  return invoke<number>("replace_block_refs", args);
}

export function getOrCreatePageByTitle(title: string) {
  return invoke<Node>("get_or_create_page_by_title", { title });
}

export function getPageByTitle(title: string) {
  return invoke<Node | null>("get_page_by_title", { title });
}

export function getNodeByUuid(uuid: string) {
  return invoke<Node | null>("get_node_by_uuid", { uuid });
}

export function searchPagesByTitle(query: string, limit = 8) {
  return invoke<Node[]>("search_pages_by_title", { query, limit });
}

export function searchBlocksFts(query: string, limit = 8) {
  return invoke<Node[]>("search_blocks_fts", { query, limit });
}

export function search(mode: Mode, query: string, limit = 20) {
  return invoke<SearchHit[]>(SEARCH_CMD[mode], { query, limit });
}

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
export type Mode = "fts" | "vec" | "hybrid" | "agentic";

export type ChatEvent =
  | { kind: "text_delta"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "tool_start"; id: string; name: string; args: unknown }
  | { kind: "tool_end"; id: string; result: string }
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

export function isReady() {
  return invoke<boolean>("is_ready");
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

export function getNode(id: number) {
  return invoke<Node | null>("get_node", { id });
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

export function deleteBlock(id: number) {
  return invoke<boolean>("delete_block", { id });
}

export function getOrCreatePageByTitle(title: string) {
  return invoke<Node>("get_or_create_page_by_title", { title });
}

export function search(mode: Mode, query: string, limit = 20) {
  return invoke<SearchHit[]>(SEARCH_CMD[mode], { query, limit });
}

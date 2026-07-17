import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import type { MarkdownLinkTarget, MarkdownOpenDisposition } from "@/features/markdown/types";
import {
  blockTargetHref,
  classifyMarkdownUrl,
  pageTargetHref,
} from "@/features/markdown/url-policy";
import { NOTES_LINK_NODE_NAMES } from "./notes-link-markdown-extension";

type MarkdownSyntaxNode = ReturnType<ReturnType<typeof syntaxTree>["resolveInner"]>;

export type DocumentLinkClick = Readonly<{
  button: number;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}>;

/**
 * Modifier-click gate for editor link navigation. A plain click must remain ordinary caret
 * placement; only an explicit Mod-click opens the typed workspace navigation path, mirroring the
 * Reading renderer's Shift-click-for-adjacent convention layered under a Mod requirement so the
 * editor's own click-to-place-caret behavior is never overloaded.
 */
export function resolveDocumentLinkOpenDisposition(
  event: DocumentLinkClick,
): MarkdownOpenDisposition | null {
  if (event.button !== 0) return null;
  if (!event.ctrlKey && !event.metaKey) return null;
  return event.shiftKey ? "adjacent" : "current";
}

/**
 * Resolves the notes-rs or ordinary Markdown link target under a document position, reusing the
 * exact typed URL policy the Reading renderer applies so editor navigation cannot diverge from it.
 * Returns null for positions with no navigable link, including blocked/invalid targets.
 */
export function resolveDocumentLinkTarget(
  state: EditorState,
  pos: number,
): MarkdownLinkTarget | null {
  const tree = syntaxTree(state);
  for (const side of [1, -1] as const) {
    let node: MarkdownSyntaxNode | null = tree.resolveInner(pos, side);
    while (node) {
      const target = targetForNode(state, node);
      if (target) return target;
      node = node.parent;
    }
  }
  return null;
}

function targetForNode(state: EditorState, node: MarkdownSyntaxNode): MarkdownLinkTarget | null {
  if (node.name === NOTES_LINK_NODE_NAMES.pageLink) {
    const content = node.getChild(NOTES_LINK_NODE_NAMES.pageContent);
    if (!content) return null;
    const title = state.doc.sliceString(content.from, content.to).trim();
    if (!title) return null;
    return allowedTarget(pageTargetHref(title));
  }
  if (node.name === NOTES_LINK_NODE_NAMES.blockLink) {
    const content = node.getChild(NOTES_LINK_NODE_NAMES.blockContent);
    if (!content) return null;
    const uuid = state.doc.sliceString(content.from, content.to);
    return allowedTarget(blockTargetHref(uuid));
  }
  if (node.name === "Link" || node.name === "Autolink") {
    const url = node.getChild("URL");
    if (!url) return null;
    return allowedTarget(state.doc.sliceString(url.from, url.to));
  }
  return null;
}

function allowedTarget(href: string): MarkdownLinkTarget | null {
  const classified = classifyMarkdownUrl(href);
  return classified.kind === "allowed" ? classified.target : null;
}

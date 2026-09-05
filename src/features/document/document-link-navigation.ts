import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import type { MarkdownLinkTarget, MarkdownOpenDisposition } from "@/features/markdown/types";
import {
  blockTargetHref,
  classifyMarkdownUrl,
  pageTargetHref,
} from "@/features/markdown/url-policy";
import type { DocumentAuthoringMode } from "./document-live-preview";
import { isRangeSourceRevealed } from "./document-live-preview";
import { NOTES_LINK_NODE_NAMES } from "./notes-link-markdown-extension";

type MarkdownSyntaxNode = ReturnType<ReturnType<typeof syntaxTree>["resolveInner"]>;

export type DocumentLinkClick = Readonly<{
  button: number;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}>;

export type ResolvedDocumentLink = Readonly<{
  target: MarkdownLinkTarget;
  range: Readonly<{ from: number; to: number }>;
}>;

/**
 * Gates click-to-navigate on the exact same decoration state the editor already renders, so the
 * click behavior can never disagree with what the user sees:
 *
 * - a fully decorated link in Live Preview (its raw source is not revealed) opens on a plain
 *   click, Shift for an adjacent pane — matching the read-only Reading renderer exactly, and
 *   giving touch a working tap-to-navigate gesture for free, with no modifier concept required;
 * - once a link's raw source is revealed (the caret is on that line) or the pane is in Source
 *   mode, where nothing is ever decorated, a plain click must remain ordinary caret placement —
 *   only Mod-click (Mod-Shift for adjacent) forces navigation from there.
 */
export function resolveDocumentLinkOpenDisposition(
  event: DocumentLinkClick,
  mode: DocumentAuthoringMode,
  isDecorated: boolean,
): MarkdownOpenDisposition | null {
  if (event.button !== 0) return null;
  if (mode === "live_preview" && isDecorated) {
    return event.shiftKey ? "adjacent" : "current";
  }
  if (!event.ctrlKey && !event.metaKey) return null;
  return event.shiftKey ? "adjacent" : "current";
}

/**
 * Resolves the tangleaf or ordinary Markdown link under a document position, reusing the exact
 * typed URL policy the Reading renderer applies so editor navigation cannot diverge from it. The
 * returned range matches the exact node range `document-live-preview.ts` decorates, so callers can
 * test it against `isRangeSourceRevealed` for click-gating. Returns null for positions with no
 * navigable link, including blocked/invalid targets.
 */
export function resolveDocumentLink(state: EditorState, pos: number): ResolvedDocumentLink | null {
  const tree = syntaxTree(state);
  for (const side of [1, -1] as const) {
    let node: MarkdownSyntaxNode | null = tree.resolveInner(pos, side);
    while (node) {
      const target = targetForNode(state, node);
      if (target) return { target, range: { from: node.from, to: node.to } };
      node = node.parent;
    }
  }
  return null;
}

/** Convenience wrapper for callers that only need the click-gating decision, not the link. */
export function resolveDocumentLinkClickDisposition(
  state: EditorState,
  pos: number,
  event: DocumentLinkClick,
  mode: DocumentAuthoringMode,
): (ResolvedDocumentLink & { disposition: MarkdownOpenDisposition }) | null {
  const resolved = resolveDocumentLink(state, pos);
  if (!resolved) return null;
  const isDecorated = mode === "live_preview" && !isRangeSourceRevealed(state, resolved.range);
  const disposition = resolveDocumentLinkOpenDisposition(event, mode, isDecorated);
  if (!disposition) return null;
  return { ...resolved, disposition };
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

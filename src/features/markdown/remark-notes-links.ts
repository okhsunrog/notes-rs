import type { Link, PhrasingContent, Root, Text } from "mdast";
import { blockTargetHref, pageTargetHref } from "./url-policy";

const NOTES_LINK =
  /\[\[([^\]\r\n]{1,2048})\]\]|\(\(([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\)\)/gi;

const OPAQUE_PARENT_TYPES = new Set(["code", "inlineCode", "html", "link", "linkReference"]);

/**
 * Converts notes dialect references into ordinary mdast links. The transform
 * only splits text nodes and never descends into Markdown links, inline code,
 * fenced code, or raw HTML.
 */
export function remarkNotesLinks() {
  return (tree: Root): void => transformParent(tree as MutableMdastNode);
}

interface MutableMdastNode {
  children?: MutableMdastNode[];
  type: string;
  value?: string;
}

function transformParent(parent: MutableMdastNode): void {
  if (!parent.children) return;
  const nextChildren: MutableMdastNode[] = [];

  for (const child of parent.children) {
    if (child.type === "text" && typeof child.value === "string") {
      nextChildren.push(...(splitTextNode(child as Text) as MutableMdastNode[]));
      continue;
    }

    if (child.children && !OPAQUE_PARENT_TYPES.has(child.type)) {
      transformParent(child);
    }
    nextChildren.push(child);
  }

  parent.children = nextChildren;
}

export function splitNotesText(value: string): PhrasingContent[] {
  return splitTextNode({ type: "text", value });
}

function splitTextNode(node: Text): PhrasingContent[] {
  NOTES_LINK.lastIndex = 0;
  const children: PhrasingContent[] = [];
  let cursor = 0;
  let match: RegExpExecArray | null;

  while ((match = NOTES_LINK.exec(node.value)) !== null) {
    if (match.index > cursor) children.push(text(node.value.slice(cursor, match.index)));

    const pageTitle = match[1]?.trim();
    const blockUuid = match[2]?.toLowerCase();
    if (pageTitle) {
      children.push(link(pageTargetHref(pageTitle), pageTitle));
    } else if (blockUuid) {
      children.push(link(blockTargetHref(blockUuid), blockUuid));
    } else {
      children.push(text(match[0]));
    }
    cursor = match.index + match[0].length;
  }

  if (cursor === 0) return [node];
  if (cursor < node.value.length) children.push(text(node.value.slice(cursor)));
  return children;
}

function text(value: string): Text {
  return { type: "text", value };
}

function link(url: string, label: string): Link {
  return { type: "link", url, children: [text(label)] };
}

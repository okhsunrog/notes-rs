import type { Link, PhrasingContent, Root, Text } from "mdast";
import { decodeString } from "micromark-util-decode-string";
import { NotesLinkKind, type NotesLinkOccurrence, scanNotesLinks } from "./notes-link-scanner";
import { blockTargetHref, pageTargetHref } from "./url-policy";

const OPAQUE_PARENT_TYPES = new Set(["code", "inlineCode", "html", "link", "linkReference"]);

interface MutableMdastNode {
  children?: MutableMdastNode[];
  position?: {
    end: { offset?: number };
    start: { offset?: number };
  };
  type: string;
  value?: string;
}

interface DecodedOccurrence {
  readonly from: number;
  readonly kind: NotesLinkKind;
  readonly target: string;
  readonly to: number;
}

/**
 * Converts notes dialect references into ordinary mdast links.
 *
 * Candidate discovery always runs against `file.value`, not mdast's escape/entity-decoded text.
 * Source offsets are then verified against the decoded text node. A candidate whose mapping cannot
 * be proven is left inert without preventing a later candidate in the same node from being mapped.
 */
export function remarkNotesLinks() {
  return (tree: Root, file: { value?: unknown }): void => {
    const source = typeof file.value === "string" ? file.value : null;
    transformParent(tree as MutableMdastNode, source);
  };
}

function transformParent(parent: MutableMdastNode, source: string | null): void {
  if (!parent.children || OPAQUE_PARENT_TYPES.has(parent.type)) return;
  const nextChildren: MutableMdastNode[] = [];

  for (const child of parent.children) {
    if (child.type === "text" && typeof child.value === "string") {
      nextChildren.push(...(splitTextNode(child as Text, source) as MutableMdastNode[]));
      continue;
    }
    transformParent(child, source);
    nextChildren.push(child);
  }

  parent.children = nextChildren;
}

/** Pure helper for already-decoded standalone text. The remark plugin uses raw source offsets. */
export function splitNotesText(value: string): PhrasingContent[] {
  return splitDecodedText(value, scanNotesLinks(value).occurrences);
}

function splitTextNode(node: Text, source: string | null): PhrasingContent[] {
  if (!source) return splitNotesText(node.value);
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  if (typeof start !== "number" || typeof end !== "number" || start > end) return [node];

  const raw = source.slice(start, end);
  const mapped = mapOccurrences(raw, node.value, scanNotesLinks(raw).occurrences);
  return mapped.length > 0 ? splitDecodedText(node.value, mapped) : [node];
}

function mapOccurrences(
  raw: string,
  decoded: string,
  occurrences: readonly NotesLinkOccurrence[],
): DecodedOccurrence[] {
  const mapped: DecodedOccurrence[] = [];
  let rawCursor = 0;
  let decodedCursor = 0;

  for (const occurrence of occurrences) {
    const local = {
      decodedCursor,
      rawCursor,
    };
    const sourceFrom = advanceDecodedBoundary(raw, decoded, local, occurrence.sourceRange.from);
    const targetFrom = advanceDecodedBoundary(raw, decoded, local, occurrence.targetRange.from);
    const targetTo = advanceDecodedBoundary(raw, decoded, local, occurrence.targetRange.to);
    const sourceTo = advanceDecodedBoundary(raw, decoded, local, occurrence.sourceRange.to);

    if (
      sourceFrom === null ||
      targetFrom === null ||
      targetTo === null ||
      sourceTo === null ||
      decoded.slice(sourceFrom, sourceTo) !==
        decodeString(raw.slice(occurrence.sourceRange.from, occurrence.sourceRange.to))
    ) {
      const recovered = decodedOffsetForRawPrefix(raw, decoded, occurrence.sourceRange.to);
      if (recovered !== null) {
        rawCursor = occurrence.sourceRange.to;
        decodedCursor = recovered;
      }
      continue;
    }

    const decodedTarget = decoded.slice(targetFrom, targetTo);
    const target =
      occurrence.kind === NotesLinkKind.Page ? decodedTarget.trim() : occurrence.target;
    if (!target) {
      rawCursor = local.rawCursor;
      decodedCursor = local.decodedCursor;
      continue;
    }

    mapped.push({ from: sourceFrom, kind: occurrence.kind, target, to: sourceTo });
    rawCursor = local.rawCursor;
    decodedCursor = local.decodedCursor;
  }

  return mapped;
}

function advanceDecodedBoundary(
  raw: string,
  decoded: string,
  cursor: { decodedCursor: number; rawCursor: number },
  rawBoundary: number,
): number | null {
  if (rawBoundary < cursor.rawCursor) return null;
  const decodedSegment = decodeString(raw.slice(cursor.rawCursor, rawBoundary));
  if (!decoded.startsWith(decodedSegment, cursor.decodedCursor)) return null;
  cursor.rawCursor = rawBoundary;
  cursor.decodedCursor += decodedSegment.length;
  return cursor.decodedCursor;
}

function decodedOffsetForRawPrefix(
  raw: string,
  decoded: string,
  rawBoundary: number,
): number | null {
  const prefix = decodeString(raw.slice(0, rawBoundary));
  return decoded.startsWith(prefix) ? prefix.length : null;
}

function splitDecodedText(
  value: string,
  occurrences: readonly (NotesLinkOccurrence | DecodedOccurrence)[],
): PhrasingContent[] {
  if (occurrences.length === 0) return [text(value)];
  const children: PhrasingContent[] = [];
  let cursor = 0;

  for (const occurrence of occurrences) {
    const from = "sourceRange" in occurrence ? occurrence.sourceRange.from : occurrence.from;
    const to = "sourceRange" in occurrence ? occurrence.sourceRange.to : occurrence.to;
    if (from < cursor || to > value.length) continue;
    if (from > cursor) children.push(text(value.slice(cursor, from)));
    children.push(
      occurrence.kind === NotesLinkKind.Page
        ? link(pageTargetHref(occurrence.target), occurrence.target)
        : link(blockTargetHref(occurrence.target), occurrence.target),
    );
    cursor = to;
  }

  if (cursor === 0) return [text(value)];
  if (cursor < value.length) children.push(text(value.slice(cursor)));
  return children;
}

function text(value: string): Text {
  return { type: "text", value };
}

function link(url: string, label: string): Link {
  return { type: "link", url, children: [text(label)] };
}

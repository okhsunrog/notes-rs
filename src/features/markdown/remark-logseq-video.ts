import type { InlineCode, PhrasingContent, Root } from "mdast";
import { classifyMarkdownUrl } from "./url-policy";

const LOGSEQ_VIDEO_MACRO = /\{\{video[\t ]+([^\r\n{}]+)\}\}/g;
const SOURCE_OPAQUE_TYPES = new Set(["code", "inlineCode", "html"]);
const TRANSFORM_OPAQUE_TYPES = new Set([...SOURCE_OPAQUE_TYPES, "link", "linkReference"]);

interface SourceRange {
  end: number;
  start: number;
}

interface VideoMacro extends SourceRange {
  node: PhrasingContent;
}

interface MutableMdastNode {
  children?: MutableMdastNode[];
  position?: {
    end: { column: number; line: number; offset?: number };
    start: { column: number; line: number; offset?: number };
  };
  type: string;
  value?: string;
}

/**
 * Turns Logseq video macros into an inert, application-opened link intent.
 *
 * The source-range transform deliberately runs after Markdown parsing: it can
 * consume both the ordinary `{{video URL}}` form and Logseq's historical
 * Markdown-link wrapper without allowing the wrapper to create nested HTML.
 * Code, raw HTML, malformed macros, and unsupported URLs remain recoverable.
 */
export function remarkLogseqVideo() {
  return (tree: Root, file: { value?: unknown }): void => {
    const source = typeof file.value === "string" ? file.value : "";
    if (!source) return;

    const forbiddenRanges: SourceRange[] = [];
    collectOpaqueRanges(tree as MutableMdastNode, forbiddenRanges);
    const macros = findVideoMacros(source).filter(
      (macro) => !forbiddenRanges.some((range) => rangesOverlap(macro, range)),
    );
    if (macros.length === 0) return;

    const replaced = new Set<VideoMacro>();
    transformParent(tree as MutableMdastNode, source, macros, replaced);
  };
}

export function parseLogseqVideoMacro(rawMacro: string): PhrasingContent {
  const match = /^\{\{video[\t ]+([^\r\n{}]+)\}\}$/.exec(rawMacro);
  if (!match) return rawFallback(rawMacro);

  const target = unwrapVideoTarget(match[1].trim());
  if (!target) return rawFallback(rawMacro);
  const classified = classifyMarkdownUrl(target);
  if (
    classified.kind !== "allowed" ||
    classified.target.kind !== "external" ||
    classified.target.protocol === "mailto"
  ) {
    return rawFallback(rawMacro);
  }

  return {
    type: "logseqVideo",
    children: [],
    data: {
      hName: "notes-video",
      hProperties: { href: classified.href },
    },
  } as unknown as PhrasingContent;
}

function findVideoMacros(source: string): VideoMacro[] {
  LOGSEQ_VIDEO_MACRO.lastIndex = 0;
  const macros: VideoMacro[] = [];
  let match: RegExpExecArray | null;
  while ((match = LOGSEQ_VIDEO_MACRO.exec(source)) !== null) {
    macros.push({
      start: match.index,
      end: match.index + match[0].length,
      node: parseLogseqVideoMacro(match[0]),
    });
  }
  return macros;
}

function unwrapVideoTarget(argument: string): string | null {
  if (!argument) return null;

  if (argument.startsWith("[")) {
    const labelEnd = argument.indexOf("](");
    if (labelEnd <= 1) return null;
    const destination = argument.slice(labelEnd + 2);
    const normalized = destination.endsWith(")") ? destination.slice(0, -1) : destination;
    return normalized && !/[\s()[\]]/.test(normalized) ? normalized : null;
  }

  return /[\s()[\]]/.test(argument) ? null : argument;
}

function transformParent(
  parent: MutableMdastNode,
  source: string,
  macros: VideoMacro[],
  replaced: Set<VideoMacro>,
): void {
  if (!parent.children || TRANSFORM_OPAQUE_TYPES.has(parent.type)) return;

  const parentRange = nodeRange(parent);
  if (parentRange) {
    const contained = macros
      .filter(
        (macro) =>
          !replaced.has(macro) && macro.start >= parentRange.start && macro.end <= parentRange.end,
      )
      .sort((left, right) => right.start - left.start);
    for (const macro of contained) {
      if (replaceMacroInChildren(parent.children, source, macro)) replaced.add(macro);
    }
  }

  for (const child of parent.children) {
    transformParent(child, source, macros, replaced);
  }
}

function replaceMacroInChildren(
  children: MutableMdastNode[],
  source: string,
  macro: VideoMacro,
): boolean {
  const startIndex = children.findIndex((child) => {
    const range = nodeRange(child);
    return range && range.start <= macro.start && range.end > macro.start;
  });
  if (startIndex < 0) return false;

  let endIndex = -1;
  for (let index = startIndex; index < children.length; index += 1) {
    const range = nodeRange(children[index]);
    if (range && range.start < macro.end && range.end >= macro.end) {
      endIndex = index;
      break;
    }
  }
  if (endIndex < 0) return false;

  const first = children[startIndex];
  const last = children[endIndex];
  const firstRange = nodeRange(first);
  const lastRange = nodeRange(last);
  if (!firstRange || !lastRange) return false;
  if (macro.start > firstRange.start && first.type !== "text") return false;
  if (macro.end < lastRange.end && last.type !== "text") return false;

  const replacement: MutableMdastNode[] = [];
  if (macro.start > firstRange.start) {
    replacement.push(positionedText(source, firstRange.start, macro.start));
  }
  replacement.push(macro.node as MutableMdastNode);
  if (macro.end < lastRange.end) {
    replacement.push(positionedText(source, macro.end, lastRange.end));
  }
  children.splice(startIndex, endIndex - startIndex + 1, ...replacement);
  return true;
}

function collectOpaqueRanges(node: MutableMdastNode, ranges: SourceRange[]): void {
  if (SOURCE_OPAQUE_TYPES.has(node.type)) {
    const range = nodeRange(node);
    if (range) ranges.push(range);
    return;
  }
  for (const child of node.children ?? []) collectOpaqueRanges(child, ranges);
}

function nodeRange(node: MutableMdastNode): SourceRange | null {
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  return typeof start === "number" && typeof end === "number" ? { start, end } : null;
}

function rangesOverlap(left: SourceRange, right: SourceRange): boolean {
  return left.start < right.end && right.start < left.end;
}

function rawFallback(value: string): InlineCode {
  return { type: "inlineCode", value };
}

function positionedText(source: string, start: number, end: number): MutableMdastNode {
  return {
    type: "text",
    value: source.slice(start, end),
    position: {
      start: pointAtOffset(source, start),
      end: pointAtOffset(source, end),
    },
  };
}

function pointAtOffset(source: string, offset: number) {
  const preceding = source.slice(0, offset);
  const lastLineBreak = preceding.lastIndexOf("\n");
  return {
    line: preceding.split("\n").length,
    column: offset - lastLineBreak,
    offset,
  };
}

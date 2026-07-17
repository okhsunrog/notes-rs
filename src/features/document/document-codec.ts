import { markdownLanguage } from "@codemirror/lang-markdown";
import type { BlockStyle, ContentRevision, TaskState } from "@/lib/api";

/** Bump when canonical projection or source-map semantics change. */
export const DOCUMENT_CODEC_VERSION = 1 as const;

export type DocumentOffsetEncoding = "utf-16";

export type DocumentRange = Readonly<{
  from: number;
  to: number;
}>;

export type DocumentBlockSnapshot = Readonly<{
  uuid: string;
  parentUuid: string | null;
  style: BlockStyle;
  markdown: string;
  revision?: ContentRevision;
}>;

export type DocumentSourceMapEntry = Readonly<{
  uuid: string;
  parentUuid: string | null;
  style: BlockStyle;
  markdown: string;
  revision: ContentRevision | undefined;
  depth: number;
  /** Exact syntactic segment owned by this block. */
  sourceRange: DocumentRange;
  /**
   * Tight span after the outer marker/fence. It is contiguous by design, so
   * multiline list/quote content may contain continuation indentation or `>`
   * markers between its first and last semantic character.
   */
  contentRange: DocumentRange;
  semanticSignature: string;
}>;

export type DocumentSourceMap = Readonly<{
  version: typeof DOCUMENT_CODEC_VERSION;
  offsetEncoding: DocumentOffsetEncoding;
  /** Exact buffer whose UTF-16 coordinates the entries describe. */
  sourceMarkdown: string;
  entries: readonly DocumentSourceMapEntry[];
}>;

export type EncodedDocument = Readonly<{
  version: typeof DOCUMENT_CODEC_VERSION;
  markdown: string;
  sourceMap: DocumentSourceMap;
}>;

export type DocumentSemanticUnit = Readonly<{
  previousUuid: string | null;
  previousRevision: ContentRevision | undefined;
  parentIndex: number | null;
  style: BlockStyle;
  markdown: string;
  sourceRange: DocumentRange;
  contentRange: DocumentRange;
}>;

export type ReconciledDocument = Readonly<{
  version: typeof DOCUMENT_CODEC_VERSION;
  offsetEncoding: DocumentOffsetEncoding;
  markdown: string;
  units: readonly DocumentSemanticUnit[];
}>;

type TreeBlock = {
  block: DocumentBlockSnapshot;
  children: TreeBlock[];
};

type Projection = Readonly<{
  source: string;
  contentFrom: number;
  contentTo: number;
}>;

type MarkdownNode = ReturnType<typeof markdownLanguage.parser.parse>["topNode"];

type ParsedUnit = {
  style: BlockStyle;
  markdown: string;
  sourceRange: DocumentRange;
  contentRange: DocumentRange;
  parentIndex: number | null;
  semanticSignature: string;
};

const LIST_KINDS = new Set<BlockStyle["kind"]>(["bullet", "numbered", "task"]);

/**
 * Pure, versioned boundary between the block tree and a marker-free Markdown
 * document. All offsets are JavaScript string offsets (UTF-16 code units), not
 * UTF-8 byte offsets or Unicode scalar indices.
 */
export const DocumentCodec = Object.freeze({
  version: DOCUMENT_CODEC_VERSION,
  encode: encodeDocument,
  reconcile: reconcileDocument,
});

export function encodeDocument(blocks: readonly DocumentBlockSnapshot[]): EncodedDocument {
  const roots = buildTree(blocks);
  const parts: string[] = [];
  const entries: DocumentSourceMapEntry[] = [];
  let offset = 0;

  const append = (node: TreeBlock, indent: string, depth: number) => {
    const projection = projectBlock(node.block, indent);
    if (parts.length > 0) {
      parts.push("\n\n");
      offset += 2;
    }
    const sourceFrom = offset;
    parts.push(projection.source);
    offset += projection.source.length;
    entries.push(
      freezeEntry({
        uuid: node.block.uuid,
        parentUuid: node.block.parentUuid,
        style: cloneStyle(node.block.style),
        markdown: node.block.markdown,
        revision: node.block.revision,
        depth,
        sourceRange: range(sourceFrom, offset),
        contentRange: range(sourceFrom + projection.contentFrom, sourceFrom + projection.contentTo),
        semanticSignature: semanticSignature(node.block.style, node.block.markdown),
      }),
    );
    const childIndent = indent + listContinuationIndent(node.block.style);
    for (const child of node.children) append(child, childIndent, depth + 1);
  };

  for (const root of roots) append(root, "", 0);
  const markdown = parts.join("");
  const sourceMap = Object.freeze({
    version: DOCUMENT_CODEC_VERSION,
    offsetEncoding: "utf-16" as const,
    sourceMarkdown: markdown,
    entries: Object.freeze(entries),
  });
  return Object.freeze({ version: DOCUMENT_CODEC_VERSION, markdown, sourceMap });
}

export function reconcileDocument(
  markdown: string,
  previous: DocumentSourceMap,
): ReconciledDocument {
  assertCompatibleSourceMap(previous);
  if (markdown === previous.sourceMarkdown) {
    return Object.freeze({
      version: DOCUMENT_CODEC_VERSION,
      offsetEncoding: "utf-16" as const,
      markdown,
      units: Object.freeze(exactUnits(previous)),
    });
  }

  const parsed = parseMarkdown(markdown);
  const matches = matchPreviousEntries(parsed, previous.entries);
  const previousIndexByUuid = new Map(previous.entries.map((entry, index) => [entry.uuid, index]));
  const newIndexByPreviousUuid = new Map<string, number>();
  matches.forEach((entryIndex, unitIndex) => {
    if (entryIndex !== null) {
      newIndexByPreviousUuid.set(previous.entries[entryIndex].uuid, unitIndex);
    }
  });

  const units = parsed.map((unit, index): DocumentSemanticUnit => {
    const matchIndex = matches[index];
    const matched = matchIndex === null ? null : previous.entries[matchIndex];
    const exactProjection =
      matched !== null &&
      markdown.slice(unit.sourceRange.from, unit.sourceRange.to) ===
        previous.sourceMarkdown.slice(matched.sourceRange.from, matched.sourceRange.to);
    const style = matched
      ? reconciledStyle(unit.style, matched.style, exactProjection)
      : unit.style;
    const content = exactProjection && matched ? matched.markdown : unit.markdown;
    let parentIndex = unit.parentIndex;
    if (parentIndex === null && matched?.parentUuid) {
      const previousParentIndex = previousIndexByUuid.get(matched.parentUuid);
      const previousParent =
        previousParentIndex === undefined ? undefined : previous.entries[previousParentIndex];
      // Markdown cannot express ownership by a non-list parent. Preserve that
      // external metadata while the child still reconciles to its source-map
      // segment. List indentation, in contrast, is explicit and authoritative.
      if (previousParent && !isListStyle(previousParent.style)) {
        parentIndex = newIndexByPreviousUuid.get(previousParent.uuid) ?? null;
      }
    }
    return freezeUnit({
      previousUuid: matched?.uuid ?? null,
      previousRevision: matched?.revision,
      parentIndex,
      style,
      markdown: content,
      sourceRange: unit.sourceRange,
      contentRange: unit.contentRange,
    });
  });

  return Object.freeze({
    version: DOCUMENT_CODEC_VERSION,
    offsetEncoding: "utf-16" as const,
    markdown,
    units: Object.freeze(units),
  });
}

function buildTree(blocks: readonly DocumentBlockSnapshot[]): TreeBlock[] {
  const nodes = new Map<string, TreeBlock>();
  for (const block of blocks) {
    if (!block.uuid) throw new Error("document block UUID cannot be empty");
    if (nodes.has(block.uuid)) throw new Error(`duplicate document block UUID: ${block.uuid}`);
    nodes.set(block.uuid, { block, children: [] });
  }
  for (const node of nodes.values()) {
    const parentUuid = node.block.parentUuid;
    if (parentUuid === null) continue;
    if (parentUuid === node.block.uuid)
      throw new Error(`document block cannot parent itself: ${parentUuid}`);
    const parent = nodes.get(parentUuid);
    if (!parent) throw new Error(`document block parent is absent: ${parentUuid}`);
    parent.children.push(node);
  }

  const state = new Map<string, "visiting" | "visited">();
  const visit = (node: TreeBlock) => {
    const current = state.get(node.block.uuid);
    if (current === "visiting")
      throw new Error(`document block cycle includes: ${node.block.uuid}`);
    if (current === "visited") return;
    state.set(node.block.uuid, "visiting");
    for (const child of node.children) visit(child);
    state.set(node.block.uuid, "visited");
  };
  for (const node of nodes.values()) visit(node);
  return blocks.filter((block) => block.parentUuid === null).map((block) => nodes.get(block.uuid)!);
}

function projectBlock(block: DocumentBlockSnapshot, indent: string): Projection {
  switch (block.style.kind) {
    case "paragraph":
      return projectPrefixed(block.markdown, indent, indent);
    case "bullet":
      return projectPrefixed(block.markdown, `${indent}- `, `${indent}  `);
    case "numbered":
      return projectPrefixed(block.markdown, `${indent}1. `, `${indent}   `);
    case "task": {
      const checked = canonicalTaskState(block.style.state) === "done";
      return projectPrefixed(block.markdown, `${indent}- [${checked ? "x" : " "}] `, `${indent}  `);
    }
    case "heading_1":
      return projectPrefixed(block.markdown, `${indent}# `, indent);
    case "heading_2":
      return projectPrefixed(block.markdown, `${indent}## `, indent);
    case "heading_3":
      return projectPrefixed(block.markdown, `${indent}### `, indent);
    case "quote":
      return projectQuote(block.markdown, indent);
    case "code":
      return projectCode(block.markdown, indent);
    case "divider": {
      const source = `${indent}---`;
      return Object.freeze({ source, contentFrom: source.length, contentTo: source.length });
    }
  }
}

function projectPrefixed(
  markdown: string,
  firstPrefix: string,
  continuationPrefix: string,
): Projection {
  const lines = markdown.split("\n");
  const rendered = lines.map((line, index) => {
    if (index === 0) return `${firstPrefix}${line}`;
    return line.length === 0 ? "" : `${continuationPrefix}${line}`;
  });
  const source = rendered.join("\n");
  return Object.freeze({
    source,
    contentFrom: firstPrefix.length,
    contentTo: source.length,
  });
}

function projectQuote(markdown: string, indent: string): Projection {
  const prefix = `${indent}> `;
  const source = markdown
    .split("\n")
    .map((line) => (line.length === 0 ? `${indent}>` : `${prefix}${line}`))
    .join("\n");
  return Object.freeze({ source, contentFrom: prefix.length, contentTo: source.length });
}

function projectCode(markdown: string, indent: string): Projection {
  const fence = "`".repeat(Math.max(3, longestRun(markdown, "`") + 1));
  const opening = `${indent}${fence}`;
  const body = markdown
    .split("\n")
    .map((line) => `${indent}${line}`)
    .join("\n");
  const source = `${opening}\n${body}\n${indent}${fence}`;
  const contentFrom = opening.length + 1 + indent.length;
  return Object.freeze({
    source,
    contentFrom,
    contentTo: markdown.length === 0 ? contentFrom : opening.length + 1 + body.length,
  });
}

function longestRun(value: string, character: string): number {
  let longest = 0;
  let current = 0;
  for (const token of value) {
    if (token === character) {
      current += 1;
      longest = Math.max(longest, current);
    } else {
      current = 0;
    }
  }
  return longest;
}

function listContinuationIndent(style: BlockStyle): string {
  if (style.kind === "numbered") return "   ";
  return style.kind === "bullet" || style.kind === "task" ? "  " : "";
}

function parseMarkdown(markdown: string): ParsedUnit[] {
  const units: ParsedUnit[] = [];
  const document = markdownLanguage.parser.parse(markdown).topNode;
  parseContainer(document, null, markdown, units);
  return units;
}

function parseContainer(
  container: MarkdownNode,
  parentIndex: number | null,
  markdown: string,
  units: ParsedUnit[],
) {
  for (let node = container.firstChild; node; node = node.nextSibling) {
    if (node.name === "BulletList" || node.name === "OrderedList") {
      parseList(node, parentIndex, markdown, units);
      continue;
    }
    units.push(parseBlockNode(node, parentIndex, markdown));
  }
}

function parseList(
  list: MarkdownNode,
  parentIndex: number | null,
  markdown: string,
  units: ParsedUnit[],
) {
  const listStyle: BlockStyle =
    list.name === "OrderedList" ? { kind: "numbered" } : { kind: "bullet" };
  for (let item = list.firstChild; item; item = item.nextSibling) {
    if (item.name !== "ListItem") continue;
    const children = directChildren(item);
    const mark = children.find((child) => child.name === "ListMark");
    const nested = children.filter(
      (child) => child.name === "BulletList" || child.name === "OrderedList",
    );
    const task = children.find((child) => child.name === "Task");
    const contentNodes = children.filter(
      (child) => child.name !== "ListMark" && !nested.includes(child),
    );
    const contentEnd = contentNodes[contentNodes.length - 1]?.to ?? mark?.to ?? item.from;
    const taskMarker = task
      ? directChildren(task).find((child) => child.name === "TaskMarker")
      : undefined;
    const contentStart = taskMarker
      ? skipHorizontalWhitespace(taskMarker.to, contentEnd, markdown)
      : skipHorizontalWhitespace(mark?.to ?? item.from, contentEnd, markdown);
    const style = taskMarker
      ? taskStyle(markdown.slice(taskMarker.from, taskMarker.to))
      : listStyle;
    const unitMarkdown = dedentRange(markdown, contentStart, contentEnd);
    const unitIndex = units.length;
    units.push(
      parsedUnit(
        style,
        unitMarkdown,
        range(item.from, contentEnd),
        range(contentStart, contentEnd),
        parentIndex,
      ),
    );
    for (const childList of nested) parseList(childList, unitIndex, markdown, units);
  }
}

function parseBlockNode(
  node: MarkdownNode,
  parentIndex: number | null,
  markdown: string,
): ParsedUnit {
  if (node.name.startsWith("ATXHeading")) {
    const level = Number(node.name.slice("ATXHeading".length));
    const marks = directChildren(node).filter((child) => child.name === "HeaderMark");
    const contentFrom = skipHorizontalWhitespace(marks[0]?.to ?? node.from, node.to, markdown);
    const trailingMark = marks[marks.length - 1];
    const contentTo = trailingMark && trailingMark.from > contentFrom ? trailingMark.from : node.to;
    const style: BlockStyle =
      level === 1
        ? { kind: "heading_1" }
        : level === 2
          ? { kind: "heading_2" }
          : { kind: "heading_3" };
    return parsedUnit(
      style,
      markdown.slice(contentFrom, contentTo).trimEnd(),
      range(node.from, node.to),
      range(contentFrom, contentTo),
      parentIndex,
    );
  }
  if (node.name === "SetextHeading1" || node.name === "SetextHeading2") {
    const mark = directChildren(node).find((child) => child.name === "HeaderMark");
    const contentTo = mark ? trimLineBreakBefore(mark.from, node.from, markdown) : node.to;
    const style: BlockStyle =
      node.name === "SetextHeading1" ? { kind: "heading_1" } : { kind: "heading_2" };
    return parsedUnit(
      style,
      dedentRange(markdown, node.from, contentTo),
      range(node.from, node.to),
      range(node.from, contentTo),
      parentIndex,
    );
  }
  if (node.name === "Blockquote") {
    const contentNode = directChildren(node).find((child) => child.name !== "QuoteMark");
    const contentFrom = contentNode?.from ?? node.to;
    return parsedUnit(
      { kind: "quote" },
      extractQuote(markdown.slice(node.from, node.to)),
      range(node.from, node.to),
      range(contentFrom, node.to),
      parentIndex,
    );
  }
  if (node.name === "FencedCode" || node.name === "CodeBlock") {
    const codeNodes = directChildren(node).filter((child) => child.name === "CodeText");
    const contentFrom = codeNodes[0]?.from ?? codeContentInsertionPoint(node, markdown);
    const contentTo = codeNodes[codeNodes.length - 1]?.to ?? contentFrom;
    return parsedUnit(
      { kind: "code" },
      codeNodes.map((child) => dedentRange(markdown, child.from, child.to)).join("\n"),
      range(node.from, node.to),
      range(contentFrom, contentTo),
      parentIndex,
    );
  }
  if (node.name === "HorizontalRule") {
    return parsedUnit(
      { kind: "divider" },
      "",
      range(node.from, node.to),
      range(node.to, node.to),
      parentIndex,
    );
  }
  return parsedUnit(
    { kind: "paragraph" },
    dedentRange(markdown, node.from, node.to),
    range(node.from, node.to),
    range(node.from, node.to),
    parentIndex,
  );
}

function directChildren(node: MarkdownNode): MarkdownNode[] {
  const children: MarkdownNode[] = [];
  for (let child = node.firstChild; child; child = child.nextSibling) children.push(child);
  return children;
}

function parsedUnit(
  style: BlockStyle,
  content: string,
  sourceRange: DocumentRange,
  contentRange: DocumentRange,
  parentIndex: number | null,
): ParsedUnit {
  return {
    style,
    markdown: content,
    sourceRange,
    contentRange,
    parentIndex,
    semanticSignature: semanticSignature(style, content),
  };
}

function extractQuote(source: string): string {
  return source
    .split("\n")
    .map((line) => line.replace(/^\s*> ?/, ""))
    .join("\n");
}

function dedentRange(markdown: string, from: number, to: number): string {
  const source = markdown.slice(from, to);
  const column = from - (markdown.lastIndexOf("\n", from - 1) + 1);
  if (column === 0 || !source.includes("\n")) return source;
  const indent = new RegExp(`^ {0,${column}}`);
  return source
    .split("\n")
    .map((line, index) => (index === 0 ? line : line.replace(indent, "")))
    .join("\n");
}

function skipHorizontalWhitespace(from: number, to: number, markdown: string): number {
  let offset = from;
  while (offset < to && (markdown[offset] === " " || markdown[offset] === "\t")) offset += 1;
  return offset;
}

function trimLineBreakBefore(from: number, floor: number, markdown: string): number {
  let offset = from;
  while (offset > floor && (markdown[offset - 1] === "\n" || markdown[offset - 1] === "\r")) {
    offset -= 1;
  }
  return offset;
}

function codeContentInsertionPoint(node: MarkdownNode, markdown: string): number {
  const mark = directChildren(node).find((child) => child.name === "CodeMark");
  if (!mark) return node.from;
  const newline = markdown.indexOf("\n", mark.to);
  return newline === -1 || newline >= node.to ? mark.to : newline + 1;
}

function taskStyle(marker: string): BlockStyle {
  return { kind: "task", state: /\[[xX]\]/.test(marker) ? "done" : "todo" };
}

function semanticSignature(style: BlockStyle, markdown: string): string {
  const syntaxStyle =
    style.kind === "task" ? `task:${canonicalTaskState(style.state)}` : style.kind;
  return JSON.stringify([syntaxStyle, markdown]);
}

function canonicalTaskState(state: TaskState): "todo" | "done" {
  return state === "done" || state === "cancelled" ? "done" : "todo";
}

function reconciledStyle(
  parsed: BlockStyle,
  previous: BlockStyle,
  exactProjection: boolean,
): BlockStyle {
  if (exactProjection) return cloneStyle(previous);
  if (parsed.kind === "task" && previous.kind === "task") {
    return canonicalTaskState(parsed.state) === canonicalTaskState(previous.state)
      ? cloneStyle(previous)
      : cloneStyle(parsed);
  }
  return parsed.kind === previous.kind ? cloneStyle(previous) : cloneStyle(parsed);
}

function matchPreviousEntries(
  units: readonly ParsedUnit[],
  previous: readonly DocumentSourceMapEntry[],
): Array<number | null> {
  const matches = Array<number | null>(units.length).fill(null);
  const claimed = new Set<number>();
  const unitSignatures = signatureIndices(units.map((unit) => unit.semanticSignature));
  const previousSignatures = signatureIndices(previous.map((entry) => entry.semanticSignature));
  const protectedPairs = new Map<number, number>();
  for (const [signature, unitIndices] of unitSignatures) {
    const previousIndices = previousSignatures.get(signature);
    if (unitIndices.length === 1 && previousIndices?.length === 1) {
      protectedPairs.set(unitIndices[0], previousIndices[0]);
    }
  }

  const candidates = new Map<number, { previousIndex: number; score: number }>();
  units.forEach((unit, unitIndex) => {
    const protectedPrevious = protectedPairs.get(unitIndex);
    let maximum = 0;
    let maximumIndices: number[] = [];
    previous.forEach((entry, previousIndex) => {
      if (protectedPrevious !== undefined && previousIndex !== protectedPrevious) return;
      for (const [otherUnit, protectedIndex] of protectedPairs) {
        if (otherUnit !== unitIndex && protectedIndex === previousIndex) return;
      }
      const score = overlap(unit.sourceRange, entry.sourceRange);
      if (score > maximum) {
        maximum = score;
        maximumIndices = [previousIndex];
      } else if (score > 0 && score === maximum) {
        maximumIndices.push(previousIndex);
      }
    });
    if (maximum > 0 && maximumIndices.length === 1) {
      candidates.set(unitIndex, { previousIndex: maximumIndices[0], score: maximum });
    }
  });

  const claimsByPrevious = new Map<number, Array<{ unitIndex: number; score: number }>>();
  for (const [unitIndex, candidate] of candidates) {
    const claims = claimsByPrevious.get(candidate.previousIndex) ?? [];
    claims.push({ unitIndex, score: candidate.score });
    claimsByPrevious.set(candidate.previousIndex, claims);
  }
  for (const [previousIndex, claims] of claimsByPrevious) {
    const maximum = Math.max(...claims.map((claim) => claim.score));
    const winners = claims.filter((claim) => claim.score === maximum);
    if (winners.length !== 1) continue;
    matches[winners[0].unitIndex] = previousIndex;
    claimed.add(previousIndex);
  }

  for (const [unitIndex, previousIndex] of protectedPairs) {
    if (matches[unitIndex] === null && !claimed.has(previousIndex)) {
      matches[unitIndex] = previousIndex;
      claimed.add(previousIndex);
    }
  }
  return matches;
}

function signatureIndices(signatures: readonly string[]): Map<string, number[]> {
  const indices = new Map<string, number[]>();
  signatures.forEach((signature, index) => {
    const rows = indices.get(signature) ?? [];
    rows.push(index);
    indices.set(signature, rows);
  });
  return indices;
}

function overlap(left: DocumentRange, right: DocumentRange): number {
  return Math.max(0, Math.min(left.to, right.to) - Math.max(left.from, right.from));
}

function exactUnits(sourceMap: DocumentSourceMap): DocumentSemanticUnit[] {
  const indexByUuid = new Map(sourceMap.entries.map((entry, index) => [entry.uuid, index]));
  return sourceMap.entries.map((entry) =>
    freezeUnit({
      previousUuid: entry.uuid,
      previousRevision: entry.revision,
      parentIndex: entry.parentUuid === null ? null : (indexByUuid.get(entry.parentUuid) ?? null),
      style: cloneStyle(entry.style),
      markdown: entry.markdown,
      sourceRange: entry.sourceRange,
      contentRange: entry.contentRange,
    }),
  );
}

function assertCompatibleSourceMap(sourceMap: DocumentSourceMap) {
  const version = Number(sourceMap.version);
  if (version !== DOCUMENT_CODEC_VERSION) {
    throw new Error(`unsupported document codec version: ${version}`);
  }
  const offsetEncoding = String(sourceMap.offsetEncoding);
  if (offsetEncoding !== "utf-16") {
    throw new Error(`unsupported document offset encoding: ${offsetEncoding}`);
  }
}

function isListStyle(style: BlockStyle): boolean {
  return LIST_KINDS.has(style.kind);
}

function cloneStyle(style: BlockStyle): BlockStyle {
  return Object.freeze(
    style.kind === "task" ? { kind: "task", state: style.state } : { kind: style.kind },
  );
}

function range(from: number, to: number): DocumentRange {
  return Object.freeze({ from, to });
}

function freezeEntry(entry: DocumentSourceMapEntry): DocumentSourceMapEntry {
  return Object.freeze(entry);
}

function freezeUnit(unit: DocumentSemanticUnit): DocumentSemanticUnit {
  return Object.freeze({
    ...unit,
    style: cloneStyle(unit.style),
    sourceRange: range(unit.sourceRange.from, unit.sourceRange.to),
    contentRange: range(unit.contentRange.from, unit.contentRange.to),
  });
}

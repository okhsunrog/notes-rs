import type { InlineContext, MarkdownExtension } from "@lezer/markdown";
import {
  NotesLinkKind,
  type NotesLinkOccurrence,
  scanNotesLinks,
} from "@/features/markdown/notes-link-scanner";

export { MAX_NOTES_LINK_CONTENT_LENGTH } from "@/features/markdown/notes-link-scanner";

export const NOTES_LINK_NODE_NAMES = {
  blockContent: "NotesBlockLinkContent",
  blockLink: "NotesBlockLink",
  blockMark: "NotesBlockLinkMark",
  pageContent: "NotesPageLinkContent",
  pageLink: "NotesPageLink",
  pageMark: "NotesPageLinkMark",
} as const;

export type NotesLinkNodeName = (typeof NOTES_LINK_NODE_NAMES)[keyof typeof NOTES_LINK_NODE_NAMES];
export type NotesLinkNodeRole = "content" | "link" | "mark";

export const NOTES_LINK_MARK_NODE_NAMES = [
  NOTES_LINK_NODE_NAMES.pageMark,
  NOTES_LINK_NODE_NAMES.blockMark,
] as const satisfies readonly NotesLinkNodeName[];

export const NOTES_LINK_CONTENT_NODE_NAMES = [
  NOTES_LINK_NODE_NAMES.pageContent,
  NOTES_LINK_NODE_NAMES.blockContent,
] as const satisfies readonly NotesLinkNodeName[];

export const NOTES_LINK_CONTAINER_NODE_NAMES = [
  NOTES_LINK_NODE_NAMES.pageLink,
  NOTES_LINK_NODE_NAMES.blockLink,
] as const satisfies readonly NotesLinkNodeName[];

const ALL_NOTES_LINK_NODE_NAMES = new Set<NotesLinkNodeName>([
  ...NOTES_LINK_CONTAINER_NODE_NAMES,
  ...NOTES_LINK_MARK_NODE_NAMES,
  ...NOTES_LINK_CONTENT_NODE_NAMES,
]);

/** Typed allowlist helper for Live Preview decorations and source-reveal ranges. */
export function isNotesLinkNodeName(name: string): name is NotesLinkNodeName {
  return ALL_NOTES_LINK_NODE_NAMES.has(name as NotesLinkNodeName);
}

export function notesLinkNodeRole(name: string): NotesLinkNodeRole | null {
  if (!isNotesLinkNodeName(name)) return null;
  if ((NOTES_LINK_MARK_NODE_NAMES as readonly string[]).includes(name)) return "mark";
  if ((NOTES_LINK_CONTENT_NODE_NAMES as readonly string[]).includes(name)) return "content";
  return "link";
}

interface NotesLinkContextPolicy {
  readonly occurrenceEndingAt: ReadonlyMap<number, NotesLinkOccurrence>;
  readonly occurrences: ReadonlyMap<number, NotesLinkOccurrence>;
}

const policyCache = new WeakMap<InlineContext, NotesLinkContextPolicy>();

/**
 * tangleaf internal-link dialect for CodeMirror's Lezer Markdown parser.
 *
 * `[[Page|Alias]]` intentionally has one content node. The Reading renderer currently treats the
 * entire `Page|Alias` text as both label and target; splitting alias semantics here would make Live
 * Preview disagree with Reading.
 */
export const notesLinkMarkdownExtension: MarkdownExtension = {
  defineNodes: Object.values(NOTES_LINK_NODE_NAMES),
  parseInline: [
    {
      before: "Link",
      name: "NotesLinks",
      parse(cx, _next, pos) {
        const policy = policyFor(cx);
        const occurrence = policy.occurrences.get(pos - cx.offset);
        if (!occurrence) return -1;
        if (cx.hasOpenLink && hasOrdinaryOpenLink(cx, pos, policy)) return -1;
        return addOccurrence(cx, occurrence);
      },
    },
  ],
};

function policyFor(cx: InlineContext): NotesLinkContextPolicy {
  const cached = policyCache.get(cx);
  if (cached) return cached;
  const scanned = scanNotesLinks(cx.text).occurrences;
  const occurrences = new Map<number, NotesLinkOccurrence>();
  const occurrenceEndingAt = new Map<number, NotesLinkOccurrence>();
  for (const occurrence of scanned) {
    occurrences.set(occurrence.sourceRange.from, occurrence);
    occurrenceEndingAt.set(occurrence.sourceRange.to - 1, occurrence);
  }
  const policy = { occurrences, occurrenceEndingAt };
  policyCache.set(cx, policy);
  return policy;
}

/**
 * `InlineContext.hasOpenLink` is authoritative for whether Lezer is inside a Markdown label, but
 * it can also stay true while our own adjacent `[[...]]` nodes are being parsed. Walk backwards,
 * jumping over complete notes links and balancing already-closed ordinary brackets, to distinguish
 * those two cases without reparsing the inline source through a second Markdown grammar.
 */
function hasOrdinaryOpenLink(
  cx: InlineContext,
  position: number,
  policy: NotesLinkContextPolicy,
): boolean {
  let cursor = position - cx.offset - 1;
  let closedBrackets = 0;
  while (cursor >= 0) {
    const occurrence = policy.occurrenceEndingAt.get(cursor);
    if (occurrence) {
      cursor = occurrence.sourceRange.from - 1;
      continue;
    }

    const code = cx.text.charCodeAt(cursor);
    if (code === 0x5d && !isEscaped(cx.text, cursor)) {
      closedBrackets += 1;
    } else if (code === 0x5b && !isEscaped(cx.text, cursor)) {
      if (cx.text.charCodeAt(cursor - 1) === 0x5b || cx.text.charCodeAt(cursor + 1) === 0x5b) {
        cursor -= cx.text.charCodeAt(cursor - 1) === 0x5b ? 2 : 1;
        continue;
      }
      if (closedBrackets > 0) closedBrackets -= 1;
      else return true;
    }
    cursor -= 1;
  }
  return false;
}

function isEscaped(source: string, position: number): boolean {
  let backslashes = 0;
  for (let cursor = position - 1; cursor >= 0 && source.charCodeAt(cursor) === 0x5c; cursor -= 1) {
    backslashes += 1;
  }
  return backslashes % 2 === 1;
}

function addOccurrence(cx: InlineContext, occurrence: NotesLinkOccurrence): number {
  const offset = cx.offset;
  const sourceFrom = offset + occurrence.sourceRange.from;
  const sourceTo = offset + occurrence.sourceRange.to;
  const contentFrom = offset + occurrence.contentRange.from;
  const contentTo = offset + occurrence.contentRange.to;
  const names =
    occurrence.kind === NotesLinkKind.Page
      ? {
          content: NOTES_LINK_NODE_NAMES.pageContent,
          link: NOTES_LINK_NODE_NAMES.pageLink,
          mark: NOTES_LINK_NODE_NAMES.pageMark,
        }
      : {
          content: NOTES_LINK_NODE_NAMES.blockContent,
          link: NOTES_LINK_NODE_NAMES.blockLink,
          mark: NOTES_LINK_NODE_NAMES.blockMark,
        };
  return cx.addElement(
    cx.elt(names.link, sourceFrom, sourceTo, [
      cx.elt(names.mark, sourceFrom, contentFrom),
      cx.elt(names.content, contentFrom, contentTo),
      cx.elt(names.mark, contentTo, sourceTo),
    ]),
  );
}

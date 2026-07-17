/** Maximum page-link target length, in JavaScript/CodeMirror UTF-16 offsets. */
export const MAX_NOTES_LINK_CONTENT_LENGTH = 2_048;
/** A malformed line beyond this depth is left inert until its next line break. */
export const MAX_OPEN_NOTES_LINK_CANDIDATES = 1_024;

export enum NotesLinkKind {
  Block = "block",
  Page = "page",
}

export enum NotesLinkRejectionReason {
  EmptyTarget = "empty_target",
  InvalidTarget = "invalid_target",
  NestedDelimiter = "nested_delimiter",
  Newline = "newline",
  Oversized = "oversized",
  Unclosed = "unclosed",
}

export interface NotesLinkRange {
  readonly from: number;
  readonly to: number;
}

export interface NotesLinkOccurrence {
  /** Raw content between the two opening and two closing marks. */
  readonly contentRange: NotesLinkRange;
  readonly kind: NotesLinkKind;
  /** Complete source range, including both pairs of marks. */
  readonly sourceRange: NotesLinkRange;
  /** Normalized target: Unicode-trimmed page title or lowercase canonical UUID. */
  readonly target: string;
  /** Trimmed target range. For block links this equals `contentRange`. */
  readonly targetRange: NotesLinkRange;
}

export interface NotesLinkRejection {
  readonly kind: NotesLinkKind;
  readonly reason: NotesLinkRejectionReason;
  readonly sourceRange: NotesLinkRange;
}

export interface NotesLinkScanResult {
  /** Null in the allocation-minimal production mode. */
  readonly diagnostics: NotesLinkDiagnostics | null;
  readonly occurrences: readonly NotesLinkOccurrence[];
}

export interface NotesLinkDiagnostics {
  /** True when a line exceeded the bounded candidate stack and was intentionally left inert. */
  readonly candidateLimitReached: boolean;
  readonly rejections: readonly NotesLinkRejection[];
  /** True when at least one further diagnostic was omitted by `maxRejections`. */
  readonly truncated: boolean;
}

export interface NotesLinkScanOptions {
  /** Omit for occurrences-only production scanning. */
  readonly diagnostics?: {
    readonly maxRejections: number;
  };
}

interface Candidate {
  readonly contentFrom: number;
  invalidInlineMarkdown: boolean;
  invalidPageContent: boolean;
  readonly kind: NotesLinkKind;
  nested: boolean;
  readonly occurrenceCheckpoint: number;
  readonly previousSameKind: number;
  readonly start: number;
}

interface DiagnosticCollector {
  candidateLimitReached: boolean;
  readonly limit: number;
  readonly rejections: NotesLinkRejection[];
  truncated: boolean;
}

const OPEN_SQUARE = 0x5b;
const CLOSE_SQUARE = 0x5d;
const OPEN_PAREN = 0x28;
const CLOSE_PAREN = 0x29;
const BACKSLASH = 0x5c;
const LINE_FEED = 0x0a;
const CARRIAGE_RETURN = 0x0d;
const AST_SPLITTING_PAGE_CHARACTERS = new Set([
  0x24, // $: remark-math
  0x2a, // *: emphasis
  0x3c, // <: raw HTML / explicit autolink
  0x3e, // >: raw HTML / explicit autolink
  0x5f, // _: emphasis
  0x60, // `: inline code
  0x7b, // {: notes-rs Logseq macro extensions
  0x7d, // }: notes-rs Logseq macro extensions
  0x7e, // ~: GFM strikethrough
]);
const GFM_URL_AUTOLINK = /(?:https?:\/\/|www\.)[^\s]+/i;
const GFM_EMAIL_AUTOLINK = /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/i;
const BLOCK_UUID_LENGTH = 36;

/**
 * Scans notes-rs internal links in one deterministic left-to-right pass.
 *
 * This function deliberately knows nothing about Markdown AST opacity. Callers decide which text
 * ranges are eligible (Reading skips code/HTML/ordinary links; Lezer does the equivalent through
 * its inline parser context). Nested candidates are retained only when their outer opener is
 * unclosed. A syntactically closed outer candidate suppresses all same-kind and cross-kind nested
 * candidates, matching the Rust reference scanner's recovery policy.
 */
export function scanNotesLinks(
  source: string,
  options: NotesLinkScanOptions = {},
): NotesLinkScanResult {
  const occurrences: NotesLinkOccurrence[] = [];
  const diagnostics = createDiagnosticCollector(options);
  const stack: Candidate[] = [];
  const topByKind = {
    [NotesLinkKind.Block]: -1,
    [NotesLinkKind.Page]: -1,
  };
  let cursor = 0;
  let precedingBackslashes = 0;
  let suppressUntilLineBreak = false;

  while (cursor < source.length) {
    const current = source.charCodeAt(cursor);
    if (suppressUntilLineBreak) {
      if (current === LINE_FEED || current === CARRIAGE_RETURN) {
        suppressUntilLineBreak = false;
        precedingBackslashes = 0;
        if (current === CARRIAGE_RETURN && source.charCodeAt(cursor + 1) === LINE_FEED) cursor += 1;
      }
      cursor += 1;
      continue;
    }
    if (current === BACKSLASH) {
      precedingBackslashes += 1;
      cursor += 1;
      continue;
    }

    const escaped = precedingBackslashes % 2 === 1;
    precedingBackslashes = 0;

    if (current === LINE_FEED || current === CARRIAGE_RETURN) {
      flushUnclosed(stack, diagnostics, cursor, NotesLinkRejectionReason.Newline);
      topByKind[NotesLinkKind.Block] = -1;
      topByKind[NotesLinkKind.Page] = -1;
      if (current === CARRIAGE_RETURN && source.charCodeAt(cursor + 1) === LINE_FEED) cursor += 1;
      cursor += 1;
      continue;
    }

    const opener = escaped ? null : openerAt(source, cursor);
    if (opener) {
      if (stack.length >= MAX_OPEN_NOTES_LINK_CANDIDATES) {
        // Everything since the oldest still-open candidate is structurally ambiguous. Discard
        // nested occurrences and fail closed for the rest of this malformed line. This bounds
        // production memory independently of the document or paragraph size.
        occurrences.length = stack[0]?.occurrenceCheckpoint ?? occurrences.length;
        if (diagnostics) {
          diagnostics.candidateLimitReached = true;
          diagnostics.truncated = true;
        }
        stack.length = 0;
        topByKind[NotesLinkKind.Block] = -1;
        topByKind[NotesLinkKind.Page] = -1;
        suppressUntilLineBreak = true;
        cursor += 2;
        continue;
      }
      const parent = stack[stack.length - 1];
      if (parent) parent.nested = true;
      stack.push({
        contentFrom: cursor + 2,
        invalidInlineMarkdown: false,
        invalidPageContent: false,
        kind: opener,
        nested: false,
        occurrenceCheckpoint: occurrences.length,
        previousSameKind: topByKind[opener],
        start: cursor,
      });
      topByKind[opener] = stack.length - 1;
      cursor += 2;
      continue;
    }

    const closingKind = closingKindAt(source, cursor);
    if (closingKind) {
      const matchingIndex = topByKind[closingKind];
      if (matchingIndex >= 0) {
        const candidate = stack[matchingIndex];
        rewindKindTops(stack, topByKind, matchingIndex);
        stack.length = matchingIndex;
        closeCandidate(source, candidate, cursor, occurrences, diagnostics);
        cursor += 2;
        continue;
      }
    }

    if (
      current === CLOSE_SQUARE &&
      source.charCodeAt(cursor + 1) !== CLOSE_SQUARE &&
      stack[stack.length - 1]?.kind === NotesLinkKind.Page
    ) {
      // The UI dialect intentionally retains the former renderer's stricter `[^\]]` policy.
      // Rust reference discovery is broader because it reports recoverable source references.
      stack[stack.length - 1].invalidPageContent = true;
    }
    if (
      !escaped &&
      stack[stack.length - 1]?.kind === NotesLinkKind.Page &&
      AST_SPLITTING_PAGE_CHARACTERS.has(current)
    ) {
      stack[stack.length - 1].invalidInlineMarkdown = true;
    }

    cursor += 1;
  }

  flushUnclosed(stack, diagnostics, source.length, NotesLinkRejectionReason.Unclosed);
  return {
    diagnostics: diagnostics
      ? {
          candidateLimitReached: diagnostics.candidateLimitReached,
          rejections: diagnostics.rejections,
          truncated: diagnostics.truncated,
        }
      : null,
    occurrences,
  };
}

function openerAt(source: string, position: number): NotesLinkKind | null {
  const current = source.charCodeAt(position);
  const next = source.charCodeAt(position + 1);
  if (current === OPEN_SQUARE && next === OPEN_SQUARE) return NotesLinkKind.Page;
  if (current === OPEN_PAREN && next === OPEN_PAREN) return NotesLinkKind.Block;
  return null;
}

function closingKindAt(source: string, position: number): NotesLinkKind | null {
  const current = source.charCodeAt(position);
  const next = source.charCodeAt(position + 1);
  if (current === CLOSE_SQUARE && next === CLOSE_SQUARE) return NotesLinkKind.Page;
  if (current === CLOSE_PAREN && next === CLOSE_PAREN) return NotesLinkKind.Block;
  return null;
}

function rewindKindTops(
  stack: readonly Candidate[],
  topByKind: Record<NotesLinkKind, number>,
  removedFrom: number,
): void {
  for (const kind of [NotesLinkKind.Page, NotesLinkKind.Block]) {
    while (topByKind[kind] >= removedFrom) {
      topByKind[kind] = stack[topByKind[kind]].previousSameKind;
    }
  }
}

function closeCandidate(
  source: string,
  candidate: Candidate,
  closeFrom: number,
  occurrences: NotesLinkOccurrence[],
  diagnostics: DiagnosticCollector | null,
): void {
  const closedTo = closeFrom + 2;
  if (candidate.nested) {
    occurrences.length = candidate.occurrenceCheckpoint;
    recordRejection(diagnostics, candidate, closedTo, NotesLinkRejectionReason.NestedDelimiter);
    return;
  }
  if (candidate.kind === NotesLinkKind.Page) {
    const contentLength = closeFrom - candidate.contentFrom;
    if (contentLength > MAX_NOTES_LINK_CONTENT_LENGTH) {
      recordRejection(diagnostics, candidate, closedTo, NotesLinkRejectionReason.Oversized);
      return;
    }
    if (candidate.invalidPageContent || candidate.invalidInlineMarkdown) {
      recordRejection(diagnostics, candidate, closedTo, NotesLinkRejectionReason.InvalidTarget);
      return;
    }
    const content = source.slice(candidate.contentFrom, closeFrom);
    if (GFM_URL_AUTOLINK.test(content) || GFM_EMAIL_AUTOLINK.test(content)) {
      recordRejection(diagnostics, candidate, closedTo, NotesLinkRejectionReason.InvalidTarget);
      return;
    }
    const trimmed = content.trim();
    if (!trimmed) {
      recordRejection(diagnostics, candidate, closedTo, NotesLinkRejectionReason.EmptyTarget);
      return;
    }
    const leadingWhitespace = content.length - content.trimStart().length;
    const targetFrom = candidate.contentFrom + leadingWhitespace;
    occurrences.push({
      contentRange: { from: candidate.contentFrom, to: closeFrom },
      kind: NotesLinkKind.Page,
      sourceRange: { from: candidate.start, to: closedTo },
      target: trimmed,
      targetRange: { from: targetFrom, to: targetFrom + trimmed.length },
    });
    return;
  }

  const content = source.slice(candidate.contentFrom, closeFrom);
  if (!isCanonicalUuid(content)) {
    recordRejection(diagnostics, candidate, closedTo, NotesLinkRejectionReason.InvalidTarget);
    return;
  }
  occurrences.push({
    contentRange: { from: candidate.contentFrom, to: closeFrom },
    kind: NotesLinkKind.Block,
    sourceRange: { from: candidate.start, to: closedTo },
    target: content.toLowerCase(),
    targetRange: { from: candidate.contentFrom, to: closeFrom },
  });
}

function recordRejection(
  diagnostics: DiagnosticCollector | null,
  candidate: Candidate,
  to: number,
  reason: NotesLinkRejectionReason,
): void {
  if (!diagnostics) return;
  if (diagnostics.rejections.length >= diagnostics.limit) {
    diagnostics.truncated = true;
    return;
  }
  diagnostics.rejections.push({
    kind: candidate.kind,
    reason,
    sourceRange: { from: candidate.start, to },
  });
}

function flushUnclosed(
  stack: Candidate[],
  diagnostics: DiagnosticCollector | null,
  boundary: number,
  reason: NotesLinkRejectionReason,
): void {
  for (const candidate of stack) {
    recordRejection(
      diagnostics,
      candidate,
      boundary,
      boundary - candidate.contentFrom > MAX_NOTES_LINK_CONTENT_LENGTH
        ? NotesLinkRejectionReason.Oversized
        : reason,
    );
  }
  stack.length = 0;
}

function createDiagnosticCollector(options: NotesLinkScanOptions): DiagnosticCollector | null {
  const maxRejections = options.diagnostics?.maxRejections;
  if (maxRejections === undefined) return null;
  if (!Number.isSafeInteger(maxRejections) || maxRejections < 0) {
    throw new RangeError(
      "notes-link diagnostics.maxRejections must be a non-negative safe integer",
    );
  }
  return {
    candidateLimitReached: false,
    limit: maxRejections,
    rejections: [],
    truncated: false,
  };
}

function isCanonicalUuid(value: string): boolean {
  if (value.length !== BLOCK_UUID_LENGTH) return false;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (index === 8 || index === 13 || index === 18 || index === 23) {
      if (code !== 0x2d) return false;
    } else if (!isAsciiHex(code)) {
      return false;
    }
  }
  return true;
}

function isAsciiHex(code: number): boolean {
  return (
    (code >= 0x30 && code <= 0x39) ||
    (code >= 0x41 && code <= 0x46) ||
    (code >= 0x61 && code <= 0x66)
  );
}

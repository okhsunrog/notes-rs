import { describe, expect, it } from "vite-plus/test";
import {
  MAX_NOTES_LINK_CONTENT_LENGTH,
  MAX_OPEN_NOTES_LINK_CANDIDATES,
  NotesLinkKind,
  NotesLinkRejectionReason,
  scanNotesLinks,
} from "./notes-link-scanner";

const UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";

function targets(source: string) {
  return scanNotesLinks(source).occurrences.map(({ kind, sourceRange, target }) => ({
    kind,
    source: source.slice(sourceRange.from, sourceRange.to),
    target,
  }));
}

function scanWithDiagnostics(source: string, maxRejections = 128) {
  return scanNotesLinks(source, { diagnostics: { maxRejections } });
}

describe("scanNotesLinks", () => {
  it("returns typed lossless ranges and normalized targets", () => {
    const upperUuid = UUID.toUpperCase();
    const source = `before [[  Архитектура | Alias  ]] and ((${upperUuid}))`;

    expect(targets(source)).toEqual([
      {
        kind: NotesLinkKind.Page,
        source: "[[  Архитектура | Alias  ]]",
        target: "Архитектура | Alias",
      },
      { kind: NotesLinkKind.Block, source: `((${upperUuid}))`, target: UUID },
    ]);
  });

  it("applies odd-backslash escaping and accepts even backslashes", () => {
    const source = String.raw`\[[ignored]] \\[[kept]] \((${UUID})) \\((${UUID.toUpperCase()}))`;
    expect(targets(source).map(({ target }) => target)).toEqual(["kept", UUID]);
  });

  it.each([
    ["same-kind", "[[outer [[inner]] tail]]"],
    ["cross-kind", `[[outer ((${UUID})) tail]]`],
    ["block outer", `((outer [[inner]] ((${UUID})) tail))`],
  ])("suppresses every nested candidate in a closed %s construct", (_name, source) => {
    const scan = scanWithDiagnostics(source);
    expect(scan.occurrences).toEqual([]);
    expect(scan.diagnostics?.rejections).toContainEqual(
      expect.objectContaining({ reason: NotesLinkRejectionReason.NestedDelimiter }),
    );
  });

  it("recovers a valid inner candidate when its outer opener is unclosed", () => {
    const source = "[[broken and [[valid]]";
    expect(targets(source)).toEqual([
      { kind: NotesLinkKind.Page, source: "[[valid]]", target: "valid" },
    ]);
    expect(scanWithDiagnostics(source).diagnostics?.rejections[0]?.reason).toBe(
      NotesLinkRejectionReason.Unclosed,
    );
  });

  it("rejects newline, unclosed, Unicode-empty, invalid UUID, and stray page brackets", () => {
    for (const source of [
      "[[line\nbreak]]",
      "[[unclosed",
      "[[\u00a0\u2003]]",
      "((not-a-uuid))",
      "[[has]bracket]]",
    ]) {
      expect(scanNotesLinks(source).occurrences, source).toEqual([]);
    }
  });

  it("accepts exactly 2048 page-link code units and rejects 2049", () => {
    const exact = `[[${"x".repeat(MAX_NOTES_LINK_CONTENT_LENGTH)}]]`;
    const oversized = `[[${"x".repeat(MAX_NOTES_LINK_CONTENT_LENGTH + 1)}]]`;
    expect(scanNotesLinks(exact).occurrences).toHaveLength(1);
    expect(scanNotesLinks(oversized).occurrences).toEqual([]);
    expect(scanWithDiagnostics(oversized).diagnostics?.rejections[0]?.reason).toBe(
      NotesLinkRejectionReason.Oversized,
    );
  });

  it("rejects mdast-splitting inline syntax but preserves escaped punctuation and entities", () => {
    for (const source of [
      "[[**bold**]]",
      "[[a *b* c]]",
      "[[a `code` c]]",
      "[[a <em>b</em>]]",
      "[[a $x$ c]]",
      "[[https://example.com]]",
      "[[www.example.com]]",
      "[[user@example.com]]",
      "[[a {{video https://video.example/x}} c]]",
    ]) {
      expect(scanNotesLinks(source).occurrences, source).toEqual([]);
    }
    expect(targets(String.raw`[[a \* b]]`).map(({ target }) => target)).toEqual([
      String.raw`a \* b`,
    ]);
    expect(targets("[[a &amp; b]]").map(({ target }) => target)).toEqual(["a &amp; b"]);
  });

  it("scans adjacent mixed links without deduplication", () => {
    expect(targets(`[[One]][[Two]]((${UUID}))`).map(({ target }) => target)).toEqual([
      "One",
      "Two",
      UUID,
    ]);
  });

  it("caps diagnostics on an adversarial deeply nested unclosed line", () => {
    const openerCount = 20_000;
    const scan = scanWithDiagnostics("[[".repeat(openerCount), 32);

    expect(scan.occurrences).toEqual([]);
    expect(scan.diagnostics?.rejections.length).toBeLessThanOrEqual(32);
    expect(scan.diagnostics?.candidateLimitReached).toBe(true);
    expect(scan.diagnostics?.truncated).toBe(true);
  });

  it("validates an explicit diagnostics cap", () => {
    expect(() => scanNotesLinks("[[broken", { diagnostics: { maxRejections: -1 } })).toThrow(
      RangeError,
    );
    const zero = scanNotesLinks("[[broken", { diagnostics: { maxRejections: 0 } });
    expect(zero.diagnostics).toEqual({
      candidateLimitReached: false,
      rejections: [],
      truncated: true,
    });
  });

  it("does not rescan deep stacks for unmatched cross-kind closers", () => {
    const pairCount = 10_000;
    const scan = scanNotesLinks("[[".repeat(pairCount) + "))".repeat(pairCount));

    expect(scan.occurrences).toEqual([]);
    expect(scan.diagnostics).toBeNull();
  });

  it("bounds the candidate stack on a 1.2MB adversarial line and resumes after newline", () => {
    const source = `${"[[".repeat(600_000)}\n[[Recovered]]`;
    const scan = scanNotesLinks(source, { diagnostics: { maxRejections: 32 } });

    expect(source.length).toBeGreaterThan(1_200_000);
    expect(scan.occurrences.map(({ target }) => target)).toEqual(["Recovered"]);
    expect(scan.diagnostics?.candidateLimitReached).toBe(true);
    expect(scan.diagnostics?.rejections.length).toBeLessThanOrEqual(32);
    expect(scan.diagnostics?.truncated).toBe(true);
    expect(MAX_OPEN_NOTES_LINK_CANDIDATES).toBeLessThan(2_048);
  });
});

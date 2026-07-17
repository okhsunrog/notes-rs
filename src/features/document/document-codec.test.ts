import { describe, expect, it } from "vite-plus/test";
import {
  DOCUMENT_CODEC_VERSION,
  DocumentCodec,
  encodeDocument,
  reconcileDocument,
  type DocumentBlockSnapshot,
} from "./document-codec";
import { ALL_BLOCK_STYLE_FIXTURES, fixtureBlock } from "./document-codec.fixtures";

const identities = (result: ReturnType<typeof reconcileDocument>) =>
  result.units.map((unit) => unit.previousUuid);

describe("DocumentCodec", () => {
  it("round-trips every BlockStyle variant and hidden task metadata", () => {
    const blocks = ALL_BLOCK_STYLE_FIXTURES.map(([style, markdown], index) =>
      fixtureBlock(`block-${index}`, style, markdown),
    );
    const encoded = encodeDocument(blocks);
    const decoded = reconcileDocument(encoded.markdown, encoded.sourceMap);

    expect(DocumentCodec.version).toBe(DOCUMENT_CODEC_VERSION);
    expect(decoded.units).toHaveLength(blocks.length);
    expect(
      decoded.units.map(({ previousUuid, style, markdown }) => ({ previousUuid, style, markdown })),
    ).toEqual(
      blocks.map((block) => ({
        previousUuid: block.uuid,
        style: block.style,
        markdown: block.markdown,
      })),
    );
    const cancelled = encoded.sourceMap.entries.find(
      (entry) => entry.style.kind === "task" && entry.style.state === "cancelled",
    );
    expect(cancelled).toBeDefined();
    expect(encoded.markdown.slice(cancelled!.sourceRange.from, cancelled!.sourceRange.to)).toBe(
      "- [x] Cancelled",
    );
  });

  it("projects and recovers nested mixed lists with semantic parents", () => {
    const blocks = [
      fixtureBlock("root", { kind: "bullet" }, "Root"),
      fixtureBlock("numbered", { kind: "numbered" }, "Numbered child", "root"),
      fixtureBlock("task", { kind: "task", state: "done" }, "Nested task", "numbered"),
      fixtureBlock("sibling", { kind: "bullet" }, "Sibling"),
    ];
    const encoded = encodeDocument(blocks);

    expect(encoded.markdown).toBe(
      "- Root\n\n  1. Numbered child\n\n     - [x] Nested task\n\n- Sibling",
    );
    const decoded = reconcileDocument(encoded.markdown, encoded.sourceMap);
    expect(decoded.units.map((unit) => unit.parentIndex)).toEqual([null, 0, 1, null]);
    expect(identities(decoded)).toEqual(["root", "numbered", "task", "sibling"]);

    const parsedWithoutMetadata = reconcileDocument(encoded.markdown, encodeDocument([]).sourceMap);
    expect(parsedWithoutMetadata.units.map((unit) => unit.style)).toEqual([
      { kind: "bullet" },
      { kind: "numbered" },
      { kind: "task", state: "done" },
      { kind: "bullet" },
    ]);
    expect(parsedWithoutMetadata.units.map((unit) => unit.parentIndex)).toEqual([null, 0, 1, null]);
    expect(parsedWithoutMetadata.units.map((unit) => unit.markdown)).toEqual([
      "Root",
      "Numbered child",
      "Nested task",
      "Sibling",
    ]);
  });

  it("round-trips multiline paragraph, quote, and fenced code without markers", () => {
    const blocks = [
      fixtureBlock("paragraph", { kind: "paragraph" }, "First line\nsecond line"),
      fixtureBlock("quote", { kind: "quote" }, "First quote\n\nSecond quote"),
      fixtureBlock("code", { kind: "code" }, "const fence = ```;\nconsole.log(fence);"),
    ];
    const encoded = encodeDocument(blocks);
    const decoded = reconcileDocument(encoded.markdown, encoded.sourceMap);

    expect(decoded.units.map((unit) => unit.markdown)).toEqual(
      blocks.map((block) => block.markdown),
    );
    expect(encoded.markdown).toContain("> First quote\n>\n> Second quote");
    expect(encoded.markdown).toContain("````\n");

    const parsed = reconcileDocument(encoded.markdown, encodeDocument([]).sourceMap);
    expect(parsed.units.map((unit) => unit.style)).toEqual([
      { kind: "paragraph" },
      { kind: "quote" },
      { kind: "code" },
    ]);
    expect(parsed.units.map((unit) => unit.markdown)).toEqual(
      blocks.map((block) => block.markdown),
    );
  });

  it("documents contiguous content ranges for multiline list and quote syntax", () => {
    const encoded = encodeDocument([
      fixtureBlock("list", { kind: "bullet" }, "first\nsecond"),
      fixtureBlock("quote", { kind: "quote" }, "quoted\nagain"),
    ]);
    const [list, quote] = encoded.sourceMap.entries;

    expect(encoded.markdown.slice(list.contentRange.from, list.contentRange.to)).toBe(
      "first\n  second",
    );
    expect(encoded.markdown.slice(quote.contentRange.from, quote.contentRange.to)).toBe(
      "quoted\n> again",
    );
    const parsed = reconcileDocument(encoded.markdown, encodeDocument([]).sourceMap);
    expect(parsed.units.map((unit) => unit.markdown)).toEqual(["first\nsecond", "quoted\nagain"]);
  });

  it("uses exact, ordered, non-overlapping half-open UTF-16 ranges", () => {
    const blocks = [
      fixtureBlock("unicode", { kind: "heading_1" }, "🧠 Привет"),
      fixtureBlock("next", { kind: "paragraph" }, "é and 漢字"),
    ];
    const encoded = encodeDocument(blocks);
    const [heading, next] = encoded.sourceMap.entries;

    expect(encoded.sourceMap.offsetEncoding).toBe("utf-16");
    expect(heading.contentRange.from).toBe(2);
    expect(heading.contentRange.to - heading.contentRange.from).toBe("🧠 Привет".length);
    expect(heading.contentRange.to).toBeLessThanOrEqual(next.sourceRange.from);
    expect(encoded.markdown.slice(heading.sourceRange.from, heading.sourceRange.to)).toBe(
      "# 🧠 Привет",
    );
    expect(encoded.markdown.slice(next.contentRange.from, next.contentRange.to)).toBe("é and 漢字");
    expect("🧠".length).toBe(2);
    expect(new TextEncoder().encode("🧠").length).toBe(4);
  });

  it("reconciles insertion, deletion, split, and merge deterministically", () => {
    const original = encodeDocument([
      fixtureBlock("alpha", { kind: "paragraph" }, "Alpha text"),
      fixtureBlock("beta", { kind: "paragraph" }, "Beta text"),
      fixtureBlock("gamma", { kind: "paragraph" }, "Gamma text"),
    ]);

    const inserted = reconcileDocument(
      "Alpha text\n\nInserted\n\nBeta text\n\nGamma text",
      original.sourceMap,
    );
    expect(identities(inserted)).toEqual(["alpha", null, "beta", "gamma"]);

    const deleted = reconcileDocument("Alpha text\n\nGamma text", original.sourceMap);
    expect(identities(deleted)).toEqual(["alpha", "gamma"]);

    const split = reconcileDocument("Alpha\n\ntext\n\nBeta text\n\nGamma text", original.sourceMap);
    expect(split.units.filter((unit) => unit.previousUuid === "alpha")).toHaveLength(1);
    expect(split.units.slice(0, 2).some((unit) => unit.previousUuid === null)).toBe(true);

    const merged = reconcileDocument("Alpha text Beta text\n\nGamma text", original.sourceMap);
    expect(merged.units).toHaveLength(2);
    expect(merged.units[0].previousUuid).toBe("alpha");
    expect(merged.units[1].previousUuid).toBe("gamma");
  });

  it("keeps identities with exact semantic units across reorder", () => {
    const original = encodeDocument([
      fixtureBlock("alpha", { kind: "paragraph" }, "Alpha"),
      fixtureBlock("beta", { kind: "quote" }, "Beta"),
      fixtureBlock("gamma", { kind: "heading_2" }, "Gamma"),
    ]);
    const reordered = reconcileDocument("## Gamma\n\nAlpha\n\n> Beta", original.sourceMap);

    expect(identities(reordered)).toEqual(["gamma", "alpha", "beta"]);
  });

  it("does not let duplicate semantic content steal a unique UUID", () => {
    const original = encodeDocument([
      fixtureBlock("first", { kind: "paragraph" }, "Duplicate"),
      fixtureBlock("second", { kind: "paragraph" }, "Duplicate"),
      fixtureBlock("unique", { kind: "paragraph" }, "Unique"),
    ]);
    const edited = "Duplicate changed\n\nDuplicate\n\nUnique";

    const first = reconcileDocument(edited, original.sourceMap);
    const second = reconcileDocument(edited, original.sourceMap);
    expect(first).toEqual(second);
    expect(first.units.filter((unit) => unit.previousUuid === "unique")).toHaveLength(1);
    expect(new Set(first.units.map((unit) => unit.previousUuid).filter(Boolean)).size).toBe(
      first.units.filter((unit) => unit.previousUuid !== null).length,
    );
  });

  it("uses explicit syntax changes while preserving mapped hidden task state", () => {
    const original = encodeDocument([
      fixtureBlock("paragraph", { kind: "paragraph" }, "Plain"),
      fixtureBlock("doing", { kind: "task", state: "doing" }, "Work"),
    ]);
    const styled = reconcileDocument("# Plain\n\n- [ ] Work changed", original.sourceMap);

    expect(styled.units[0].style).toEqual({ kind: "heading_1" });
    expect(styled.units[1].style).toEqual({ kind: "task", state: "doing" });
    const completed = reconcileDocument("Plain\n\n- [x] Work", original.sourceMap);
    expect(completed.units[1].style).toEqual({ kind: "task", state: "done" });

    const cancelled = encodeDocument([
      fixtureBlock("cancelled", { kind: "task", state: "cancelled" }, "Cancelled task"),
    ]);
    const reopened = reconcileDocument("- [ ] Cancelled task", cancelled.sourceMap);
    expect(reopened.units[0].style).toEqual({ kind: "task", state: "todo" });
  });

  it("preserves non-list parent metadata while edited segments reconcile", () => {
    const original = encodeDocument([
      fixtureBlock("parent", { kind: "paragraph" }, "Parent"),
      fixtureBlock("child", { kind: "paragraph" }, "Child", "parent"),
    ]);
    const edited = reconcileDocument("Parent\n\nChild edited", original.sourceMap);

    expect(identities(edited)).toEqual(["parent", "child"]);
    expect(edited.units.map((unit) => unit.parentIndex)).toEqual([null, 0]);
  });

  it("decodes newly inserted checkboxes only as Todo or Done", () => {
    const decoded = reconcileDocument(
      "- [ ] New todo\n\n- [X] New done",
      encodeDocument([]).sourceMap,
    );

    expect(decoded.units.map((unit) => unit.previousUuid)).toEqual([null, null]);
    expect(decoded.units.map((unit) => unit.style)).toEqual([
      { kind: "task", state: "todo" },
      { kind: "task", state: "done" },
    ]);
    expect(decoded.units.map((unit) => unit.markdown)).toEqual(["New todo", "New done"]);
  });

  it("recovers an unclosed fence as a code semantic unit", () => {
    const original = encodeDocument([fixtureBlock("code", { kind: "code" }, "old")]);
    const decoded = reconcileDocument("```ts\nconst value = 1;\n", original.sourceMap);

    expect(decoded.units).toHaveLength(1);
    expect(decoded.units[0].style).toEqual({ kind: "code" });
    expect(decoded.units[0].markdown).toContain("const value = 1;");
  });

  it("is deterministic, immutable, and never embeds identity markers", () => {
    const blocks: DocumentBlockSnapshot[] = [
      fixtureBlock("018f-secret-uuid", { kind: "paragraph" }, "Text"),
    ];
    const first = encodeDocument(blocks);
    const second = encodeDocument(blocks);

    expect(first).toEqual(second);
    expect(first.markdown).toBe("Text");
    expect(first.markdown).not.toMatch(/uuid|<!--|data-/i);
    expect(Object.isFrozen(first.sourceMap)).toBe(true);
    expect(Object.isFrozen(first.sourceMap.entries)).toBe(true);
    expect(Object.isFrozen(first.sourceMap.entries[0].sourceRange)).toBe(true);
  });
});

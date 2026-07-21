import { describe, expect, it } from "vite-plus/test";
import type { Block, BlockStyle } from "@/lib/api";
import { journalOutlineRows, journalPreviewLimitForHeight } from "./journal-outline-preview";

function block(
  uuid: string,
  markdown: string,
  parentUuid: string | null = null,
  style: BlockStyle = { kind: "bullet" },
): Block {
  return {
    uuid,
    pageUuid: "page",
    parentUuid,
    orderKey: "a",
    style,
    markdown,
    markdownRevision: "1:0:test",
    createdAt: 1,
    updatedAt: 1,
  };
}

describe("journal outline preview", () => {
  it("keeps document preorder, derives nesting depth, and omits empty editor blocks", () => {
    expect(
      journalOutlineRows([
        block("root", "Plan"),
        block("empty", "", "root"),
        block("child", "Ship it", "root"),
        block("grandchild", "Verify", "child"),
        block("divider", "", null, { kind: "divider" }),
      ]).map(({ block, depth }) => [block.uuid, depth]),
    ).toEqual([
      ["root", 0],
      ["child", 1],
      ["grandchild", 2],
      ["divider", 0],
    ]);
  });

  it("uses a smaller preview on short screens", () => {
    expect(journalPreviewLimitForHeight(700)).toBe(3);
    expect(journalPreviewLimitForHeight(800)).toBe(4);
    expect(journalPreviewLimitForHeight(1_000)).toBe(6);
  });
});

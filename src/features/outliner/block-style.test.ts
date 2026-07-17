import { describe, expect, it } from "vite-plus/test";
import type { Block, BlockStyle } from "@/lib/api";
import {
  BLOCK_STYLE_OPTIONS,
  getBlockStyleOption,
  isBlockStyle,
  replaceCachedBlock,
} from "./block-style";

const ALL_STYLES: BlockStyle[] = [
  "paragraph",
  "bullet",
  "numbered",
  "task",
  "heading_1",
  "heading_2",
  "heading_3",
  "quote",
  "code",
  "divider",
];

function block(uuid: string, style: BlockStyle = "paragraph"): Block {
  return {
    uuid,
    pageUuid: "page-a",
    parentUuid: null,
    orderKey: `order-${uuid}`,
    style,
    markdown: uuid,
    createdAt: 0,
    updatedAt: 0,
  };
}

describe("block style catalogue", () => {
  it("maps every generated style exactly once to a human label and icon", () => {
    expect(BLOCK_STYLE_OPTIONS.map(({ value }) => value)).toEqual(ALL_STYLES);
    expect(new Set(BLOCK_STYLE_OPTIONS.map(({ value }) => value)).size).toBe(ALL_STYLES.length);

    for (const style of ALL_STYLES) {
      expect(isBlockStyle(style)).toBe(true);
      expect(getBlockStyleOption(style).label.length).toBeGreaterThan(0);
      expect(getBlockStyleOption(style).icon.length).toBeGreaterThan(0);
    }
    expect(isBlockStyle("heading_4")).toBe(false);
  });

  it("patches only the matching cached block and preserves sibling order", () => {
    const first = block("first");
    const second = block("second");
    const updated = { ...second, style: "quote" as const, updatedAt: 1 };

    const result = replaceCachedBlock([first, second], updated);

    expect(result).toEqual([first, updated]);
    expect(result[0]).toBe(first);
    expect(result[1]).toBe(updated);
  });
});

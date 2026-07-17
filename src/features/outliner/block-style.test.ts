import { describe, expect, it } from "vite-plus/test";
import type { Block, BlockStyle, TaskState } from "@/lib/api";
import {
  BLOCK_STYLE_OPTIONS,
  TASK_STATE_OPTIONS,
  blockStyleForKind,
  blockStylesEqual,
  getBlockStyleOption,
  isBlockStyleKind,
  replaceCachedBlock,
  toggledTaskState,
  type BlockStyleKind,
} from "./block-style";

const ALL_STYLE_KINDS: BlockStyleKind[] = [
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

const ALL_TASK_STATES: TaskState[] = [
  "todo",
  "doing",
  "now",
  "later",
  "done",
  "waiting",
  "cancelled",
];

function block(uuid: string, style: BlockStyle = { kind: "paragraph" }): Block {
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
  it("maps every generated style kind exactly once to a human label and icon", () => {
    expect(BLOCK_STYLE_OPTIONS.map(({ value }) => value)).toEqual(ALL_STYLE_KINDS);
    expect(new Set(BLOCK_STYLE_OPTIONS.map(({ value }) => value)).size).toBe(
      ALL_STYLE_KINDS.length,
    );

    for (const kind of ALL_STYLE_KINDS) {
      expect(isBlockStyleKind(kind)).toBe(true);
      const option = getBlockStyleOption(blockStyleForKind(kind));
      expect(option.label.length).toBeGreaterThan(0);
      expect(option.icon.length).toBeGreaterThan(0);
    }
    expect(isBlockStyleKind("heading_4")).toBe(false);
  });

  it("uses an explicit default state and preserves an existing task state", () => {
    expect(blockStyleForKind("task")).toEqual({ kind: "task", state: "todo" });
    expect(blockStyleForKind("task", { kind: "task", state: "waiting" })).toEqual({
      kind: "task",
      state: "waiting",
    });
    expect(
      blockStylesEqual({ kind: "task", state: "todo" }, { kind: "task", state: "doing" }),
    ).toBe(false);
  });

  it("maps every typed task state and mirrors backend checkbox behavior", () => {
    expect(TASK_STATE_OPTIONS.map(({ value }) => value)).toEqual(ALL_TASK_STATES);
    for (const state of ["todo", "doing", "now", "later", "waiting"] as const) {
      expect(toggledTaskState(state)).toBe("done");
    }
    expect(toggledTaskState("done")).toBe("todo");
    expect(toggledTaskState("cancelled")).toBe("todo");
  });

  it("patches only the matching cached block and preserves sibling order", () => {
    const first = block("first");
    const second = block("second");
    const updated = { ...second, style: { kind: "quote" } as const, updatedAt: 1 };

    const result = replaceCachedBlock([first, second], updated);

    expect(result).toEqual([first, updated]);
    expect(result[0]).toBe(first);
    expect(result[1]).toBe(updated);
  });
});

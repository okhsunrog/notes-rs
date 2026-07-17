import { describe, expect, it } from "vite-plus/test";
import { detectTrigger } from "./autocomplete";
import { nextSibling, prevSibling } from "./keyboard";
import type { Block } from "@/lib/api";

function block(uuid: string, orderKey: string): Block {
  return {
    uuid,
    pageUuid: "page-a",
    parentUuid: null,
    orderKey,
    style: { kind: "paragraph" },
    markdown: "",
    markdownRevision: `revision-${uuid}`,
    createdAt: 0,
    updatedAt: 0,
  };
}

describe("outliner utilities", () => {
  it("detects only the rightmost unclosed autocomplete trigger", () => {
    expect(detectTrigger("closed [[Page]] then ((block", 27)).toEqual({
      kind: "((",
      start: 21,
      end: 27,
      query: "bloc",
    });
    expect(detectTrigger("[[closed]]", 10)).toBeNull();
  });

  it("navigates siblings by stable UUID", () => {
    const siblings = [
      block("block-a", "0000000100000000"),
      block("block-b", "0000000200000000"),
      block("block-c", "0000000300000000"),
    ];
    expect(prevSibling(siblings, "block-b")?.uuid).toBe("block-a");
    expect(nextSibling(siblings, "block-b")?.uuid).toBe("block-c");
    expect(prevSibling(siblings, "block-a")).toBeNull();
    expect(nextSibling(siblings, "block-c")).toBeNull();
  });
});

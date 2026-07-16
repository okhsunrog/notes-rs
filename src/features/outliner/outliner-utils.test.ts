import { describe, expect, it } from "vite-plus/test";
import { detectTrigger } from "./autocomplete";
import { nextSibling, positionAfter, prevSibling } from "./keyboard";
import { parseRefs } from "./parse-refs";
import type { Node } from "@/lib/api";

function node(id: number, position: number): Node {
  return {
    id,
    uuid: `node-${id}`,
    kind: "block",
    title: null,
    content: "",
    content_json: "{}",
    parent_id: 1,
    position,
    created_at: 0,
    updated_at: 0,
  };
}

describe("outliner utilities", () => {
  it("extracts unique wikilinks and block references", () => {
    expect(parseRefs("[[Roadmap]] and [[ Roadmap ]] with ((abc-123))")).toEqual({
      wikilinks: ["Roadmap"],
      blockRefs: ["abc-123"],
    });
  });

  it("detects only the rightmost unclosed autocomplete trigger", () => {
    expect(detectTrigger("closed [[Page]] then ((block", 27)).toEqual({
      kind: "((",
      start: 21,
      end: 27,
      query: "bloc",
    });
    expect(detectTrigger("[[closed]]", 10)).toBeNull();
  });

  it("calculates insertion positions and sibling navigation", () => {
    const siblings = [node(1, 1), node(2, 3), node(3, 4)];
    expect(positionAfter(siblings, 1)).toBe(2);
    expect(positionAfter(siblings, 3)).toBe(5);
    expect(prevSibling(siblings, 2)?.id).toBe(1);
    expect(nextSibling(siblings, 2)?.id).toBe(3);
  });
});

import { describe, expect, it } from "vite-plus/test";
import { parseLogseqVideoMacro } from "./remark-logseq-video";

describe("parseLogseqVideoMacro", () => {
  it("produces a custom semantic boundary only for checked web URLs", () => {
    const node = parseLogseqVideoMacro("{{video https://video.example/watch?q=private}}");

    expect(node.type).toBe("logseqVideo");
    expect(node.data).toEqual({
      hName: "notes-video",
      hProperties: { href: "https://video.example/watch?q=private" },
    });
  });

  it("preserves rejected URLs as recoverable source", () => {
    expect(parseLogseqVideoMacro("{{video data:text/html,unsafe}}")).toEqual({
      type: "inlineCode",
      value: "{{video data:text/html,unsafe}}",
    });
    expect(parseLogseqVideoMacro("{{video [Watch](https://video.example/has space)}}")).toEqual({
      type: "inlineCode",
      value: "{{video [Watch](https://video.example/has space)}}",
    });
  });
});

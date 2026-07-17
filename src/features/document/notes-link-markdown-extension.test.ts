import { parser } from "@lezer/markdown";
import { describe, expect, it } from "vite-plus/test";
import {
  MAX_NOTES_LINK_CONTENT_LENGTH,
  NOTES_LINK_CONTENT_NODE_NAMES,
  NOTES_LINK_MARK_NODE_NAMES,
  NOTES_LINK_NODE_NAMES,
  isNotesLinkNodeName,
  notesLinkMarkdownExtension,
  notesLinkNodeRole,
} from "./notes-link-markdown-extension";

const notesParser = parser.configure(notesLinkMarkdownExtension);
const UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";

interface ParsedNode {
  from: number;
  name: string;
  text: string;
  to: number;
}

function parseNodes(source: string): ParsedNode[] {
  const result: ParsedNode[] = [];
  notesParser.parse(source).iterate({
    enter(node) {
      if (!isNotesLinkNodeName(node.name)) return;
      result.push({
        from: node.from,
        name: node.name,
        text: source.slice(node.from, node.to),
        to: node.to,
      });
    },
  });
  return result;
}

function links(source: string): ParsedNode[] {
  return parseNodes(source).filter((node) => notesLinkNodeRole(node.name) === "link");
}

function allNodeNames(source: string): string[] {
  const names: string[] = [];
  notesParser.parse(source).iterate({ enter: (node) => void names.push(node.name) });
  return names;
}

describe("notesLinkMarkdownExtension", () => {
  it("parses a page link with distinct source marks and lossless content", () => {
    expect(parseNodes("Before [[Project Aurora]] after")).toEqual([
      { from: 7, name: NOTES_LINK_NODE_NAMES.pageLink, text: "[[Project Aurora]]", to: 25 },
      { from: 7, name: NOTES_LINK_NODE_NAMES.pageMark, text: "[[", to: 9 },
      { from: 9, name: NOTES_LINK_NODE_NAMES.pageContent, text: "Project Aurora", to: 23 },
      { from: 23, name: NOTES_LINK_NODE_NAMES.pageMark, text: "]]", to: 25 },
    ]);
  });

  it("keeps renderer-compatible Page|Alias text as one literal target", () => {
    const nodes = parseNodes("[[Project Aurora|Roadmap]]");
    const content = nodes.find((node) => node.name === NOTES_LINK_NODE_NAMES.pageContent);

    expect(content?.text).toBe("Project Aurora|Roadmap");
    expect(
      nodes.filter((node) => NOTES_LINK_CONTENT_NODE_NAMES.includes(node.name as never)),
    ).toHaveLength(1);
    expect(
      nodes.filter((node) => NOTES_LINK_MARK_NODE_NAMES.includes(node.name as never)),
    ).toHaveLength(2);
  });

  it("preserves Unicode and surrounding spaces inside page content", () => {
    const nodes = parseNodes("[[  Архитектура / 日本語  ]]");
    expect(nodes.find((node) => node.name === NOTES_LINK_NODE_NAMES.pageContent)?.text).toBe(
      "  Архитектура / 日本語  ",
    );
  });

  it("parses canonical UUID spelling case-insensitively with separate marks", () => {
    const upper = UUID.toUpperCase();
    expect(parseNodes(`((${upper}))`)).toEqual([
      { from: 0, name: NOTES_LINK_NODE_NAMES.blockLink, text: `((${upper}))`, to: 40 },
      { from: 0, name: NOTES_LINK_NODE_NAMES.blockMark, text: "((", to: 2 },
      { from: 2, name: NOTES_LINK_NODE_NAMES.blockContent, text: upper, to: 38 },
      { from: 38, name: NOTES_LINK_NODE_NAMES.blockMark, text: "))", to: 40 },
    ]);
  });

  it.each([
    "((not-a-uuid))",
    "((019c8d1a4ab17f318f00f594337c3ca5))",
    "((019c8d1a-4ab1-7f31-8f00-f594337c3cag))",
    "((019c8d1a-4ab1-7f31-8f00-f594337c3ca5)",
  ])("leaves invalid block reference source ordinary: %s", (source) => {
    expect(parseNodes(source)).toEqual([]);
  });

  it.each(["[[unclosed", "[[   ]]", "[[\u00a0\u2003]]", "[[line\nbreak]]", "[[has]bracket]]"])(
    "leaves invalid page source ordinary: %s",
    (source) => {
      expect(parseNodes(source)).toEqual([]);
    },
  );

  it("does not partially recognize nested page or block constructs", () => {
    expect(parseNodes("[[outer [[inner]] tail]]")).toEqual([]);
    expect(parseNodes(`((outer ((${UUID})) tail))`)).toEqual([]);
  });

  it("does not recognize an escaped opener", () => {
    expect(parseNodes(String.raw`\[[Escaped]] and \((${UUID}))`)).toEqual([]);
  });

  it("bounds oversized constructs without swallowing their source", () => {
    const source = `[[${"x".repeat(MAX_NOTES_LINK_CONTENT_LENGTH + 1)}]]`;
    expect(parseNodes(source)).toEqual([]);
  });

  it("accepts content exactly at the renderer-compatible maximum", () => {
    const content = "x".repeat(MAX_NOTES_LINK_CONTENT_LENGTH);
    expect(links(`[[${content}]]`).map((node) => node.text)).toEqual([`[[${content}]]`]);
  });

  it("parses adjacent valid constructs independently", () => {
    const source = `[[One]][[Two]]((${UUID}))`;
    expect(links(source).map((node) => node.text)).toEqual(["[[One]]", "[[Two]]", `((${UUID}))`]);
  });

  it("coexists with normal Markdown links and stays inert inside their labels, URLs, and code", () => {
    const source = `[[Outside]] [label [[Inside]]](<https://example.test/[[url]]>) \`[[inline]]\`\n\n\`\`\`md\n[[fenced]]\n\`\`\``;
    const nodeNames = allNodeNames(source);

    expect(links(source).map((node) => node.text)).toEqual(["[[Outside]]"]);
    expect(nodeNames).toContain("Link");
    expect(nodeNames).toContain("InlineCode");
    expect(nodeNames).toContain("FencedCode");
  });

  it("exports typed node-role helpers for Live Preview allowlists", () => {
    expect(notesLinkNodeRole(NOTES_LINK_NODE_NAMES.pageLink)).toBe("link");
    expect(notesLinkNodeRole(NOTES_LINK_NODE_NAMES.blockMark)).toBe("mark");
    expect(notesLinkNodeRole(NOTES_LINK_NODE_NAMES.pageContent)).toBe("content");
    expect(notesLinkNodeRole("Paragraph")).toBeNull();
  });
});

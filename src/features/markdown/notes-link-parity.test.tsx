import { parser } from "@lezer/markdown";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import {
  NOTES_LINK_NODE_NAMES,
  notesLinkMarkdownExtension,
} from "@/features/document/notes-link-markdown-extension";
import { MAX_NOTES_LINK_CONTENT_LENGTH, NotesLinkKind } from "./notes-link-scanner";
import { MarkdownRenderer } from "./markdown-renderer";

const UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";
const notesParser = parser.configure(notesLinkMarkdownExtension);
const CONTEXT = {
  kind: "note",
  presentation: "reading",
  pageUuid: "page-a",
} as const;

interface ParityFixture {
  expected: readonly NotesLinkKind[];
  name: string;
  source: string;
}

const fixtures: ParityFixture[] = [
  {
    name: "odd-backslash escaped page and block",
    source: String.raw`\[[Escaped]] and \((${UUID}))`,
    expected: [],
  },
  {
    name: "valid link after escaped candidate in one text node",
    source: String.raw`\[[Escaped]] then [[Valid]]`,
    expected: [NotesLinkKind.Page],
  },
  {
    name: "even backslashes retain link semantics",
    source: String.raw`\\[[Even]] and \\((${UUID.toUpperCase()}))`,
    expected: [NotesLinkKind.Page, NotesLinkKind.Block],
  },
  {
    name: "same-kind nesting",
    source: "[[outer [[inner]] tail]]",
    expected: [],
  },
  {
    name: "cross-kind nesting",
    source: `[[outer ((${UUID})) tail]]`,
    expected: [],
  },
  {
    name: "broken outer recovers inner",
    source: "[[broken and [[valid]]",
    expected: [NotesLinkKind.Page],
  },
  {
    name: "Unicode-empty page",
    source: "[[\u00a0\u2003]]",
    expected: [],
  },
  {
    name: "exact maximum page target",
    source: `[[${"x".repeat(MAX_NOTES_LINK_CONTENT_LENGTH)}]]`,
    expected: [NotesLinkKind.Page],
  },
  {
    name: "oversized page target",
    source: `[[${"x".repeat(MAX_NOTES_LINK_CONTENT_LENGTH + 1)}]]`,
    expected: [],
  },
  {
    name: "adjacent page and block links",
    // Whitespace avoids CommonMark's unrelated `[label](destination)` interpretation at the
    // boundary between a closing page link and an opening block reference.
    source: `[[One]] [[Two]] ((${UUID}))`,
    expected: [NotesLinkKind.Page, NotesLinkKind.Page, NotesLinkKind.Block],
  },
  {
    name: "code and standard links are opaque",
    source: `[[Outside]] [label [[Inside]]](https://example.com) \`[[Code]]\` [block ((${UUID}))](https://example.com/block)`,
    expected: [NotesLinkKind.Page],
  },
  {
    name: "multiple candidates in an inline link label are opaque",
    source: "[label [[One]] and [[Two]]](https://example.com)",
    expected: [],
  },
  {
    name: "multiple candidates in image alt text are opaque",
    source: "![alt [[One]] and [[Two]]](image.png)",
    expected: [],
  },
  {
    name: "multiple candidates in a reference link label are opaque",
    source: "[label [[One]] and [[Two]]][id]\n\n[id]: https://example.com",
    expected: [],
  },
  {
    name: "a closed ordinary link does not hide a following notes link",
    source: "[ordinary](https://example.com) then [[Visible]]",
    expected: [NotesLinkKind.Page],
  },
  {
    name: "emphasis source inside a page target is inert",
    source: "[[a **bold** and *emphasis*]]",
    expected: [],
  },
  {
    name: "inline code source inside a page target is inert",
    source: "[[a `code` c]]",
    expected: [],
  },
  {
    name: "math source inside a page target is inert",
    source: "[[a $x$ c]]",
    expected: [],
  },
  {
    name: "GFM URL autolinks inside page targets are inert",
    source: "[[https://example.com]] [[www.example.com]]",
    expected: [],
  },
  {
    name: "GFM email autolinks inside page targets are inert",
    source: "[[user@example.com]]",
    expected: [],
  },
  {
    name: "Logseq video macro source inside a page target is inert",
    source: "[[a {{video https://video.example/x}} c]]",
    expected: [],
  },
  {
    name: "escaped emphasis punctuation remains literal target content",
    source: String.raw`[[a \* b]]`,
    expected: [NotesLinkKind.Page],
  },
  {
    name: "decoded entity remains valid target content",
    source: "[[a &amp; b]]",
    expected: [NotesLinkKind.Page],
  },
  {
    name: "Page|Alias remains a literal target",
    source: "[[Project Aurora|Roadmap]]",
    expected: [NotesLinkKind.Page],
  },
];

function readingOccurrences(source: string): NotesLinkKind[] {
  const html = renderToStaticMarkup(
    <MarkdownRenderer context={CONTEXT} markdown={source} onOpenLink={() => undefined} />,
  );
  return [...html.matchAll(/data-markdown-link="(page|block)"/g)].map((match) =>
    match[1] === "page" ? NotesLinkKind.Page : NotesLinkKind.Block,
  );
}

function lezerOccurrences(source: string): NotesLinkKind[] {
  const occurrences: Array<{ from: number; kind: NotesLinkKind }> = [];
  notesParser.parse(source).iterate({
    enter(node) {
      if (node.name === NOTES_LINK_NODE_NAMES.pageLink) {
        occurrences.push({ from: node.from, kind: NotesLinkKind.Page });
      } else if (node.name === NOTES_LINK_NODE_NAMES.blockLink) {
        occurrences.push({ from: node.from, kind: NotesLinkKind.Block });
      }
    },
  });
  return occurrences.sort((left, right) => left.from - right.from).map(({ kind }) => kind);
}

describe("notes-link Reading/Lezer parity", () => {
  it.each(fixtures)("matches shared policy for $name", ({ expected, source }) => {
    expect(readingOccurrences(source)).toEqual(expected);
    expect(lezerOccurrences(source)).toEqual(expected);
  });
});

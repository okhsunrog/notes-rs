import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vite-plus/test";
import {
  type DocumentLinkClick,
  resolveDocumentLink,
  resolveDocumentLinkClickDisposition,
  resolveDocumentLinkOpenDisposition,
} from "./document-link-navigation";
import { notesLinkMarkdownExtension } from "./notes-link-markdown-extension";

const click = (overrides: Partial<DocumentLinkClick> = {}): DocumentLinkClick => ({
  button: 0,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  ...overrides,
});

function stateFor(doc: string, selection?: { anchor: number; head?: number }): EditorState {
  return EditorState.create({
    doc,
    selection,
    extensions: [markdown({ base: markdownLanguage, extensions: notesLinkMarkdownExtension })],
  });
}

describe("resolveDocumentLinkOpenDisposition", () => {
  it("opens a decorated Live Preview link on a plain click, Shift for adjacent", () => {
    expect(resolveDocumentLinkOpenDisposition(click(), "live_preview", true)).toBe("current");
    expect(
      resolveDocumentLinkOpenDisposition(click({ shiftKey: true }), "live_preview", true),
    ).toBe("adjacent");
  });

  it("leaves a plain click as caret placement once source is revealed", () => {
    expect(resolveDocumentLinkOpenDisposition(click(), "live_preview", false)).toBeNull();
    expect(
      resolveDocumentLinkOpenDisposition(click({ shiftKey: true }), "live_preview", false),
    ).toBeNull();
  });

  it("requires Mod-click for revealed source or Source mode", () => {
    expect(
      resolveDocumentLinkOpenDisposition(click({ ctrlKey: true }), "live_preview", false),
    ).toBe("current");
    expect(resolveDocumentLinkOpenDisposition(click({ metaKey: true }), "source", true)).toBe(
      "current",
    );
    expect(resolveDocumentLinkOpenDisposition(click(), "source", true)).toBeNull();
  });

  it("opens adjacent on Mod-Shift-click", () => {
    expect(
      resolveDocumentLinkOpenDisposition(
        click({ ctrlKey: true, shiftKey: true }),
        "live_preview",
        false,
      ),
    ).toBe("adjacent");
  });

  it("ignores non-primary buttons even when decorated or modified", () => {
    expect(
      resolveDocumentLinkOpenDisposition(click({ button: 2 }), "live_preview", true),
    ).toBeNull();
    expect(
      resolveDocumentLinkOpenDisposition(
        click({ ctrlKey: true, button: 2 }),
        "live_preview",
        false,
      ),
    ).toBeNull();
  });
});

describe("resolveDocumentLink", () => {
  it("resolves a tangleaf page link from any position inside it, with its full source range", () => {
    const source = "See [[Project Aurora]] today";
    const state = stateFor(source);
    const from = source.indexOf("[[Project");
    const to = source.indexOf("]]") + 2;
    for (let pos = from + 1; pos < to - 1; pos += 3) {
      expect(resolveDocumentLink(state, pos)).toEqual({
        target: { kind: "page", title: "Project Aurora" },
        range: { from, to },
      });
    }
  });

  it("resolves a tangleaf block link", () => {
    const uuid = "0f5a1c2e-1234-4bcd-8abc-1234567890ab";
    const source = `See ((${uuid})) today`;
    const state = stateFor(source);
    const pos = source.indexOf(uuid) + 3;
    expect(resolveDocumentLink(state, pos)?.target).toEqual({ kind: "block", uuid });
  });

  it("resolves an ordinary inline Markdown link", () => {
    const source = "Read [the docs](https://example.com/guide) now";
    const state = stateFor(source);
    const pos = source.indexOf("the docs") + 2;
    expect(resolveDocumentLink(state, pos)?.target).toEqual({
      kind: "external",
      href: "https://example.com/guide",
      protocol: "https",
    });
  });

  it("resolves a bare autolink", () => {
    const source = "See <https://example.com> for details";
    const state = stateFor(source);
    const pos = source.indexOf("https://example.com") + 4;
    expect(resolveDocumentLink(state, pos)?.target).toEqual({
      kind: "external",
      href: "https://example.com/",
      protocol: "https",
    });
  });

  it("returns null for plain text and unsupported protocols", () => {
    const source = "Just plain text with [ftp link](ftp://example.com/file)";
    const state = stateFor(source);
    expect(resolveDocumentLink(state, 5)).toBeNull();
    const pos = source.indexOf("ftp link") + 2;
    expect(resolveDocumentLink(state, pos)).toBeNull();
  });
});

describe("resolveDocumentLinkClickDisposition", () => {
  // Two lines: a single-line source is always "revealed" because its one line always holds the
  // caret. The link lives on line 2 so a caret on line 1 leaves it decorated.
  const source = "plain\nSee [[Project Aurora]] today";
  const linkPos = source.indexOf("Aurora");
  const otherLinePos = source.indexOf("plain");

  it("navigates a decorated link on a plain click when the caret is elsewhere", () => {
    const state = stateFor(source, { anchor: otherLinePos });
    const resolved = resolveDocumentLinkClickDisposition(state, linkPos, click(), "live_preview");
    expect(resolved?.disposition).toBe("current");
    expect(resolved?.target).toEqual({ kind: "page", title: "Project Aurora" });
  });

  it("leaves a plain click as caret placement once the caret is already on the link's line", () => {
    const state = stateFor(source, { anchor: linkPos });
    expect(resolveDocumentLinkClickDisposition(state, linkPos, click(), "live_preview")).toBeNull();
  });

  it("still navigates from revealed source with Mod-click", () => {
    const state = stateFor(source, { anchor: linkPos });
    const resolved = resolveDocumentLinkClickDisposition(
      state,
      linkPos,
      click({ ctrlKey: true }),
      "live_preview",
    );
    expect(resolved?.disposition).toBe("current");
  });

  it("never navigates on a plain click in Source mode", () => {
    const state = stateFor(source, { anchor: otherLinePos });
    expect(resolveDocumentLinkClickDisposition(state, linkPos, click(), "source")).toBeNull();
    expect(
      resolveDocumentLinkClickDisposition(state, linkPos, click({ ctrlKey: true }), "source")
        ?.disposition,
    ).toBe("current");
  });

  it("returns null where there is no link regardless of mode or modifiers", () => {
    const state = stateFor(source, { anchor: otherLinePos });
    expect(
      resolveDocumentLinkClickDisposition(state, 1, click({ ctrlKey: true }), "live_preview"),
    ).toBeNull();
  });
});

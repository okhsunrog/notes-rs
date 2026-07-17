import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vite-plus/test";
import {
  type DocumentLinkClick,
  resolveDocumentLinkOpenDisposition,
  resolveDocumentLinkTarget,
} from "./document-link-navigation";
import { notesLinkMarkdownExtension } from "./notes-link-markdown-extension";

const click = (overrides: Partial<DocumentLinkClick> = {}): DocumentLinkClick => ({
  button: 0,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  ...overrides,
});

function stateFor(doc: string): EditorState {
  return EditorState.create({
    doc,
    extensions: [markdown({ base: markdownLanguage, extensions: notesLinkMarkdownExtension })],
  });
}

describe("resolveDocumentLinkOpenDisposition", () => {
  it("leaves a plain click as ordinary caret placement", () => {
    expect(resolveDocumentLinkOpenDisposition(click())).toBeNull();
    expect(resolveDocumentLinkOpenDisposition(click({ shiftKey: true }))).toBeNull();
  });

  it("requires Mod-click to open the current pane", () => {
    expect(resolveDocumentLinkOpenDisposition(click({ ctrlKey: true }))).toBe("current");
    expect(resolveDocumentLinkOpenDisposition(click({ metaKey: true }))).toBe("current");
  });

  it("opens adjacent on Mod-Shift-click", () => {
    expect(resolveDocumentLinkOpenDisposition(click({ ctrlKey: true, shiftKey: true }))).toBe(
      "adjacent",
    );
  });

  it("ignores non-primary buttons even with modifiers held", () => {
    expect(resolveDocumentLinkOpenDisposition(click({ ctrlKey: true, button: 2 }))).toBeNull();
  });
});

describe("resolveDocumentLinkTarget", () => {
  it("resolves a notes-rs page link from any position inside it", () => {
    const source = "See [[Project Aurora]] today";
    const state = stateFor(source);
    const from = source.indexOf("[[Project");
    const to = source.indexOf("]]") + 2;
    for (let pos = from + 1; pos < to - 1; pos += 3) {
      expect(resolveDocumentLinkTarget(state, pos)).toEqual({
        kind: "page",
        title: "Project Aurora",
      });
    }
  });

  it("resolves a notes-rs block link", () => {
    const uuid = "0f5a1c2e-1234-4bcd-8abc-1234567890ab";
    const source = `See ((${uuid})) today`;
    const state = stateFor(source);
    const pos = source.indexOf(uuid) + 3;
    expect(resolveDocumentLinkTarget(state, pos)).toEqual({ kind: "block", uuid });
  });

  it("resolves an ordinary inline Markdown link", () => {
    const source = "Read [the docs](https://example.com/guide) now";
    const state = stateFor(source);
    const pos = source.indexOf("the docs") + 2;
    expect(resolveDocumentLinkTarget(state, pos)).toEqual({
      kind: "external",
      href: "https://example.com/guide",
      protocol: "https",
    });
  });

  it("resolves a bare autolink", () => {
    const source = "See <https://example.com> for details";
    const state = stateFor(source);
    const pos = source.indexOf("https://example.com") + 4;
    expect(resolveDocumentLinkTarget(state, pos)).toEqual({
      kind: "external",
      href: "https://example.com/",
      protocol: "https",
    });
  });

  it("returns null for plain text and unsupported protocols", () => {
    const source = "Just plain text with [ftp link](ftp://example.com/file)";
    const state = stateFor(source);
    expect(resolveDocumentLinkTarget(state, 5)).toBeNull();
    const pos = source.indexOf("ftp link") + 2;
    expect(resolveDocumentLinkTarget(state, pos)).toBeNull();
  });
});

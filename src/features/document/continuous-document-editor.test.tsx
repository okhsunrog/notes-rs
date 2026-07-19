// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { history } from "@codemirror/commands";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it, vi } from "vite-plus/test";
import { applyDocumentHistoryAction, ContinuousDocumentEditor } from "./continuous-document-editor";

const cleanup: Array<() => void> = [];

function editorView(container: HTMLElement): EditorView {
  const editor = container.querySelector<HTMLElement>(".cm-editor");
  const view = editor ? EditorView.findFromDOM(editor) : null;
  if (!view) throw new Error("CodeMirror view was not mounted");
  return view;
}

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  // jsdom has no layout engine: Range lacks getClientRects, which CodeMirror's own default
  // mousedown handling calls to place the caret. Dispatching a real "mousedown" (needed to test
  // the link-navigation handler, which now runs there instead of on "click") reaches that code
  // path once our handler declines to intercept the event. The stub only prevents the crash; the
  // tests that need a specific position mock `EditorView.posAtCoords` directly.
  if (!Range.prototype.getClientRects) {
    Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
  }
});

afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose();
});

describe("ContinuousDocumentEditor", () => {
  it("stops Mod-z before the app-level backend history boundary", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    await act(async () => {
      root.render(
        <ContinuousDocumentEditor
          value="Draft"
          readOnly={false}
          focusRequest={1}
          onChange={() => undefined}
          onCompositionEnd={() => undefined}
          onBlur={() => undefined}
        />,
      );
    });
    const backendUndo = vi.fn();
    window.addEventListener("keydown", backendUndo);
    cleanup.push(() => window.removeEventListener("keydown", backendUndo));

    const content = container.querySelector<HTMLElement>(".cm-content");
    expect(content).not.toBeNull();
    const event = new KeyboardEvent("keydown", {
      key: "z",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    content!.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(backendUndo).not.toHaveBeenCalled();
  });

  it("does not let local undo mutate a read-only pane", () => {
    const parent = document.createElement("div");
    document.body.append(parent);
    const readOnly = new Compartment();
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: "Original",
        extensions: [history(), readOnly.of(EditorState.readOnly.of(false))],
      }),
    });
    cleanup.push(() => {
      view.destroy();
      parent.remove();
    });
    view.dispatch({ changes: { from: view.state.doc.length, insert: " changed" } });
    view.dispatch({ effects: readOnly.reconfigure(EditorState.readOnly.of(true)) });

    expect(applyDocumentHistoryAction(view, "undo")).toBe(true);
    expect(view.state.doc.toString()).toBe("Original changed");
  });

  it("uses a hard key boundary so history and selection cannot cross pages", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    const renderPage = (pageUuid: string, value: string) => (
      <ContinuousDocumentEditor
        key={pageUuid}
        value={value}
        readOnly={false}
        focusRequest={0}
        onChange={() => undefined}
        onCompositionEnd={() => undefined}
        onBlur={() => undefined}
      />
    );
    await act(async () => root.render(renderPage("page-a", "Document A")));
    const firstEditor = container.querySelector(".cm-editor");

    await act(async () => root.render(renderPage("page-b", "Document B")));
    const secondEditor = container.querySelector(".cm-editor");

    expect(secondEditor).not.toBe(firstEditor);
    expect(secondEditor?.textContent).toContain("Document B");
    expect(secondEditor?.textContent).not.toContain("Document A");
  });

  it("maps the caret through a remote insertion above its logical line", async () => {
    const original = "Heading\nKeep caret on this line\nTail";
    const updated = `Remote preface\n${original}`;
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    const render = (value: string) => (
      <ContinuousDocumentEditor
        value={value}
        readOnly={false}
        focusRequest={0}
        onChange={() => undefined}
        onCompositionEnd={() => undefined}
        onBlur={() => undefined}
      />
    );
    await act(async () => root.render(render(original)));
    const view = editorView(container);
    const caretOffset = original.indexOf("caret") + "caret".length;
    act(() => view.dispatch({ selection: { anchor: caretOffset } }));

    await act(async () => root.render(render(updated)));

    expect(view.state.selection.main.anchor).toBe(updated.indexOf("caret") + "caret".length);
    expect(view.state.doc.lineAt(view.state.selection.main.anchor).text).toBe(
      "Keep caret on this line",
    );
  });

  it("navigates a decorated link on a plain click, Shift for adjacent", async () => {
    const source = "plain\nSee [[Project Aurora]] today";
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const onOpenMarkdownLink = vi.fn();
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    await act(async () =>
      root.render(
        <ContinuousDocumentEditor
          value={source}
          readOnly={false}
          focusRequest={0}
          pageUuid="page-uuid"
          onChange={() => undefined}
          onCompositionEnd={() => undefined}
          onBlur={() => undefined}
          onOpenMarkdownLink={onOpenMarkdownLink}
        />,
      ),
    );
    // The default caret sits on line 1 ("plain"), so the link on line 2 renders decorated.
    const link = container.querySelector<HTMLElement>(".cm-lp-link");
    expect(link).not.toBeNull();
    const view = editorView(container);
    // jsdom has no real layout engine, so posAtCoords cannot resolve real pixel coordinates here;
    // this stands in for the browser's own coordinate-to-position mapping while still exercising
    // the production mousedown handler, decoration-state gate, and syntax-tree target resolution
    // unmocked.
    vi.spyOn(view, "posAtCoords").mockReturnValue(source.indexOf("Project Aurora"));

    act(() => {
      link!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    });
    expect(onOpenMarkdownLink).toHaveBeenCalledWith({
      context: { kind: "note", presentation: "live_preview", pageUuid: "page-uuid" },
      disposition: "current",
      target: { kind: "page", title: "Project Aurora" },
    });

    onOpenMarkdownLink.mockClear();
    act(() => {
      link!.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true, shiftKey: true }),
      );
    });
    expect(onOpenMarkdownLink).toHaveBeenCalledWith(
      expect.objectContaining({ disposition: "adjacent" }),
    );
  });

  it("leaves a plain click as caret placement once the link's line already holds the caret", async () => {
    const source = "plain\nSee [[Project Aurora]] today";
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const onOpenMarkdownLink = vi.fn();
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    await act(async () =>
      root.render(
        <ContinuousDocumentEditor
          value={source}
          readOnly={false}
          focusRequest={0}
          pageUuid="page-uuid"
          onChange={() => undefined}
          onCompositionEnd={() => undefined}
          onBlur={() => undefined}
          onOpenMarkdownLink={onOpenMarkdownLink}
        />,
      ),
    );
    const view = editorView(container);
    const linkPos = source.indexOf("Project Aurora");
    act(() => {
      view.focus();
      view.dispatch({ selection: { anchor: linkPos } });
    });
    vi.spyOn(view, "posAtCoords").mockReturnValue(linkPos);
    const content = container.querySelector<HTMLElement>(".cm-content");

    act(() => {
      content!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    });
    expect(onOpenMarkdownLink).not.toHaveBeenCalled();

    act(() => {
      content!.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true, ctrlKey: true }),
      );
    });
    expect(onOpenMarkdownLink).toHaveBeenCalledWith(
      expect.objectContaining({ disposition: "current" }),
    );
  });

  it("never navigates on a plain click in Source mode, where nothing is decorated", async () => {
    const source = "See [[Project Aurora]] today";
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const onOpenMarkdownLink = vi.fn();
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    await act(async () =>
      root.render(
        <ContinuousDocumentEditor
          value={source}
          readOnly={false}
          focusRequest={0}
          mode="source"
          pageUuid="page-uuid"
          onChange={() => undefined}
          onCompositionEnd={() => undefined}
          onBlur={() => undefined}
          onOpenMarkdownLink={onOpenMarkdownLink}
        />,
      ),
    );
    const view = editorView(container);
    const linkPos = source.indexOf("Project Aurora");
    vi.spyOn(view, "posAtCoords").mockReturnValue(linkPos);
    const content = container.querySelector<HTMLElement>(".cm-content");

    act(() => {
      content!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    });
    expect(onOpenMarkdownLink).not.toHaveBeenCalled();

    act(() => {
      content!.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true, ctrlKey: true }),
      );
    });
    expect(onOpenMarkdownLink).toHaveBeenCalledWith(
      expect.objectContaining({ disposition: "current" }),
    );
  });

  it("resolves the actual clicked position rather than the start of the line", async () => {
    const source = "[[Project Aurora]] trailing plain text";
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    const onOpenMarkdownLink = vi.fn();
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    await act(async () =>
      root.render(
        <ContinuousDocumentEditor
          value={source}
          readOnly={false}
          focusRequest={0}
          pageUuid="page-uuid"
          onChange={() => undefined}
          onCompositionEnd={() => undefined}
          onBlur={() => undefined}
          onOpenMarkdownLink={onOpenMarkdownLink}
        />,
      ),
    );
    const content = container.querySelector<HTMLElement>(".cm-content");
    expect(content).not.toBeNull();
    const view = editorView(container);
    // A DOM click on plain text targets the containing .cm-line, not a decorated span. The
    // resolver must use the actual pointer position, not fall back to the line/element start —
    // otherwise clicking unrelated trailing text on a line that starts with a link would wrongly
    // navigate to that link.
    vi.spyOn(view, "posAtCoords").mockReturnValue(source.indexOf("trailing"));

    act(() => {
      content!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    });
    expect(onOpenMarkdownLink).not.toHaveBeenCalled();
  });
});
